// The agent runtime (sub-project 7 §3; plan 7A Task 6b, Decisions 2, 3, 5, 6): sessions,
// runs and their provider instances over `AgentStore`. The commands an RPC handler or the
// console calls; `runtime-engine.ts` holds the event handling, budgets and the outbox.
//
// - `boot()` first (Decision 3, the Task 3b order): crash recovery (reservations returned), the
//   orphan sweep at grace 0 before any session work, the outbox drained, every live session
//   credential of an ended or missing session ended; then the hourly sweep (overdue approvals
//   expired, retention) and a budget check every few seconds (the wall clock moves without
//   events).
// - A start of a non-local model without a price is refused (`PRICING_UNKNOWN`); every start
//   answers the persistent warnings (`SANDBOX_DISABLED` until 7C, owner decision F2).
// - Every command runs in its session's queue, after the events and commands before it.
// - A session's provider instance is opened at its first run (with its resume reference) and
//   kept while the session is `idle`; interrupt, end and any provider exit close it.
// - The store and session credentials are Task 7's `AgentCore` (one per server, `AgentCoreTag`);
//   a start opens through its `openSession`, the outbox's credential effects go to
//   `core.credentials.handle`, and a resume refreshes the credential first. The journal is
//   Task 10's (`RuntimeJournal`); the built-in tools are 8b's (`ToolExecutor`).
import type { AgentWarning, Approval, ApprovalDecision, FinError, Principal, Run, RunTrigger, Session, WatchItem } from "@finisher/contracts";
import { Cause, Effect, Exit, Option, type Stream } from "effect";
import type { CallMeta } from "../diagnostics.ts";
import { finError, toFinError } from "../errors.ts";
import { AgentCoreTag } from "./core.ts";
import { openSession, registryBranchOps } from "./credentials.ts";
import { type RuntimeDeps, RuntimeEngine } from "./runtime-engine.ts";
import { budgetError, harnessUnknown, pricingUnknown, type StartRequest } from "./runtime-deps.ts";
import { call, closeLive, type Live } from "./runtime-live.ts";
import type { Recovery } from "./store.ts";
import { agentWarnings } from "./warnings.ts";

export type { RuntimeDeps } from "./runtime-engine.ts";

/** What an `allow` on a `budget` approval adds (Decision 21). */
export interface BudgetRaise {
  readonly tokens?: number;
  readonly moneyMicros?: number;
  readonly wallMs?: number;
}

const HOUR_MS = 3_600_000;
/** How often the wall-clock budget of a running session is checked. */
const BUDGET_CHECK_MS = 5_000;

const attempt = <A>(f: () => Promise<A>): Effect.Effect<A, FinError> => Effect.tryPromise({ try: f, catch: toFinError });

const principalRef = (p: Principal) => `${p.kind}:${p.id}`;

/** Runs `e`, rejecting with its `FinError` (not a fiber failure). */
async function exitOf<A>(e: Effect.Effect<A, FinError>): Promise<A> {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) return exit.value;
  const err = Cause.findErrorOption(exit.cause);
  throw Option.isSome(err) ? err.value : Cause.squash(exit.cause);
}

/** The runtime over the server's `AgentCore` (Task 12 builds it in `main.ts`, after the core). */
export const makeRuntime = (deps: Omit<RuntimeDeps, "core">): Effect.Effect<Runtime, never, AgentCoreTag> =>
  Effect.gen(function* () {
    const core = yield* AgentCoreTag;
    return new Runtime({ ...deps, core });
  });

export class Runtime extends RuntimeEngine {
  #timers: ReturnType<typeof setInterval>[] = [];
  #booted = false;
  #closed = false;

  constructor(deps: RuntimeDeps) {
    super(deps);
  }

  /**
   * At server start, before the transports listen: recovery (every live session becomes
   * `interrupted`, open calls get a synthetic result), the orphan sweep, the outbox drained;
   * then the timers.
   */
  async boot(): Promise<Recovery & { orphans: number; credentialsEnded: number }> {
    const recovery = this.store.recoverAfterCrash();
    const orphans = await this.store.sweepOrphans(0);
    await this.drain();
    const credentialsEnded = this.core.credentials.reconcile();
    this.#booted = true;
    if (this.deps.timers ?? true) {
      const hourly = setInterval(() => void this.sweep().catch(() => {}), HOUR_MS);
      const budgets = setInterval(() => this.checkBudgets(), BUDGET_CHECK_MS);
      hourly.unref?.();
      budgets.unref?.();
      this.#timers.push(hourly, budgets);
    }
    return { ...recovery, orphans, credentialsEnded };
  }

  /** Shutdown: the timers stop and every instance closes. The next `boot` recovers the rest. */
  async close(): Promise<void> {
    this.#closed = true;
    for (const t of this.#timers.splice(0)) clearInterval(t);
    const lives = [...this.lives.values()];
    this.lives.clear();
    await Promise.allSettled(lives.map((l) => closeLive(l)));
  }

  /** A crash, for tests: nothing more is written, nothing is cleaned up in the store. */
  async abandon(): Promise<void> {
    this.dead = true;
    await this.close();
  }

  /**
   * Hourly from `boot`: overdue approvals expired (they leave their sessions' open lists), then
   * the retention sweep (Decision 4). Answers how many sessions the retention removed.
   */
  sweep(now = this.clock.now()): Promise<number> {
    for (const s of this.store.expireOverdueApprovals(now)) {
      this.publishStatus(s, this.store.activeRun(s.id));
      this.#journalExpired(s);
    }
    return this.store.sweep(now, this.deps.retentionDays);
  }

  /** Charges the wall clock of running sessions and stops those over a budget. */
  checkBudgets(): void {
    for (const live of this.lives.values()) {
      if (live.runId === null) continue;
      this.background(live.sessionId, "a budget check", async () => {
        if (this.lives.get(live.sessionId) !== live || live.runId === null) return;
        await this.crossings(live, live.runId, this.budget(this.session(live.sessionId)).check());
      });
    }
  }

  stats(): { live: number; dropped: number } {
    return { live: this.lives.size, dropped: this.dropped };
  }

  #guard(): void {
    if (this.#closed) throw finError("SERVER_SHUTTING_DOWN", "the server is shutting down", {});
    if (!this.#booted) throw finError("SERVER_UNAVAILABLE", "the agent runtime has not recovered yet", { reason: "agentRuntimeBooting" });
  }

  // Commands --------------------------------------------------------------------------------

  /**
   * `agent.session.start` (Decision 15): step 1 here (role, mode, harness, preset, model), steps
   * 2–5 in Task 7's `openSession` (the root branch `agent.<role>-<n>` created as the starter,
   * whose actor and guard `meta` carries; the record; the credential; `idle`), then the first run.
   */
  start(req: StartRequest, by: Principal, meta: CallMeta): Effect.Effect<{ session: Session; run?: Run; warnings: AgentWarning[] }, FinError> {
    return attempt(async () => {
      this.#guard();
      const plan = await this.deps.planner.plan(req, by);
      if (this.adapter(plan.harness) === undefined) harnessUnknown(plan.harness);
      // A CLI harness whose sandbox cannot be applied here never starts (spec §8.9), nor one
      // the runtime has no sandboxes for (it would start its processes unconfined).
      if (this.deps.sandboxes !== undefined) this.deps.sandboxes.check(plan.harness);
      else if (plan.harness === "claudeCode" || plan.harness === "codex") throw finError("SANDBOX_UNAVAILABLE", "this runtime has no session sandbox for CLI harnesses", { profile: "agent", harness: plan.harness });
      // A money budget without a price would fail open: refused before anything is made.
      if (plan.provider !== "local" && !this.pricing.known(plan.provider, plan.model)) throw pricingUnknown(plan.provider, plan.model);
      const opened = await exitOf(
        openSession(
          { creds: this.core.credentials, store: this.store, ops: registryBranchOps(this.deps.registry, meta), ids: this.ids },
          {
            projectId: req.projectId,
            harness: plan.harness,
            provider: plan.provider,
            model: plan.model,
            weights: plan.weights,
            role: plan.role,
            mode: plan.mode,
            budget: plan.budget,
            createdBy: { principal: { kind: by.kind, id: by.id } },
            ...(req.base === undefined ? {} : { base: req.base }),
            ...(req.title === undefined ? {} : { title: req.title }),
          },
          by,
        ),
      ).catch(async (e: unknown) => {
        // A failed open ended what it recorded: its journal phases are owed all the same.
        await this.drain();
        throw e;
      });
      const id = opened.session.id;
      return this.serial(id, async () => {
        await this.drain();
        this.publishStatus(this.session(id));
        const run = req.prompt === undefined ? undefined : await this.promptIn(id, req.prompt, "prompt", req.attachments);
        return { session: this.session(id), ...(run === undefined ? {} : { run }), warnings: agentWarnings() };
      });
    });
  }

  /** `agent.session.prompt`: a run; `SESSION_BUSY` while one runs, `BUDGET_EXCEEDED` when nothing is left. */
  prompt(sessionId: string, text: string, _by: Principal, attachments?: ReadonlyArray<unknown>): Effect.Effect<Run, FinError> {
    return attempt(() => {
      this.#guard();
      return this.serial(sessionId, () => this.promptIn(sessionId, text, "prompt", attachments));
    });
  }

  protected async promptIn(sessionId: string, text: string, trigger: RunTrigger & ("prompt" | "steer"), attachments?: ReadonlyArray<unknown>): Promise<Run> {
    const s = this.session(sessionId);
    const b = this.budget(s);
    if (s.status === "idle") {
      const { stop } = b.check();
      if (stop) throw budgetError(stop);
    }
    const head = await this.#head(s.projectId, s.rootBranch ?? s.base.branch);
    const { run } = this.store.startRun(sessionId, { trigger, ...(head === undefined ? {} : { headAtStart: head }) });
    this.publishStatus(this.session(sessionId), run.id);
    b.startRun();
    await this.append({ sessionId, runId: run.id, kind: "userMessage", payload: { text, trigger, ...(attachments?.length ? { attachments } : {}) } });
    let live;
    try {
      live = await this.ensureLive(s);
    } catch (e) {
      await this.#failRun(sessionId, run.id, toFinError(e));
      throw e;
    }
    live.runId = run.id;
    await this.#sandboxNotices(live, run.id);
    const err = await call(live.provider.startTurn({ runId: run.id, text, facts: this.facts(this.session(sessionId)) }));
    if (err) {
      live.runId = null;
      await this.#failRun(sessionId, run.id, err);
      throw err;
    }
    return this.store.run(run.id)!;
  }

  protected sandboxFor(s: Session) {
    return this.deps.sandboxes?.open(s, (v) =>
      this.background(v.sessionId, "a sandbox violation", async () => {
        const runId = this.store.activeRun(v.sessionId);
        if (runId !== null) await this.append({ sessionId: v.sessionId, runId, kind: "notice", payload: { sandboxViolation: { op: v.op, profile: v.profile, rootKind: v.rootKind } } });
      }),
    );
  }

  /**
   * Once per opened instance of a sandboxed session: a `notice` per persistent warning of the
   * `agent` profile (F2: `SANDBOX_DISABLED`, spec §8.9), so a person sees it on the session.
   */
  async #sandboxNotices(live: Live, runId: string): Promise<void> {
    if (live.sandbox === undefined || live.noticed) return;
    live.noticed = true;
    for (const w of agentWarnings()) {
      await this.append({ sessionId: live.sessionId, runId, kind: "notice", payload: { sandbox: { code: w.code, ...w.details } } });
    }
  }

  /** A branch's head seq (by id), for `headAtStart`; undefined when it cannot be read. */
  async #head(projectId: string, branch: string): Promise<number | undefined> {
    const exit = await Effect.runPromiseExit(this.deps.registry.branches(projectId, false));
    return Exit.isSuccess(exit) ? exit.value.branches.find((b) => b.id === branch)?.head : undefined;
  }

  /** Task 7: the credential's expiry restarts before a resume; an ended one refuses it. */
  async #refresh(s: Session): Promise<void> {
    if (s.status !== "interrupted") return;
    const err = await call(this.core.credentials.refreshForSession(s));
    if (err) throw err;
  }

  /** The run could not start on its provider: an `error` node, and the session `interrupted`. */
  async #failRun(sessionId: string, runId: string, e: FinError): Promise<void> {
    await this.append({ sessionId, runId, kind: "error", payload: { code: e.code, message: e.message, details: e.details ?? {} } });
    await this.apply(sessionId, { kind: "interrupted", reason: e.code });
  }

  /**
   * `agent.session.steer` (Decision 6): into the running turn when the provider takes steers
   * (the built-in loop appends it before its next model call), else held for the next run
   * (`queued: true`). On an idle session it starts a run.
   */
  steer(sessionId: string, text: string, _by: Principal): Effect.Effect<{ queued: boolean }, FinError> {
    return attempt(() => {
      this.#guard();
      return this.serial(sessionId, async () => {
        const s = this.session(sessionId);
        const live = this.lives.get(sessionId);
        if ((s.status === "running" || s.status === "waitingApproval") && live?.runId) {
          if (!live.caps.steer) {
            live.held.push(text);
            return { queued: true };
          }
          await this.append({ sessionId, runId: live.runId, kind: "userMessage", payload: { text, trigger: "steer" } });
          const err = await call(live.provider.steer({ text }));
          if (err) throw err;
          return { queued: false };
        }
        await this.promptIn(sessionId, text, "steer");
        return { queued: false };
      });
    });
  }

  /** `agent.session.interrupt`: the run stops, open calls get a synthetic result, the branch stays. */
  interrupt(sessionId: string, _by: Principal): Effect.Effect<Session, FinError> {
    return attempt(() => {
      this.#guard();
      return this.serial(sessionId, async () => {
        const live = this.lives.get(sessionId);
        if (live) {
          live.runId = null;
          live.pendingStop = null;
          await call(live.provider.interrupt());
        }
        return (await this.apply(sessionId, { kind: "interrupted", reason: "userInterrupt" })).session;
      });
    });
  }

  /**
   * `agent.session.resume`: `interrupted` → `idle` with the credential's expiry refreshed; the
   * provider reopens with its resume reference at the next run, which `prompt` starts at once.
   */
  resume(sessionId: string, _by: Principal, prompt?: string): Effect.Effect<{ session: Session; run?: Run }, FinError> {
    return attempt(() => {
      this.#guard();
      return this.serial(sessionId, async () => {
        await this.#refresh(this.session(sessionId));
        await this.apply(sessionId, { kind: "resumed" });
        const run = prompt === undefined ? undefined : await this.promptIn(sessionId, prompt, "prompt");
        return { session: this.session(sessionId), ...(run === undefined ? {} : { run }) };
      });
    });
  }

  /** `agent.session.end`: the credential is revoked (outbox); the branch stays. */
  end(sessionId: string, _by: Principal): Effect.Effect<Session, FinError> {
    return attempt(() => {
      this.#guard();
      return this.serial(sessionId, async () => {
        const session = await this.#endIn(sessionId, "userEnded");
        this.forgetBudget(sessionId);
        return session;
      });
    });
  }

  async #endIn(sessionId: string, reason: string): Promise<Session> {
    const live = this.lives.get(sessionId);
    if (live) {
      live.runId = null;
      await call(live.provider.interrupt());
    }
    return (await this.apply(sessionId, { kind: "ended", reason })).session;
  }

  /** `agent.session.delete` (owner): ends it (journaled while the record exists), then removes it. */
  delete(sessionId: string, _by: Principal): Effect.Effect<void, FinError> {
    return attempt(() => {
      this.#guard();
      return this.serial(sessionId, async () => {
        const session = this.session(sessionId);
        if (session.status !== "ended") await this.#endIn(sessionId, "deleted");
        // Its unmerged transactions are abandoned (Task 11) before its product state goes.
        this.core.outcomes.observeSessionDeleted(session);
        await this.store.delete(sessionId);
        this.forgetBudget(sessionId);
        await this.drain();
      });
    });
  }

  /**
   * `agent.approval.answer` for a `budget` approval (Decision 21; console `agent approve`):
   * `allow` raises the limits by `add`, marks the approval used and resumes the session.
   */
  approve(approvalId: string, decision: ApprovalDecision, add: BudgetRaise, by: Principal): Effect.Effect<{ approval: Approval; session: Session }, FinError> {
    return attempt(async () => {
      this.#guard();
      const found = this.store.approval(approvalId) ?? (() => {
        throw finError("APPROVAL_NOT_FOUND", "no such approval", { id: approvalId });
      })();
      return this.serial(found.sessionId, async () => {
        let answered: { approval: Approval; session: Session };
        try {
          answered = this.store.answerApproval(approvalId, decision, principalRef(by));
        } catch (e) {
          // Past its deadline it expired instead (`APPROVAL_EXPIRED`): that is journaled.
          const s = (e as { code?: unknown }).code === "APPROVAL_EXPIRED" ? this.store.get(found.sessionId) : undefined;
          if (s !== undefined) this.#journalExpired(s);
          throw e;
        }
        const { approval, session } = answered;
        this.record("an approval", () => this.journal.approval({ session, approval }));
        this.publishStatus(session);
        if (approval.kind !== "budget" || decision !== "allow") return { approval, session };
        const b = this.budget(session);
        if (add.tokens) b.raise("tokens", add.tokens);
        if (add.moneyMicros) b.raise("money", add.moneyMicros);
        if (add.wallMs) b.raise("wallMs", add.wallMs);
        this.store.consumeApproval(approvalId);
        const now = this.session(session.id);
        if (now.status !== "interrupted") return { approval: this.store.approval(approvalId)!, session: now };
        await this.#refresh(now);
        const resumed = (await this.apply(now.id, { kind: "resumed" })).session;
        return { approval: this.store.approval(approvalId)!, session: resumed };
      });
    });
  }

  /** Journals the session's expired approvals (the journal records each once). */
  #journalExpired(s: Session): void {
    for (const approval of this.store.approvals(s.id)) {
      if (approval.status === "expired") this.record("an approval", () => this.journal.approval({ session: s, approval }));
    }
  }

  /** `agent.watch`: the stored nodes from `fromSeq`, the status, then live items until `ended`. */
  watch(sessionId: string, fromSeq?: number): Stream.Stream<WatchItem, FinError> {
    return this.hub.watch(this.store, sessionId, fromSeq, this.deps.watchBufferSize);
  }
}
