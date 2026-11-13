// The agent runtime (sub-project 7 §3; plan 7A Task 6b, Decisions 2, 3, 5, 6): sessions,
// runs and their provider instances over `AgentStore`. The commands an RPC handler or the
// console calls; `runtime-engine.ts` holds the event handling, budgets and the outbox.
//
// - `boot()` first (Decision 3, the Task 3b order): crash recovery (reservations returned), the
//   orphan sweep at grace 0 before any session work, the outbox drained, every live session
//   credential of an ended or missing session ended, overdue approvals expired; then the hourly
//   sweep (overdue approvals expired, retention) and a budget check every few seconds (the wall
//   clock moves without events). A provider card also expires at its own `expiresAt`
//   (`provider-requests.ts`).
// - A start of a non-local model without a price is refused (`PRICING_UNKNOWN`); every start
//   answers the persistent warnings (`SANDBOX_DISABLED` until 7C, owner decision F2).
// - Every command runs in its session's queue, after the events and commands before it.
// - Children (7B Task 6, `subagents.ts`): `spawnChild` starts one for a parent session,
//   `waitChildren` waits on them; after every transition the subagents see it (a brief's end,
//   the parent's `subagent` node, the cascade; a root's end waits for a descendant's calls in
//   flight, `settled`) and the live limits (`live-limits.ts`) admit runs and start their turns.
// - A session's provider instance is opened at its first run (with its resume reference) and
//   kept while the session is `idle`; interrupt, end and any provider exit close it.
// - The store and session credentials are Task 7's `AgentCore` (one per server, `AgentCoreTag`);
//   a start opens through its `openSession`, the outbox's credential effects go to
//   `core.credentials.handle`, and a resume refreshes the credential first. The journal is
//   Task 10's (`RuntimeJournal`); the built-in tools are 8b's (`ToolExecutor`).
import type { AgentWarning, Approval, ApprovalDecision, ChildResult, FinError, Principal, Run, RunTrigger, Session, WatchItem } from "@finisher/contracts";
import { Cause, Effect, Exit, Option, type Stream } from "effect";
import type { CallMeta } from "../diagnostics.ts";
import { finError, toFinError } from "../errors.ts";
import { DEFAULT_BUDGET } from "./budget.ts";
import { childCarve } from "./carve.ts";
import { AgentCoreTag } from "./core.ts";
import { openSession, registryBranchOps } from "./credentials.ts";
import { type RuntimeDeps, RuntimeEngine, STOP_GRACE_MS } from "./runtime-engine.ts";
import { budgetError, harnessUnknown, pricingUnknown, type StartRequest } from "./runtime-deps.ts";
import { call, closeLive, type Live } from "./runtime-live.ts";
import { LiveLimits } from "./live-limits.ts";
import { CRITIC_ROLE } from "./outcomes.ts";
import { BUNDLED_ROLES } from "./roles.ts";
import { DEFAULT_LIMITS } from "./settings.ts";
import type { Applied, Recovery } from "./store.ts";
import { type ChildOpen, Subagents } from "./subagents.ts";
import type { ConversationOps, RollbackResult, Rollbacks } from "./rollback.ts";
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
/** The runtime's deps: the engine's, and the rollbacks (7B Task 8, `rollback.ts`). */
type Deps = RuntimeDeps & { readonly rollbacks?: Rollbacks };

export const makeRuntime = (deps: Omit<Deps, "core">): Effect.Effect<Runtime, never, AgentCoreTag> =>
  Effect.gen(function* () {
    const core = yield* AgentCoreTag;
    return new Runtime({ ...deps, core });
  });

export class Runtime extends RuntimeEngine {
  #timers: ReturnType<typeof setInterval>[] = [];
  #booted = false;
  #closed = false;
  readonly #limits: LiveLimits;
  readonly subagents: Subagents;
  readonly #rollbacks: Rollbacks | undefined;

  constructor(deps: Deps) {
    super(deps);
    this.#rollbacks = deps.rollbacks;
    const limits = () => ({ ...DEFAULT_LIMITS, ...deps.limits });
    this.#limits = new LiveLimits({
      max: () => ({ builtin: limits().maxLiveBuiltin, provider: limits().maxLiveProviders }),
      active: () => this.store.tree.active(),
      current: (id, runId) => this.store.get(id)?.status === "running" && this.store.activeRun(id) === runId,
      turns: {
        session: (id) => this.session(id),
        open: (s) => this.ensureLive(s),
        started: (live, runId) => this.#sandboxNotices(live, runId),
        startClock: (s) => this.budget(s).startRun(),
        facts: (s) => this.facts(s),
        append: async (n) => void (await this.append(n)),
        interrupt: async (id, reason) => void (await this.apply(id, { kind: "interrupted", reason })),
      },
    });
    const sub = { store: this.store, roles: deps.roles ?? BUNDLED_ROLES, limits, live: this.#limits, clock: this.clock, graceMs: deps.stopGraceMs ?? STOP_GRACE_MS };
    this.subagents = new Subagents({ ...sub, ...(deps.afterChildOpened === undefined ? {} : { afterOpened: deps.afterChildOpened }) });
    this.subagents.bind({
      background: (id, what, f) => this.background(id, what, f),
      inFlight: (id) => this.lives.get(id)?.calls.size ?? 0,
      endIn: async (id, reason) => {
        if (this.store.get(id)?.status !== "ended") await this.#endIn(id, reason);
        this.forgetBudget(id);
      },
      interruptIn: async (id, reason) => {
        const st = this.store.get(id)?.status;
        if (st !== undefined && st !== "interrupted" && st !== "ended") await this.#interruptIn(id, reason);
      },
      append: async (n) => void (await this.append(n)),
      startQueued: (q) => this.background(q.sessionId, "a queued run", () => this.#limits.begin(q).catch(() => {})),
    });
    // A provider card is settled at its `expiresAt`, not only by the hourly sweep (7B Task 5).
    deps.requests?.bind({ clock: this.clock, expire: () => this.expireApprovals() });
    // A session's review request is a `notice` in its active run (7B Task 11); the run goes on.
    this.core.reviewLink.onBranchReady(({ sessionId, branch, headSeq, reviewId }) => {
      const runId = reviewId === undefined ? null : this.store.activeRun(sessionId);
      if (runId !== null) this.background(sessionId, "a review notice", () => this.append({ sessionId, runId, kind: "notice", payload: { review: { itemId: reviewId, branch, headSeq } } }).then(() => {}));
    });
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
    // Cards already overdue (a budget card of an interrupted session) expire now, not within the hour.
    this.expireApprovals();
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
    this.subagents.close();
    this.deps.requests?.close();
    const lives = [...this.lives.values()];
    this.lives.clear();
    await Promise.allSettled(lives.map((l) => closeLive(l, (m, d) => this.log(m, d))));
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
    this.expireApprovals(now);
    return this.store.sweep(now, this.deps.retentionDays);
  }

  /** Overdue approvals expired: their sessions' statuses, journal, and provider requests (a `deny`). */
  expireApprovals(now = this.clock.now()): void {
    if (this.#closed) return;
    for (const s of this.store.expireOverdueApprovals(now)) {
      this.publishStatus(s, this.store.activeRun(s.id));
      this.#journalExpired(s);
      this.background(s.id, "a provider request's expiry", () => this.deps.requests?.settleSession(s.id, (id) => this.store.approval(id)) ?? Promise.resolve());
    }
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
  start(req: StartRequest, by: Principal, meta: CallMeta, parent?: Session, trigger?: "savedPrompt"): Effect.Effect<{ session: Session; run?: Run; warnings: AgentWarning[] }, FinError> {
    return attempt(async () => {
      this.#guard();
      const plan = await this.deps.planner.plan(req, by, parent);
      const child: ChildOpen | undefined = parent === undefined ? undefined : this.subagents.childOf(parent);
      // A child's budget is carved from its parent's remaining (7B Decision 22, `carve.ts`); the
      // parent's `Budget` puts its limits in the ledger first. Re-checked as the child is recorded.
      if (parent !== undefined) this.budget(parent);
      const carved = parent === undefined ? undefined : childCarve(this.store.ledger(), parent.id, plan.budget, req.budget, this.deps.defaults ?? DEFAULT_BUDGET);
      if (this.adapter(plan.harness) === undefined) harnessUnknown(plan.harness);
      // A CLI harness whose sandbox cannot be applied here never starts (spec §8.9), nor one
      // the runtime has no sandboxes for (it would start its processes unconfined).
      if (this.deps.sandboxes !== undefined) this.deps.sandboxes.check(plan.harness);
      else if (plan.harness === "claudeCode" || plan.harness === "codex") throw finError("SANDBOX_UNAVAILABLE", "this runtime has no session sandbox for CLI harnesses", { profile: "agent", harness: plan.harness });
      // A money budget without a price would fail open: refused before anything is made.
      if (plan.provider !== "local" && !this.pricing.known(plan.provider, plan.model)) throw pricingUnknown(plan.provider, plan.model);
      // A critic on a branch under review works on a child branch of it (7B Decision 26, `review-link.ts`).
      const under = child === undefined && plan.role === CRITIC_ROLE && req.base !== undefined ? await exitOf(this.core.reviewLink.criticUnder(req.projectId, req.base)) : undefined;
      const link = this.core.reviewLink;
      const ops = registryBranchOps(this.deps.registry, meta, {
        archived: (p, b) => link.archived(p, b, principalRef(by)),
        underReview: (p, b) => link.underReview(p, b),
        reviewedHead: (p, b) => link.reviewedHead(p, b),
      });
      const opened = await exitOf(
        openSession(
          { creds: this.core.credentials, store: this.store, ops, ids: this.ids },
          {
            projectId: req.projectId,
            harness: plan.harness,
            provider: plan.provider,
            model: plan.model,
            weights: plan.weights,
            role: plan.role,
            mode: plan.mode,
            budget: carved?.budget ?? plan.budget,
            ...(carved === undefined ? {} : { carve: carved.carve }),
            createdBy: { principal: { kind: by.kind, id: by.id } },
            ...(child === undefined ? {} : child),
            ...(req.base === undefined || child !== undefined ? {} : { base: req.base }),
            ...(under === undefined ? {} : { under }),
            ...(req.title === undefined ? {} : { title: req.title }),
            ...(trigger === undefined ? {} : { trigger }),
            ...(plan.tools === undefined ? {} : { tools: plan.tools }),
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
        // A parent that stopped while the child was opened stops it, and the spawn fails.
        if (child !== undefined) await this.subagents.adopt(child.parent, id);
        const run = req.prompt === undefined ? undefined : await this.promptIn(id, req.prompt, "prompt", req.attachments);
        return { session: this.session(id), ...(run === undefined ? {} : { run }), warnings: agentWarnings() };
      });
    });
  }

  /**
   * `agent.session.start` from a session (7B Decision 16): a child of `parent` (`subagents.ts`),
   * its brief its first run; spawns under one root session one at a time.
   */
  spawnChild(parent: Session, req: StartRequest, by: Principal, meta: CallMeta): Effect.Effect<{ session: Session; run?: Run; warnings: AgentWarning[] }, FinError> {
    return attempt(() => this.subagents.serialSpawn(parent, () => exitOf(this.start(req, by, meta, this.session(parent.id)))));
  }

  /** `agent.session.wait` (7B Decision 21): `WAIT_TIMEOUT` when not done in time. */
  waitChildren(by: Principal, ids: ReadonlyArray<string>, any: boolean, timeoutMs: number): Effect.Effect<ChildResult[], FinError> {
    return Effect.tryPromise({ try: (signal) => this.subagents.wait(by, ids, any, timeoutMs, signal), catch: toFinError });
  }

  /**
   * `agent.session.rollback` (7B Decision 23, `rollback.ts`): in the session's queue, so no run
   * starts meanwhile; `meta` is the caller's actor and the guard of the session's reach.
   */
  rollback(sessionId: string, toRun: number, by: Principal, meta: CallMeta): Effect.Effect<RollbackResult & { session: Session }, FinError> {
    return attempt(() => {
      this.#guard();
      const rb = this.#rollbacks;
      if (rb === undefined) throw finError("SERVER_UNAVAILABLE", "rollback is not wired on this server", { reason: "noRollback" });
      return this.serial(sessionId, async () => {
        const ops: ConversationOps = {
          caps: async (s) => this.lives.get(s.id)?.caps ?? (await exitOf((this.adapter(s.harness) ?? harnessUnknown(s.harness)).detect())).capabilities,
          live: (id) => this.lives.get(id)?.provider,
          open: async (s) => (await this.ensureLive(s)).provider,
          close: (id) => this.stopLive(id),
          append: async (n) => void (await this.append(n)),
        };
        const r = await rb.rollback(this.session(sessionId), toRun, by, meta, ops);
        return { ...r, session: this.session(sessionId) };
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
    await this.append({ sessionId, runId: run.id, kind: "userMessage", payload: { text, trigger, ...(attachments?.length ? { attachments } : {}) } });
    // Its turn, or over the server's live limit a wait for a slot (7B Decision 19, `live-limits.ts`).
    await this.#limits.enter(s, run.id, text);
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
        // A run waiting for a slot takes the steer into its first turn.
        if (await this.#limits.hold(sessionId, text)) return { queued: true };
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
      return this.serial(sessionId, () => this.#interruptIn(sessionId, "userInterrupt"));
    });
  }

  async #interruptIn(sessionId: string, reason: string): Promise<Session> {
    const live = this.lives.get(sessionId);
    if (live) {
      live.runId = null;
      live.pendingStop = null;
      await call(live.provider.interrupt());
    }
    return (await this.apply(sessionId, { kind: "interrupted", reason })).session;
  }

  /** The calls in flight have their results: a root's end waiting for them goes on (7B Decision 20), else a budget stop. */
  protected override async settled(live: Live): Promise<void> {
    if (!(await this.subagents.settled(live.sessionId))) await super.settled(live);
  }

  /** Every transition: then the children and the live limits see it (7B Task 6), and an end the review link (Task 11). */
  protected override async after(sessionId: string, applied: Applied): Promise<void> {
    await super.after(sessionId, applied);
    this.subagents.observe(applied);
    if (applied.effects.some((e) => e.kind === "journal" && e.phase === "ended")) this.core.reviewLink.sessionEnded(applied.session);
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
   * `agent.approval.answer` (Decision 21; console `agent approve`), by kind: a `budget` `allow`
   * raises the limits by `add`, marks the approval used and resumes the session; a `provider`
   * answer goes back to the waiting request; a `rule` answer is recorded (its grant, 7B).
   */
  approve(approvalId: string, decision: ApprovalDecision, add: BudgetRaise, by: Principal): Effect.Effect<{ approval: Approval; session: Session }, FinError> {
    return attempt(async () => {
      this.#guard();
      const found = this.store.approval(approvalId) ?? (() => {
        throw finError("APPROVAL_NOT_FOUND", "no such approval", { id: approvalId });
      })();
      return this.serial(found.sessionId, async () => {
        const raising = found.kind === "budget" && decision === "allow";
        // The session's `Budget` puts its limits in the ledger first, outside the transaction below.
        const b = raising ? this.budget(this.session(found.sessionId)) : undefined;
        // The answer and every raise are one transaction (7B Decision 22): a child's raise past
        // its parent's remaining, or any failure, answers nothing and raises nothing.
        const out = this.store.ledger().tx(() => {
          let answered: { approval: Approval; session: Session };
          try {
            answered = this.store.answerApproval(approvalId, decision, principalRef(by));
          } catch (e) {
            // Past its deadline it expired instead (`APPROVAL_EXPIRED`): kept, and journaled below.
            if ((e as { code?: unknown }).code === "APPROVAL_EXPIRED") return { expired: e };
            throw e;
          }
          if (b !== undefined) {
            b.raiseAll({ tokens: add.tokens, money: add.moneyMicros, wallMs: add.wallMs });
            this.store.consumeApproval(approvalId);
          }
          return { answered };
        });
        if ("expired" in out) {
          const s = this.store.get(found.sessionId);
          if (s !== undefined) this.#journalExpired(s);
          throw out.expired;
        }
        const { approval, session } = out.answered;
        this.record("an approval", () => this.journal.approval({ session, approval }));
        this.publishStatus(session);
        // By kind (7B Task 5): a provider's request hears the answer; a rule's is a one-time grant.
        if (approval.kind === "provider") await this.deps.requests?.settle(approval);
        if (!raising) return { approval: this.store.approval(approvalId)!, session };
        const now = this.session(session.id);
        if (now.status !== "interrupted") return { approval: this.store.approval(approvalId)!, session: now };
        await this.#refresh(now);
        const resumed = (await this.apply(now.id, { kind: "resumed" })).session;
        return { approval: this.store.approval(approvalId)!, session: resumed };
      });
    });
  }

  /** A card the chain opened (7B Task 4, `approvals.ts`): its `approval` node in the active run, then the watchers. */
  async approvalOpened(s: Session, a: Approval, node: Record<string, unknown>): Promise<void> {
    const runId = this.store.activeRun(s.id);
    if (runId !== null) await this.append({ sessionId: s.id, runId, kind: "approval", payload: node });
    this.publish(s.id, { type: "approvalOpened", approval: a });
    this.publishStatus(s, runId);
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
