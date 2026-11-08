// The built-in agent's loop (sub-project 7 §4.4; plan 7A Decisions 6, 7, 9, 10, 13): an adapter
// like any other. Each turn: the prompt (after the run's facts) → a streamed model call → its
// tool calls (reads in parallel, writes one at a time, in order) → their results appended →
// the next call, until the model ends its turn, a per-run cap, the context stop, a budget stop
// or an interrupt.
//
// - **Tools run in the runtime.** The loop announces a call with `toolStarted` and waits for the
//   runtime's `respond(providerId, toolResult)`, then reports `toolCompleted`. The runtime runs
//   a call as soon as it is announced, so a write is announced only once everything before it
//   has its result: that is how writes stay serial.
// - **Budgets:** `spec.gate.beforeCall` before every model attempt; a refusal ends the run.
//   Every attempt is charged: one `usage` event each (a failed attempt's from its cost), or
//   `gate.release()` for one that reported none.
// - **Capture:** one `capture {request, response}` per model call: the exact body bytes as
//   sent (a string; `bodyHash` is its BLAKE3) and the response chunks with the call's metadata.
// - **Caps (Decision 10):** 60 model calls and 200 tool calls per run; each tool result shown to
//   the model is at most 16 KiB (head 12 KiB + tail 4 KiB + a note), and at most 1 MiB of it is
//   stored; at most 8 reads run at once; the run stops with `MODEL_CONTEXT_EXCEEDED` when a
//   call's prompt would pass 85 % of the model's window.
// - **Event order of a model call** (the journal's contract, `RuntimeJournal.capture`): its
//   `capture`, then its `usage`, then its reasoning and message, then its `toolStarted`s.
// - **Resume:** what the loop answers itself (an unknown tool, arguments that are not JSON, a
//   call past the cap) is a `localToolResult` notice, and steers taken into the conversation a
//   `steersTaken` notice, so `historyFromNodes` rebuilds the conversation byte for byte.
import { FinError, type Node, type ProviderEvent } from "@finisher/contracts";
import { type Cause, Effect, Queue, type Scope, Stream } from "effect";
import { finError } from "../../errors.ts";
import { type BoundEvent, type Capabilities, NO_CAPABILITIES, type ProjectedUsage, type ProviderAdapter, type ProviderSession, type RuntimeAnswer, type SessionSpec } from "../adapter.ts";
import type { Catalogue } from "../catalogue/catalogue.ts";
import { type AttemptCost, type KeySource, type ModelClient, type ModelResult, noKey } from "./compat.ts";
import type { FinIndex } from "./fin-tool.ts";
import { boundOutput, historyFromNodes, INTERRUPTED_RESULT, renderResult } from "./loop-history.ts";
import type { ModelMessage, ModelRequest, Preset, ToolCall } from "./request.ts";
import { FIN_TOOL, isWrite, TOOL_DEFS, TOOL_NAMES } from "./tools.ts";

export interface LoopLimits {
  readonly modelCalls: number;
  readonly toolCalls: number;
  /** Bytes of one tool result shown to the model, and its head and tail when cut. */
  readonly resultBytes: number;
  readonly headBytes: number;
  readonly tailBytes: number;
  /** The share of the window a call's prompt may reach. */
  readonly contextRatio: number;
  /** `max_tokens` of each call (capped by the model's `maxOutput`). */
  readonly maxTokens: number;
  /** Reads (and `fin --dry-run`s) running at once. */
  readonly parallelReads: number;
  /** Bytes of one tool result stored (`toolCompleted`); more is cut. */
  readonly storedBytes: number;
}

export const LOOP_LIMITS: LoopLimits = {
  modelCalls: 60,
  toolCalls: 200,
  resultBytes: 16 * 1024,
  headBytes: 12 * 1024,
  tailBytes: 4 * 1024,
  contextRatio: 0.85,
  maxTokens: 8192,
  parallelReads: 8,
  storedBytes: 1024 * 1024,
};

export interface BuiltinDeps {
  /** The model client of a preset (`openrouter`; `local` from the owner's base URL). */
  readonly client: (preset: Preset) => ModelClient;
  /** The OpenRouter key for one request (`ModelGateway.withKey`); the local preset uses none. */
  readonly gateway?: { withKey<A>(projectId: string, sessionId: string, f: (key: string) => Promise<A>): Promise<A> };
  readonly catalogue: Catalogue;
  /** The `fin` binary the tools run (`detect` reports whether it is there). */
  readonly finPath: string;
  /** What the tools' write/read split reads (`fin schema commands`); unknown `fin` commands are writes. */
  readonly index?: () => Promise<FinIndex | undefined>;
  /** A session's stored nodes, to rebuild the conversation when it is opened again (resume). */
  readonly nodes?: (sessionId: string) => ReadonlyArray<Node>;
  readonly limits?: Partial<LoopLimits>;
}

export const BUILTIN_CAPABILITIES: Capabilities = {
  ...NO_CAPABILITIES,
  steer: true,
  resume: true,
  reasoning: true,
  parallelTools: true,
  costReported: true,
  exactCapture: true,
};

type EventQueue = Queue.Queue<BoundEvent, Cause.Done>;
type ToolAnswer = { readonly ok: boolean; readonly output: unknown };

/** Interrupted: the turn ends with `turnCompleted {stopReason: "interrupted"}`. */
class Stopped extends Error {}

const estimate = (chars: number) => Math.ceil(chars / 4);

export function BuiltinAdapter(deps: BuiltinDeps): ProviderAdapter {
  const limits: LoopLimits = { ...LOOP_LIMITS, ...deps.limits };
  let opened = 0;

  const open = (spec: SessionSpec) =>
    Effect.gen(function* () {
      const preset: Preset = spec.provider === "local" ? "local" : "openrouter";
      const info = deps.catalogue.describe(preset, spec.model);
      if (!info.capabilities.tools) {
        return yield* Effect.fail(finError("MODEL_UNKNOWN", `${spec.model} does not take tools, which the built-in agent needs`, { model: spec.model, reason: "noTools" }, "choose a model with tool support"));
      }
      const client = yield* Effect.try({ try: () => deps.client(preset), catch: (e) => (e instanceof FinError ? e : finError("PROVIDER_UNAVAILABLE", String(e), { provider: preset })) });
      const instance = `builtin-${++opened}`;
      const queue: EventQueue = yield* Queue.unbounded<BoundEvent, Cause.Done>();
      const emit = (event: ProviderEvent) => void Effect.runSync(Queue.offer(queue, { instance, event }));
      const key: KeySource =
        preset === "local" || deps.gateway === undefined ? noKey : <A>(f: (k: string) => Promise<A>) => deps.gateway!.withKey(spec.projectId, spec.sessionId, f);
      const maxTokens = Math.max(1, Math.min(limits.maxTokens, info.maxOutput || limits.maxTokens));
      const viewBudget = Math.max(200, Math.min(4000, Math.floor(info.window * 0.05)));

      const history: ModelMessage[] = deps.nodes ? historyFromNodes(deps.nodes(spec.sessionId), limits) : [];
      const steers: string[] = [];
      const waits = new Map<string, { resolve: (a: ToolAnswer) => void; reject: (e: unknown) => void }>();
      const early = new Map<string, ToolAnswer>();
      let running: Promise<void> | null = null;
      let ctl: AbortController | null = null;
      /** Tokens of the last call's prompt and reply, and the history length they cover. */
      let known: { tokens: number; covers: number } | null = null;

      const waitFor = (id: string) =>
        new Promise<ToolAnswer>((resolve, reject) => {
          const ready = early.get(id);
          if (ready) {
            early.delete(id);
            return resolve(ready);
          }
          waits.set(id, { resolve, reject });
        }).then((a) => {
          // Stored as it is shown: cut to `storedBytes` first, so the resume reads the same.
          const bounded = { ok: a.ok, output: boundOutput(a.output, limits.storedBytes) };
          emit({ type: "toolCompleted", providerId: id, ok: bounded.ok, output: bounded.output });
          return bounded;
        });

      /** The prompt tokens of `req`: the last call's count plus the new messages, else all of it. */
      const promptTokens = (req: ModelRequest) => {
        if (known !== null && known.covers <= req.messages.length) {
          return known.tokens + estimate(JSON.stringify(req.messages.slice(known.covers)).length);
        }
        return estimate(JSON.stringify({ s: req.system, t: req.tools, m: req.messages }).length);
      };

      /** Charges one attempt: its usage, else the gate's reservation is released. */
      const settle = (cost: AttemptCost) => {
        if (cost.usage === null) spec.gate?.release();
        else emit({ type: "usage", ...cost.usage, ...(cost.costMicros === undefined ? {} : { costMicros: cost.costMicros }) });
      };

      const admit = (projected: ProjectedUsage): FinError | null => {
        const g = spec.gate?.beforeCall(projected);
        return g === undefined || g.ok ? null : g.error;
      };

      /**
       * One model call with its retries; a refusal by the gate is returned, not thrown. Every
       * admitted attempt is charged exactly once: its `retry` or `failed` cost, the result's usage
       * (emitted by the caller, after the capture), or — when the stream throws or ends without
       * either — the gate's reservation is released (the `finally`).
       */
      async function modelCall(req: ModelRequest, signal: AbortSignal): Promise<{ result: ModelResult } | { refused: FinError }> {
        const projected: ProjectedUsage = { input: promptTokens(req), output: req.maxTokens, cacheRead: 0, cacheWrite: 0, reasoning: 0 };
        const first = admit(projected);
        if (first) return { refused: first };
        let open = true;
        const charge = (cost: AttemptCost) => {
          open = false;
          settle(cost);
        };
        try {
          const it = client.stream(req, key, signal)[Symbol.asyncIterator]();
          for (;;) {
            const step = await it.next();
            if (step.done) throw finError("PROVIDER_PROTOCOL", "the model stream ended without a result", {});
            const ev = step.value;
            switch (ev.type) {
              case "textDelta":
                emit({ type: "messageDelta", text: ev.text });
                break;
              case "reasoningDelta":
                emit({ type: "reasoningDelta", text: ev.text });
                break;
              case "retry": {
                charge(ev.cost);
                emit({ type: "notice", name: "retry", details: { attempt: ev.attempt, code: ev.code, delayMs: ev.delayMs } });
                const again = admit(projected);
                if (again) {
                  await it.return?.();
                  return { refused: again };
                }
                open = true;
                break;
              }
              case "failed":
                charge(ev.cost);
                break;
              case "done":
                open = false;
                return { result: ev.result };
            }
          }
        } finally {
          if (open) spec.gate?.release();
        }
      }

      /** A call the loop answers itself (no runtime run): recorded so the resume has it. */
      const answerLocally = (st: ToolsState, index: number, content: string) => {
        const c = st.recorded[index]!;
        st.results.set(c.id, content);
        emit({ type: "notice", name: "localToolResult", details: { providerId: c.id, tool: c.name, arguments: c.arguments, index, content } });
      };

      /**
       * Runs the calls of one reply: reads together (at most `parallelReads` at once), each write
       * alone, results in call order. `st.recorded` becomes each call as announced (its
       * arguments the JSON of the input the runtime ran) and `st.results` its result text.
       */
      async function runTools(st: ToolsState, used: { tools: number }, signal: AbortSignal): Promise<void> {
        const index = await deps.index?.().catch(() => undefined);
        type Ready = { i: number; id: string; tool: string; input: unknown; write: boolean };
        const ready: Ready[] = [];
        for (const [i, c] of st.recorded.entries()) {
          if (!TOOL_NAMES.has(c.name)) {
            answerLocally(st, i, JSON.stringify({ ok: false, error: { code: "USAGE", message: `no tool named ${c.name}`, tools: [...TOOL_NAMES] } }));
            continue;
          }
          let input: unknown;
          try {
            input = c.arguments.trim() === "" ? {} : JSON.parse(c.arguments);
          } catch {
            answerLocally(st, i, JSON.stringify({ ok: false, error: { code: "USAGE", message: "the tool arguments are not JSON" } }));
            continue;
          }
          if (c.name === FIN_TOOL) input = withViewBudget(input, viewBudget);
          st.recorded[i] = { id: c.id, name: c.name, arguments: JSON.stringify(input) ?? "null" };
          ready.push({ i, id: c.id, tool: c.name, input, write: isWrite(c.name, input, index) });
        }
        let i = 0;
        while (i < ready.length) {
          if (signal.aborted) throw new Stopped();
          // A run of reads, or one write.
          const batch = ready[i]!.write ? [ready[i]!] : takeWhile(ready, i, (r) => !r.write);
          i += batch.length;
          const queue = [...batch];
          const worker = async () => {
            for (let r = queue.shift(); r !== undefined; r = queue.shift()) {
              if (signal.aborted) throw new Stopped();
              if (used.tools >= limits.toolCalls) {
                st.capped = true;
                answerLocally(st, r.i, JSON.stringify({ ok: false, error: { code: "BUDGET_EXCEEDED", message: `the run's ${limits.toolCalls} tool calls are used up`, kind: "toolCalls" } }));
                continue;
              }
              used.tools++;
              emit({ type: "toolStarted", providerId: r.id, tool: r.tool, input: r.input });
              const a = await waitFor(r.id);
              st.results.set(r.id, renderResult(r.id, a, limits));
            }
          };
          await Promise.all(Array.from({ length: Math.min(Math.max(1, limits.parallelReads), batch.length) }, worker));
        }
      }

      async function runTurn(): Promise<void> {
        const signal = ctl!.signal;
        const used = { tools: 0 };
        emit({ type: "turnStarted" });
        try {
          for (let calls = 0; ; calls++) {
            if (steers.length > 0) {
              const taken = steers.splice(0);
              history.push({ role: "user", content: [{ type: "text", text: taken.join("\n\n") }] });
              emit({ type: "notice", name: "steersTaken", details: { count: taken.length } });
            }
            if (calls >= limits.modelCalls) return emit({ type: "turnCompleted", stopReason: "modelCallCap" });
            const req: ModelRequest = { system: spec.system, tools: TOOL_DEFS, messages: [...history], model: spec.model, maxTokens };
            const tokens = promptTokens(req);
            if (tokens > limits.contextRatio * info.window) {
              const e = finError("MODEL_CONTEXT_EXCEEDED", "the conversation fills 85 % of the model's context window", { tokens, window: info.window }, "start a new run or session");
              return emit({ type: "turnFailed", error: { code: e.code, message: e.message, details: e.details, fix: e.fix! } });
            }
            const answer = await modelCall(req, signal);
            if ("refused" in answer) {
              const e = answer.refused;
              const kind = (e.details as { kind?: unknown } | null)?.kind;
              if (kind === "modelCalls") return emit({ type: "turnCompleted", stopReason: "modelCallCap" });
              return emit({ type: "turnFailed", error: { code: e.code, message: e.message, details: e.details ?? {}, ...(e.fix === undefined ? {} : { fix: e.fix }) } });
            }
            const r = answer.result;
            // The journal's order: the capture, then the usage it joins, then the content.
            emit({
              type: "capture",
              request: r.capture.request,
              response: {
                chunks: r.capture.response,
                stopReason: r.stopReason,
                bodyHash: r.bodyHash,
                usageEstimated: r.usageEstimated,
                latencyMs: r.latencyMs,
                ...(r.ttftMs === undefined ? {} : { ttftMs: r.ttftMs }),
                ...(r.upstream === undefined ? {} : { upstream: r.upstream }),
                ...(r.generationId === undefined ? {} : { generationId: r.generationId }),
              },
            });
            emit({ type: "usage", ...r.usage, ...(r.costMicros === undefined ? {} : { costMicros: r.costMicros }) });
            if (r.reasoningText !== "" || r.reasoning.length > 0) {
              emit({ type: "reasoning", ...(r.reasoningText === "" ? {} : { text: r.reasoningText }), ...(r.reasoning.length === 0 ? {} : { encrypted: r.reasoning }) });
            }
            if (r.text !== "") emit({ type: "message", role: "assistant", blocks: [{ type: "text", text: r.text }] });
            const tokensNow = r.usage.input + r.usage.cacheRead + r.usage.cacheWrite + r.usage.output;
            /** The reply as the conversation keeps it (and `historyFromNodes` rebuilds it). */
            const reply = (calls: readonly ToolCall[]) => {
              if (r.text !== "" || calls.length > 0 || r.reasoning.length > 0) {
                history.push({
                  role: "assistant",
                  ...(r.text === "" ? {} : { text: r.text }),
                  ...(calls.length === 0 ? {} : { toolCalls: [...calls] }),
                  ...(r.reasoning.length === 0 ? {} : { reasoning: r.reasoning }),
                });
              }
              known = { tokens: tokensNow, covers: history.length };
            };
            if (r.toolCalls.length === 0) {
              reply([]);
              return emit({ type: "turnCompleted", stopReason: r.stopReason });
            }
            const st: ToolsState = { recorded: uniqueIds(r.toolCalls), results: new Map(), capped: false };
            try {
              await runTools(st, used, signal);
            } finally {
              // Even when stopped: every call keeps a result, so the next request is valid.
              reply(st.recorded);
              for (const c of st.recorded) history.push({ role: "tool", toolCallId: c.id, content: st.results.get(c.id) ?? INTERRUPTED_RESULT });
            }
            if (st.capped) return emit({ type: "turnCompleted", stopReason: "toolCallCap" });
          }
        } catch (e) {
          if (e instanceof Stopped || signal.aborted) return emit({ type: "turnCompleted", stopReason: "interrupted" });
          const err = e instanceof FinError ? e : finError("INTERNAL", e instanceof Error ? e.message : String(e), {});
          emit({ type: "turnFailed", error: { code: err.code, message: err.message, details: err.details ?? {}, ...(err.fix === undefined ? {} : { fix: err.fix }) } });
        }
      }

      const stop = async () => {
        const turn = running;
        if (turn === null) return;
        ctl?.abort(new Stopped());
        for (const w of waits.values()) w.reject(new Stopped());
        waits.clear();
        await turn;
      };

      yield* Effect.addFinalizer(() => Effect.promise(stop).pipe(Effect.andThen(Queue.end(queue)), Effect.asVoid));

      const session: ProviderSession = {
        instance,
        events: Stream.fromQueue(queue),
        startTurn: (i) =>
          Effect.suspend(() => {
            if (running !== null) return Effect.fail(finError("SESSION_BUSY", "a turn is running", { instance }));
            const blocks = [...(i.facts === undefined ? [] : [{ type: "text" as const, text: i.facts }]), { type: "text" as const, text: i.text }];
            history.push({ role: "user", content: blocks });
            ctl = new AbortController();
            early.clear();
            running = runTurn().finally(() => {
              running = null;
            });
            return Effect.void;
          }),
        steer: (i) =>
          Effect.sync(() => {
            steers.push(i.text);
          }),
        interrupt: () => Effect.promise(stop),
        respond: (id: string, a: RuntimeAnswer) =>
          Effect.sync(() => {
            if (a.kind !== "toolResult") return;
            const answer = { ok: a.ok, output: a.output };
            const w = waits.get(id);
            if (w) {
              waits.delete(id);
              w.resolve(answer);
            } else {
              early.set(id, answer);
            }
          }),
        rollback: () => Effect.fail(finError("PROVIDER_UNAVAILABLE", "the built-in agent rolls back in 7B", {})),
        ref: () => ({ kind: "builtin", id: spec.sessionId }),
      };
      return session;
    });

  return {
    harness: "builtin",
    detect: () =>
      Effect.tryPromise({
        try: async () => {
          const { stat } = await import("node:fs/promises");
          const installed = await stat(deps.finPath).then((s) => s.isFile(), () => false);
          return { installed, version: "builtin-1", supported: "builtin-1", signedIn: true, capabilities: BUILTIN_CAPABILITIES };
        },
        catch: (e) => finError("INTERNAL", String(e), {}),
      }),
    open: (spec) => open(spec) as Effect.Effect<ProviderSession, FinError, Scope.Scope>,
  };
}

interface ToolsState {
  /** The reply's calls, ids made unique; each becomes the call as announced. */
  recorded: ToolCall[];
  readonly results: Map<string, string>;
  capped: boolean;
}

/**
 * The calls with unique ids (review M8): a repeated (or empty) id gets a suffix, so every call
 * is announced, answered and waited for once. The conversation keeps the new ids.
 */
export function uniqueIds(calls: readonly ToolCall[]): ToolCall[] {
  const given = new Set(calls.map((c) => c.id));
  const used = new Set<string>();
  return calls.map((c, i) => {
    let id = c.id;
    if (id === "" || used.has(id)) {
      const base = id === "" ? `call_${i}` : id;
      id = base;
      for (let n = 2; used.has(id) || (id !== c.id && given.has(id)); n++) id = `${base}_${n}`;
    }
    used.add(id);
    return id === c.id ? c : { ...c, id };
  });
}

function takeWhile<T>(xs: readonly T[], from: number, p: (x: T) => boolean): T[] {
  const out: T[] = [];
  for (let i = from; i < xs.length && p(xs[i]!); i++) out.push(xs[i]!);
  return out;
}

/** `fin view` gets a `--budget` from the window when it has none (spec §4.4). */
function withViewBudget(input: unknown, budget: number): unknown {
  const argv = (input as { argv?: unknown } | null)?.argv;
  if (!Array.isArray(argv) || argv[0] !== "view" || argv.some((a) => a === "--budget" || (typeof a === "string" && a.startsWith("--budget="))) || argv.includes("--help")) return input;
  return { ...(input as object), argv: [...argv, "--budget", String(budget)] };
}
