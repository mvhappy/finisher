// The agent runtime's journal entries (plan 7A Task 10; Decisions 25, 26; spec §12): the
// runtime engine's `RuntimeJournal`, recording into the two stores of D34.
//
// - **Metadata** goes to the diagnostics journal (`<server-data>/events`) whenever it is on:
//   `agent.session` per phase (totals at `ended`), `agent.turn` per model call,
//   `agent.toolCall` at announcement, `agent.toolResult` at completion (`txIds` from
//   `session_txs`, `attribution`), `agent.approval` when one is answered or expires. Every
//   session and turn carries its **provenance** (`harness`, `provider`, `model`, `weights`;
//   `upstream` on OpenRouter turns), read from the session and the capture at the time, so a
//   later filter needs no join (spec §12.7; owner decision 1). A local model's base URL enters
//   no record: nothing here reads it.
// - **Content** goes to the agent-trace store (`<server-data>/agent-traces`) only while
//   training is on (only a person turns it on) and the project records: the same entries with
//   their content fields (`messagesDelta`, `output`, `system`, `tools`, `bodyBlake3`; `args`;
//   `output`; `subject`), large ones as blobs (`journal-content.ts`). Diagnostics entries never
//   carry content.
// - **Provenance in the trace store too** (Task 10 review B1): it holds every `agent.session`
//   phase and every turn (a stream turn, or one whose blobs were refused, without content), and
//   its calls, results and approvals carry the session's `provenance`. Content is never there
//   without the harness, provider, model and weights that made it, even once `events/` aged out.
// - **Deltas chain only through recorded turns** (review H1): a turn's delta is based on the
//   previous turn of its run only when that turn's content is in the trace store (`baseTurnId`);
//   otherwise it starts over (`from: 0`). After a compaction turn (`purpose: compaction`, 7B
//   Decision 24) it starts over too: the next request no longer holds the runs the summary
//   replaced, and a replay never splices a compacted history onto an uncompacted one.
// - **Off means off:** each call checks the gates (one atomic load each) and the project's
//   `collect` policy before it builds a payload; a project that is `never` or unknown records
//   nothing in either store, and no blob is attached for it.
// - **Best effort:** a refusal is logged, never thrown (the engine relies on it).
// - **At least once:** `session` phases and synthetic tool results come from the outbox, which
//   redelivers after a crash between the record and `outboxDone`. Within a process a call id's
//   result and an approval are recorded once; across a crash a phase or synthetic result may
//   appear twice. Duplicates are harmless by construction: readers key `agent.toolResult` by
//   `callId` and `agent.approval` by `approvalId` (first wins), and collapse an `agent.session`
//   that repeats the previous entry of its session (same phase and reason).
import type { Approval, Session, Usage } from "@finisher/contracts";
import { FIN_ERROR_CODES } from "../console/exit-codes.ts";
import type { AgentApproval, AgentSession, AgentToolCall, AgentToolResult, AgentTurn, Attribution, Body, SessionTotals } from "@finisher/core-types";
import type { Journal } from "@finisher/fin-node";
import { commandOfArgv } from "./command-of.ts";
import type { Clock, Ids } from "./clock.ts";
import { blobOf, bodyOf, type CaptureMeta, captureMeta, hashOf, splitRequest } from "./journal-content.ts";
import type { RuntimeJournal } from "./runtime-deps.ts";
import type { JournalPhase } from "./session.ts";
import type { AgentStore } from "./store.ts";

/** What the journal uses of a fin-node `Journal` (the diagnostics journal or the trace store). */
export type JournalSink = Pick<Journal, "enabled" | "training" | "record" | "recordsProject" | "attachBlob">;
export type JournalStore = Pick<AgentStore, "get" | "txsOfCall" | "txsOfSession" | "approvals" | "nodes">;

export interface AgentJournalDeps {
  readonly diagnostics: JournalSink;
  readonly traces: JournalSink;
  readonly store: JournalStore;
  readonly ids: Ids;
  readonly clock: Clock;
  readonly log?: (message: string, fields?: Record<string, unknown>) => void;
}

// The wire forms finisherd sends (fin-core `wire_agent.rs`): raw model ids and upstreams,
// unkeyed `blake3:` hashes and raw txIds, which the journal names, hashes and keys.
type TurnIn = Omit<AgentTurn, "modelHash" | "upstreamHash">;
type SessionIn = Omit<AgentSession, "modelHash">;

const SOURCE = "agent";
const KV = 1;
const MAX_TX_IDS = 64;
const REMEMBERED = 4096;
const PROJECT_ID = /^[0-9a-hjkmnp-tv-z]{26}$/;
const BRANCH_ID = /^(main|br[0-9]+)$/;
const CODE = /^[A-Z][A-Z0-9_]{1,63}$/;
/** Harnesses whose tools Finisher runs with `FIN_CALL` set: every join is exact (spec §12.3). */
const EXACT_HARNESSES: ReadonlySet<string> = new Set(["builtin", "fake"]);

/** A code as the journal stores it: `serverRestart` → `SERVER_RESTART`; undefined if not one. */
export function codeOf(reason: string | null | undefined): string | undefined {
  if (reason === null || reason === undefined) return undefined;
  const c = CODE.test(reason) ? reason : reason.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toUpperCase();
  return CODE.test(c) ? c : undefined;
}

/** A tool result's `code`: one of the FinError table's, else `OTHER` (free text never enters a record). */
export function resultCodeOf(code: unknown): string | undefined {
  if (code === undefined || code === null) return undefined;
  const c = typeof code === "string" ? codeOf(code) : undefined;
  return c !== undefined && FIN_ERROR_CODES.has(c) ? c : "OTHER";
}

/** A tool's `AGENT_TOOLS` name (the built-in `workspace_*` tools are `workspace`). */
export const toolOf = (tool: string) => (tool.startsWith("workspace_") ? "workspace" : tool);

/** A bounded map: the oldest key goes when it is full. */
class Recent<K, V> extends Map<K, V> {
  override set(k: K, v: V): this {
    super.delete(k);
    super.set(k, v);
    if (this.size > REMEMBERED) this.delete(this.keys().next().value as K);
    return this;
  }
}

interface RunState {
  readonly sessionId: string;
  seq: number;
  lastTurnId?: string;
  /** The id a turn not yet recorded takes (a call announced before the run's first turn). */
  nextTurnId?: string | undefined;
  /** The capture of the call whose `usage` comes next. */
  capture?: CaptureMeta | undefined;
  /** The messages (after the system message) of the run's last turn whose content is in the trace store: the next delta's base. */
  previous: unknown[];
  /** That turn's id (the next delta's `baseTurnId`); undefined: the next delta starts over. */
  previousTurnId?: string | undefined;
}

export class AgentJournal implements RuntimeJournal {
  readonly #d: AgentJournalDeps;
  /** The last view of each session: its provenance once it is deleted. */
  readonly #sessions = new Recent<string, Session>();
  readonly #runs = new Recent<string, RunState>();
  readonly #calls = new Recent<string, { turnId: string; at: number }>();
  /** Call ids with a result, approvals recorded (once per process). */
  readonly #done = new Recent<string, true>();

  constructor(deps: AgentJournalDeps) {
    this.#d = deps;
  }

  #on(): boolean {
    return this.#d.diagnostics.enabled || this.#d.traces.training;
  }

  #guard(what: string, f: () => void): void {
    try {
      f();
    } catch (e) {
      this.#d.log?.(`agent journal: ${what} was not recorded`, { error: (e as { code?: string }).code ?? String(e) });
    }
  }

  #ctx(s: Session, call?: string): string {
    return JSON.stringify({
      ...(PROJECT_ID.test(s.projectId) ? { project: s.projectId } : {}),
      ...(s.rootBranch !== undefined && BRANCH_ID.test(s.rootBranch) ? { branch: s.rootBranch } : {}),
      actor: { kind: "agent", session: s.id, ...(call === undefined ? {} : { call }) },
    });
  }

  /**
   * Records `meta` in the diagnostics journal, and in the trace store `meta` with the session's
   * `provenance` (`stamp`: a kind that does not carry it flat) and `content()`, or without the
   * content when it has none or a blob was refused. True when the trace store took the entry
   * with its content.
   */
  #emit(kind: string, s: Session, meta: object, opts: { content?: () => object | undefined; call?: string; stamp?: boolean } = {}): boolean {
    const ctx = this.#ctx(s, opts.call);
    const d = this.#d.diagnostics;
    if (d.enabled && d.recordsProject(s.projectId)) this.#guard(`a ${kind} entry`, () => d.record(kind, KV, ctx, JSON.stringify(meta), SOURCE));
    const t = this.#d.traces;
    if (!t.training || !t.recordsProject(s.projectId)) return false;
    const c = opts.content?.();
    const entry = { ...meta, ...(opts.stamp === true ? { provenance: this.#provenance(s) } : {}), ...c };
    t.record(kind, KV, ctx, JSON.stringify(entry), SOURCE);
    return c !== undefined;
  }

  #remember(s: Session): Session {
    this.#sessions.set(s.id, s);
    return s;
  }

  #run(runId: string, sessionId: string): RunState {
    let r = this.#runs.get(runId);
    if (r === undefined) {
      r = { sessionId, seq: 0, previous: [] };
      this.#runs.set(runId, r);
    }
    return r;
  }

  #provenance(s: Session) {
    return { harness: s.harness, provider: s.provider, model: s.model, weights: s.weights } as const;
  }

  // RuntimeJournal ---------------------------------------------------------------------------

  session(sessionId: string, session: Session | undefined, phase: JournalPhase, reason: string | null): void {
    this.#guard(`the ${phase} of session ${sessionId}`, () => {
      const s = session === undefined ? this.#sessions.get(sessionId) : this.#remember(session);
      if (phase === "interrupted" || phase === "ended") {
        for (const [id, r] of this.#runs) {
          if (r.sessionId !== sessionId) continue;
          if (phase === "ended") this.#runs.delete(id);
          else r.capture = undefined;
        }
      }
      if (s === undefined) {
        if (this.#on()) this.#d.log?.("agent journal: a phase of a deleted session has no provenance; not recorded", { sessionId, phase });
        return;
      }
      if (!this.#on()) return;
      this.#expired(s);
      const code = codeOf(reason);
      const totals = phase === "ended" && session !== undefined ? this.#totals(session) : undefined;
      const meta: SessionIn = {
        sessionId: s.id,
        phase,
        ...this.#provenance(s),
        role: s.role,
        mode: s.mode,
        ...(s.parent === undefined ? {} : { parent: s.parent }),
        trigger: s.parent === undefined ? "user" : "subagent",
        ...(code === undefined ? {} : { reason: code }),
        ...(totals === undefined ? {} : { totals }),
      };
      this.#emit("agent.session", s, meta);
      if (phase === "ended") this.#sessions.delete(sessionId);
    });
  }

  #totals(s: Session): SessionTotals {
    let turns = 0;
    let toolCalls = 0;
    for (let from = 1; ; ) {
      const page = this.#d.store.nodes(s.id, from, 500);
      for (const n of page) {
        if (n.kind === "usage") turns += 1;
        else if (n.kind === "toolCall") toolCalls += 1;
      }
      if (page.length < 500) break;
      from = page[page.length - 1]!.seq + 1;
    }
    const u = s.usage;
    return {
      turns,
      toolCalls,
      txs: this.#d.store.txsOfSession(s.id).length,
      inputTokens: u.input,
      outputTokens: u.output,
      ...(u.costBasis === undefined || u.costBasis === "unknown" ? {} : { costMicros: u.moneyMicros }),
      wallS: Math.max(0, Math.floor(((s.endedAt ?? this.#d.clock.now()) - s.createdAt) / 1000)),
    };
  }

  capture(r: { session: Session; runId: string; request: unknown; response: unknown }): void {
    this.#guard("a capture", () => {
      if (!this.#on()) return;
      const run = this.#run(r.runId, r.session.id);
      if (run.capture !== undefined) this.#d.log?.("agent journal: a capture had no usage after it; dropped", { sessionId: r.session.id });
      run.capture = captureMeta(r.request, r.response);
    });
  }

  turn(r: { session: Session; runId: string; usage: Usage }): void {
    this.#guard("a turn", () => {
      if (!this.#on()) return;
      const s = this.#remember(r.session);
      const run = this.#run(r.runId, s.id);
      const cap = run.capture;
      run.capture = undefined;
      const turnId = run.nextTurnId ?? this.#d.ids.ulid();
      run.nextTurnId = undefined;
      const parent = run.lastTurnId;
      run.seq += 1;
      run.lastTurnId = turnId;
      const split = cap?.request === undefined ? undefined : splitRequest(cap.request, run.previous, run.previousTurnId);
      // The next delta's base, once this turn's content is known to be in the trace store.
      run.previous = [];
      run.previousTurnId = undefined;
      const u = r.usage;
      const basis = u.costBasis ?? "unknown";
      const meta: TurnIn = {
        turnId,
        runId: r.runId,
        seq: run.seq,
        ...(parent === undefined ? {} : { parentTurnId: parent }),
        ...this.#provenance(s),
        ...(cap?.upstream !== undefined && s.provider === "openrouter" ? { upstream: cap.upstream } : {}),
        inputTokens: u.input,
        outputTokens: u.output,
        cacheReadTokens: u.cacheRead,
        cacheWriteTokens: u.cacheWrite,
        reasoningTokens: u.reasoning,
        latencyMs: cap?.latencyMs ?? 0,
        ...(cap?.ttftMs === undefined ? {} : { ttftMs: cap.ttftMs }),
        stopReason: cap?.stopReason ?? "other",
        ...(basis === "unknown" ? {} : { costMicros: u.moneyMicros }),
        costBasis: basis,
        capture: cap === undefined ? "stream" : "exact",
        ...(cap?.purpose === undefined ? {} : { purpose: cap.purpose }),
        // A stream turn has no prompt or tool definitions of its own: no hash of nothing.
        ...(split === undefined ? {} : { systemHash: hashOf(split.system), toolsHash: hashOf(split.tools) }),
        ...(cap?.request === undefined ? {} : { bodyHash: hashOf(cap.request) }),
      };
      const built = this.#emit("agent.turn", s, meta, {
        content: () => {
          if (cap?.request === undefined || split === undefined) return undefined;
          const t = this.#d.traces;
          const parts = [bodyOf(t, split.delta), bodyOf(t, cap.response), blobOf(t, split.system), blobOf(t, split.tools)];
          if (parts.some((p) => p === undefined)) return undefined;
          const [messagesDelta, output, system, tools] = parts.map((p) => p!.body) as [Body, Body, Body, Body];
          return { bodyBlake3: hashOf(cap.request), messagesDelta, output, system, tools };
        },
      });
      if (built && split !== undefined && cap?.purpose !== "compaction") {
        run.previous = split.messages;
        run.previousTurnId = turnId;
      }
    });
  }

  toolCall(r: { session: Session; runId: string; callId: string; providerId: string; tool: string; input: unknown }): void {
    this.#guard("a tool call", () => {
      if (!this.#on()) return;
      const s = this.#remember(r.session);
      const run = this.#run(r.runId, s.id);
      const turnId = run.lastTurnId ?? (run.nextTurnId ??= this.#d.ids.ulid());
      this.#calls.set(r.callId, { turnId, at: this.#d.clock.now() });
      const command = r.tool === "fin" ? commandOfArgv(r.input) : undefined;
      const meta: AgentToolCall = {
        callId: r.callId,
        turnId,
        tool: toolOf(r.tool),
        ...(command === undefined ? {} : { command }),
        argsHash: hashOf(JSON.stringify(r.input ?? null)),
      };
      this.#emit("agent.toolCall", s, meta, {
        content: () => {
          const args = bodyOf(this.#d.traces, r.input);
          return args === undefined ? undefined : { args: args.body };
        },
        call: r.callId,
        stamp: true,
      });
    });
  }

  toolResult(r: { sessionId: string; runId: string; callId: string; ok: boolean; output: unknown; synthetic: boolean }): void {
    this.#guard("a tool result", () => {
      if (!this.#on() || this.#done.has(`call:${r.callId}`)) return;
      const s = this.#d.store.get(r.sessionId) ?? this.#sessions.get(r.sessionId);
      if (s === undefined) return;
      this.#done.set(`call:${r.callId}`, true);
      const call = this.#calls.get(r.callId);
      this.#calls.delete(r.callId);
      const txs = this.#d.store.txsOfCall(r.callId);
      const out = (r.output ?? null) as { code?: unknown; exit?: unknown } | null;
      const code = r.synthetic ? "SESSION_INTERRUPTED" : r.ok ? undefined : resultCodeOf(out?.code);
      const exit = typeof out?.exit === "number" && Number.isInteger(out.exit) ? out.exit : undefined;
      const meta: Omit<AgentToolResult, "truncated" | "output"> = {
        callId: r.callId,
        ok: r.ok,
        ...(code === undefined ? {} : { code }),
        ...(exit === undefined ? {} : { exit }),
        micros: call === undefined ? 0 : Math.max(0, this.#d.clock.now() - call.at) * 1000,
        ...(txs.length === 0 ? {} : { txIds: txs.slice(0, MAX_TX_IDS).map((t) => t.txId) }),
        attribution: attributionOf(s.harness, txs.map((t) => t.attribution)),
      };
      this.#emit("agent.toolResult", s, meta, {
        content: () => {
          const o = bodyOf(this.#d.traces, r.output);
          return o === undefined ? undefined : { truncated: o.truncated, output: o.body };
        },
        call: r.callId,
        stamp: true,
      });
    });
  }

  approval(r: { session: Session; approval: Approval }): void {
    this.#guard("an approval", () => {
      if (!this.#on() || r.approval.status === "open") return; // opened: its decision is recorded
      this.#approval(this.#remember(r.session), r.approval);
    });
  }

  /** The session's approvals that expired since: recorded at its next phase. */
  #expired(s: Session): void {
    for (const a of this.#d.store.approvals(s.id)) if (a.status === "expired") this.#approval(s, a);
  }

  #approval(s: Session, a: Approval): void {
    const key = `approval:${a.id}`;
    if (this.#done.has(key) || (a.status === "answered" && a.decision === undefined)) return;
    this.#done.set(key, true);
    const expired = a.status === "expired";
    const end = a.answeredAt ?? Math.min(this.#d.clock.now(), a.expiresAt);
    const meta: Omit<AgentApproval, "subject"> = {
      approvalId: a.id,
      sessionId: s.id,
      kind: a.kind,
      decision: expired ? "expired" : a.decision!,
      by: expired || a.by === "policy" ? "policy" : "user",
      latencyMs: Math.max(0, end - a.requestedAt),
    };
    this.#emit("agent.approval", s, meta, {
      content: () => {
        const subject = bodyOf(this.#d.traces, a.subject);
        return subject === undefined ? undefined : { subject: subject.body };
      },
      stamp: true,
    });
  }
}

/** Built-in and fake tools run with `FIN_CALL`: exact. A CLI's own: what the matcher found. */
export function attributionOf(harness: string, found: readonly Attribution[]): Attribution {
  if (EXACT_HARNESSES.has(harness)) return "exact";
  if (found.length === 0) return "none";
  return found.every((a) => a === found[0]) ? found[0]! : "ambiguous";
}
