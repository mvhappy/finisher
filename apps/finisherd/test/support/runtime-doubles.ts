// Test doubles for the agent runtime (plan 7A Task 6b): the start planner, the project registry
// (branches in memory, `branch.create` recorded), a recording journal, a `fin`-like tool
// executor whose writes are keyed by `FIN_CALL`, as the real `fin` tool's are, and Task 7's real
// `AgentCore` (credentials minted, refreshed and revoked in the test's `control.db`).
import { randomBytes } from "node:crypto";
import type { Principal } from "@finisher/contracts";
import type { BranchView } from "@finisher/core-types";
import { Effect } from "effect";
import type { ProviderAdapter } from "../../src/agent/adapter.ts";
import { type Clock, manualClock, systemIds } from "../../src/agent/clock.ts";
import type { AgentCore } from "../../src/agent/core.ts";
import { type BranchRegistry, SessionCredentials } from "../../src/agent/credentials.ts";
import type { Rollbacks } from "../../src/agent/rollback.ts";
import { OutcomeTracker } from "../../src/agent/outcomes.ts";
import { ReviewLink } from "../../src/agent/review-link.ts";
import { BranchLineage } from "../../src/agent/reach.ts";
import { Runtime, type RuntimeDeps } from "../../src/agent/runtime.ts";
import type { Pricing, RuntimeJournal, SessionPlan, SessionPlanner, StartRequest, ToolCall, ToolExecutor, ToolOutcome } from "../../src/agent/runtime-deps.ts";
import { NO_JOURNAL, REPORTED_PRICING } from "../../src/agent/runtime-deps.ts";
import type { AgentStore } from "../../src/agent/store.ts";
import { Credentials } from "../../src/auth/credentials.ts";
import { Audit } from "../../src/control/audit.ts";
import type { ControlStore } from "../../src/control/db.ts";
import type { CallMeta } from "../../src/diagnostics.ts";
import { finError } from "../../src/errors.ts";

export const PEOPLE: Principal = { kind: "local", id: "local", scopes: [], projects: "*", branches: "*", human: true };
/** The start call's actor and guard, as the handler builds them (`{...actorFor(by, headers), guard}`). */
export const META: CallMeta = { actor: { kind: "local" } };

/** Reported costs, and every model priced (the planner's `openrouter` models start). */
export const TEST_PRICING: Pricing = { ...REPORTED_PRICING, known: () => true };

export function planner(over: Partial<SessionPlan> = {}): SessionPlanner & { readonly plans: StartRequest[] } {
  const plans: StartRequest[] = [];
  return {
    plans,
    plan: async (req) => {
      plans.push(req);
      return {
        harness: req.harness ?? "fake",
        provider: "openrouter",
        model: req.model ?? "test/closed-explicit",
        weights: "closed",
        role: req.role ?? "editor",
        mode: req.mode ?? "draft",
        budget: req.budget ?? {},
        ...over,
      };
    },
  };
}

const view = (id: string, name: string, head: number, parent: string | null, createdBy: string): BranchView => ({
  id,
  name,
  display: name,
  head,
  forkSeq: head,
  parent,
  createdBy,
  state: "active",
});

/** The project registry's branch calls, in memory: every project starts with `main` at seq 3. */
export class FakeRegistry implements BranchRegistry {
  readonly created: { projectId: string; name: string; from: string; author: string; meta: CallMeta }[] = [];
  readonly #branches = new Map<string, BranchView[]>();

  list(projectId: string): BranchView[] {
    let bs = this.#branches.get(projectId);
    if (bs === undefined) this.#branches.set(projectId, (bs = [view("main", "main", 3, null, "local")]));
    return bs;
  }

  branches(projectId: string, _all: boolean) {
    return Effect.succeed({ branches: [...this.list(projectId)] });
  }

  resolveBranch(projectId: string, ref: string) {
    const b = this.list(projectId).find((x) => x.id === ref || x.name === ref);
    return b === undefined ? Effect.fail(finError("BRANCH_NOT_FOUND", `no branch ${ref}`, { branch: ref })) : Effect.succeed(b.id);
  }

  history(projectId: string, op: "branchCreate" | "branchSet", p: Record<string, unknown>, meta: CallMeta) {
    return Effect.sync(() => {
      const bs = this.list(projectId);
      if (op === "branchSet") {
        const i = bs.findIndex((b) => b.id === p.branch);
        if (i >= 0 && p.archived === true) bs[i] = { ...bs[i]!, state: "archived" };
        return { dryRun: false, record: {} };
      }
      const b = view(`br${bs.length}`, p.name as string, (p.at as { seq: number }).seq, p.from as string, p.author as string);
      bs.push(b);
      this.created.push({ projectId, name: b.name, from: b.parent!, author: b.createdBy, meta });
      return { branch: b, dryRun: false, record: {} };
    });
  }
}

/** Task 7's per-server core over `store`, as `main.ts` builds it (connections and tickets stubbed). */
export function testCore(store: AgentStore, control: ControlStore, clock: Clock): AgentCore & { readonly auth: Credentials } {
  const audit = new Audit(control, () => clock.now());
  const auth = new Credentials(control, randomBytes(32), audit, () => clock.now());
  const credentials = new SessionCredentials({
    store,
    credentials: auth,
    revoke: { audit, tickets: { dropWhere: () => 0 }, connections: { closeWhere: () => 0, revoke: () => {} }, revoke: (ref) => auth.markEnded(ref, "owner"), persist: () => auth.persist() },
  });
  const lineage = new BranchLineage();
  const outcomes = new OutcomeTracker({ store, lineage, clock: () => clock.now(), verdicts: { version: () => null, emit: () => false } });
  return { store, credentials, lineage, outcomes, reviewLink: new ReviewLink({ store, lineage }), clock, ids: systemIds(clock), journal: NO_JOURNAL, auth };
}

export function journal(): RuntimeJournal & { readonly entries: { kind: string; [k: string]: unknown }[]; failSessions: boolean } {
  const entries: { kind: string; [k: string]: unknown }[] = [];
  const j = {
    entries,
    failSessions: false,
    session: (sessionId: string, _s: unknown, phase: string, reason: string | null) => {
      if (j.failSessions) throw new Error("journal unavailable");
      entries.push({ kind: "session", sessionId, phase, reason });
    },
    turn: (r: { runId: string; usage: unknown }) => void entries.push({ kind: "turn", runId: r.runId, usage: r.usage }),
    capture: (r: { runId: string }) => void entries.push({ kind: "capture", runId: r.runId }),
    toolCall: (r: { callId: string; tool: string }) => void entries.push({ kind: "toolCall", callId: r.callId, tool: r.tool }),
    toolResult: (r: { callId: string; synthetic: boolean }) => void entries.push({ kind: "toolResult", callId: r.callId, synthetic: r.synthetic }),
    approval: (r: { approval: { id: string } }) => void entries.push({ kind: "approval", id: r.approval.id }),
  };
  return j;
}

/**
 * A `fin` stand-in: `item …` is a write keyed by the call id (`FIN_CALL`, as `FIN_CALL_IDEM=1`
 * makes it for the built-in tool), everything else a read. `hold` makes calls wait for `release`.
 */
export class FinTool implements ToolExecutor {
  readonly calls: ToolCall[] = [];
  /** Commits by idempotency key. */
  readonly commits = new Map<string, string[]>();
  readonly #held = new Map<string, () => void>();
  hold = false;
  /** Runs before a call (after it is counted): a test's crash point. */
  before: ((c: ToolCall) => Promise<"continue" | "hang">) | null = null;
  /** Runs after a write committed and before the result is returned. */
  afterCommit: ((c: ToolCall) => Promise<"continue" | "hang">) | null = null;

  async run(c: ToolCall): Promise<ToolOutcome> {
    this.calls.push(c);
    if (this.before && (await this.before(c)) === "hang") return new Promise<never>(() => {});
    if (this.hold) await new Promise<void>((resolve) => this.#held.set(c.callId, resolve));
    const argv = (c.input as { argv?: string[] } | null)?.argv ?? [];
    if (argv[0] === "item") {
      this.commit(c.callId, argv);
      if (this.afterCommit && (await this.afterCommit(c)) === "hang") return new Promise<never>(() => {});
      return { ok: true, output: { txId: `tx-${c.callId}` } };
    }
    return { ok: true, output: { argv } };
  }

  /** A write under its idempotency key: a second one with the key is a dedupe. */
  commit(key: string, argv: string[]): void {
    if (!this.commits.has(key)) this.commits.set(key, argv);
  }

  waiting(): string[] {
    return [...this.#held.keys()];
  }

  release(callId?: string): void {
    for (const [id, go] of [...this.#held]) {
      if (callId !== undefined && id !== callId) continue;
      this.#held.delete(id);
      go();
    }
  }
}

export interface Harness {
  readonly runtime: Runtime;
  readonly core: ReturnType<typeof testCore>;
  readonly planner: ReturnType<typeof planner>;
  readonly registry: FakeRegistry;
  readonly journal: ReturnType<typeof journal>;
  readonly tool: FinTool;
  readonly clock: ReturnType<typeof manualClock>;
}

export function makeRuntime(
  { store, control }: { readonly store: AgentStore; readonly control: ControlStore },
  adapter: ProviderAdapter,
  opts: Partial<Omit<RuntimeDeps, "core">> & { clock?: ReturnType<typeof manualClock>; tool?: FinTool; registry?: FakeRegistry; rollbacks?: Rollbacks } = {},
): Harness {
  const clock = opts.clock ?? manualClock(Date.now());
  const h = {
    core: testCore(store, control, clock),
    planner: planner(),
    registry: opts.registry ?? new FakeRegistry(),
    journal: journal(),
    tool: opts.tool ?? new FinTool(),
    clock,
  };
  const runtime = new Runtime({
    core: h.core,
    registry: h.registry,
    adapters: [adapter],
    planner: h.planner,
    environment: { prepare: async (s) => ({ system: `You are the ${s.role}.`, cwd: `/workspace/${s.id}`, env: { FIN_SESSION: s.id } }) },
    tools: { fake: h.tool },
    journal: h.journal,
    timers: false,
    pricing: TEST_PRICING,
    ...opts,
  });
  return { runtime, ...h };
}
