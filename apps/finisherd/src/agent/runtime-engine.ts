// The runtime engine's core (plan 7A Task 6b; Decisions 2, 3, 5, 6, 21): per-session order,
// provider events into nodes, budgets and their safe-point stop, the outbox, live instances.
// `runtime.ts` adds the commands on top.
//
// - **Order:** everything that touches a session (its provider's events, RPC commands, tool
//   results, budget timers) runs through one promise queue per session, so the nodes, the
//   ledger and the journal follow the provider's event order and commands never interleave.
// - **Binding:** an event whose instance is not the session's current one is dropped (a
//   replaced or exited process; arch §27.5). An exited instance keeps its pump until its
//   stream ends, so its late events arrive here and are dropped, never stored.
// - **Tools:** a `toolStarted` gets a Finisher call id (a ULID: `FIN_CALL`) and its `toolCall`
//   node before anything runs; a harness whose tools the runtime runs (built-in, fake) gets the
//   result back through `respond`, and the `toolResult` node is written from the adapter's
//   `toolCompleted`. Nothing re-runs a call: recovery gives it a synthetic result (store).
// - **Budgets:** charged from `usage` and `toolCompleted` (a non-local call that reports no cost
//   and has no price is charged its projection, never 0); a stop waits for the calls in flight
//   (the safe point) for at most `stopGraceMs` (30 s), then aborts them, interrupts the session
//   with `BUDGET_EXCEEDED` and opens a `budget` approval.
// - **The hooks' tap** (7F Decision 18): every published item and 80 % notice, watched or not.
import type { Approval, BudgetState, FinError, Harness, ProviderEvent, Run, Session, Usage, WatchItem } from "@finisher/contracts";
import { Effect } from "effect";
import { finError, toFinError } from "../errors.ts";
import type { BoundEvent, CallGate, Capabilities, ProviderAdapter } from "./adapter.ts";
import { Budget, type Crossings, DEFAULT_BUDGET, type Reading } from "./budget.ts";
import type { Clock, Ids } from "./clock.ts";
import type { AgentCore } from "./core.ts";
import { credentialPath } from "./credentials.ts";
import type { Pricing, RuntimeDeps, RuntimeJournal, ToolExecutor } from "./runtime-deps.ts";
import { budgetError, errorShape, harnessUnknown, limitsOf, NO_JOURNAL, parseRef, REPORTED_PRICING, runFacts, toolsOf } from "./runtime-deps.ts";
import { call, closeLive, closeScope, type Live, openLive } from "./runtime-live.ts";
import { OutboxDrainer } from "./runtime-outbox.ts";
import { handleRequest } from "./provider-requests.ts";
import type { SessionSandboxes } from "./sandbox.ts";
import type { SessionEvent } from "./session.ts";
import type { AgentStore, Appended, Applied, NewNode } from "./store.ts";
import { sessionNotFound } from "./store-rows.ts";
import { WatchHub } from "./watch-hub.ts";

/** What the engine is built from (`runtime-deps.ts`). */
export type { RuntimeDeps } from "./runtime-deps.ts";

/** A provider failure that ends only the run; the session stays `idle` (Decision 10). */
const RUN_ONLY_STOPS = new Set(["MODEL_CONTEXT_EXCEEDED"]);
const DAY_MS = 86_400_000;
export const STOP_GRACE_MS = 30_000;

/** Thrown into work queued after `abandon` (a simulated crash): nothing more is written. */
class Abandoned extends Error {}

export abstract class RuntimeEngine {
  protected readonly store: AgentStore;
  protected readonly core: AgentCore;
  protected readonly clock: Clock;
  protected readonly ids: Ids;
  protected readonly deps: RuntimeDeps;
  protected readonly journal: RuntimeJournal;
  protected readonly pricing: Pricing;
  protected readonly hub = new WatchHub();
  protected readonly lives = new Map<string, Live>();
  readonly #adapters = new Map<Harness, ProviderAdapter>();
  readonly #caps = new Map<Harness, Promise<Capabilities>>();
  readonly #budgets = new Map<string, Budget>();
  readonly #queues = new Map<string, Promise<unknown>>();
  readonly #outbox: OutboxDrainer;
  protected dead = false;
  protected dropped = 0;

  constructor(deps: RuntimeDeps) {
    this.deps = deps;
    this.core = deps.core;
    this.store = deps.core.store;
    this.clock = deps.core.clock;
    this.ids = deps.core.ids;
    this.journal = deps.journal ?? NO_JOURNAL;
    this.pricing = deps.pricing ?? REPORTED_PRICING;
    for (const a of deps.adapters) this.#adapters.set(a.harness, a);
    this.#outbox = new OutboxDrainer({
      store: this.store,
      credentials: this.core.credentials,
      journal: this.journal,
      dead: () => this.dead,
      log: (m, d) => this.log(m, d),
    });
  }

  protected log(message: string, details: Record<string, unknown>): void {
    this.deps.log?.(message, details);
  }

  /** A journal record is best effort: a failure is reported, never the session's. */
  protected record(what: string, f: () => void): void {
    try {
      f();
    } catch (e) {
      this.log(`agent runtime: the journal refused ${what}`, { error: toFinError(e).code });
    }
  }

  protected adapter(harness: Harness): ProviderAdapter | undefined {
    return this.#adapters.get(harness);
  }

  protected session(sessionId: string): Session {
    return this.store.get(sessionId) ?? sessionNotFound(sessionId);
  }

  /** Runs `f` after every earlier task of the session: the session's total order. */
  protected serial<A>(sessionId: string, f: () => Promise<A>): Promise<A> {
    const prev = this.#queues.get(sessionId) ?? Promise.resolve();
    const done = prev.then(() => {
      if (this.dead) throw new Abandoned("the runtime is gone");
      return f();
    });
    const tail = done.then(
      () => {},
      () => {},
    );
    this.#queues.set(sessionId, tail);
    void tail.then(() => {
      if (this.#queues.get(sessionId) === tail) this.#queues.delete(sessionId);
    });
    return done;
  }

  /** Queues `f` and reports a failure instead of returning it (provider events, timers). */
  protected background(sessionId: string, what: string, f: () => Promise<void>): void {
    this.serial(sessionId, f).catch((e: unknown) => {
      if (!(e instanceof Abandoned)) this.log(`agent runtime: ${what} failed`, { sessionId, error: toFinError(e).code });
    });
  }

  protected budget(s: Session): Budget {
    let b = this.#budgets.get(s.id);
    if (b === undefined) {
      b = new Budget(this.store.ledger(), s.id, limitsOf(s.budget, this.deps.defaults ?? DEFAULT_BUDGET), this.clock, {
        price: (u) => this.pricing.projected(s, u),
        cacheReadWeight: this.pricing.cacheReadWeight(s),
      });
      this.#budgets.set(s.id, b);
    }
    return b;
  }

  /** The first budget `s` has used up (`Budget.spent`, with this runtime's defaults), or null (7B Task 12). */
  budgetSpent(s: Session): Reading | null {
    return (this.#budgets.get(s.id) ?? new Budget(this.store.ledger(), s.id, limitsOf(s.budget, this.deps.defaults ?? DEFAULT_BUDGET), this.clock)).spent();
  }

  protected forgetBudget(sessionId: string): void {
    this.#budgets.delete(sessionId);
  }

  protected budgetState(b: Budget): BudgetState {
    const r = new Map(b.readings().map((x) => [x.kind, x]));
    const pair = (k: "tokens" | "money" | "wallMs") => ({ limit: r.get(k)!.limit, used: r.get(k)!.used });
    return { tokens: pair("tokens"), moneyMicros: pair("money"), wallMs: pair("wallMs") };
  }

  protected facts(s: Session): string {
    return runFacts(s, this.budget(s).readings());
  }

  // Writes, then the watchers ---------------------------------------------------------------

  protected async append(n: NewNode): Promise<Appended> {
    const a = await this.store.appendNode(n);
    this.flush(n.sessionId);
    return a;
  }

  /** Publishes the session's nodes written since the last publish (synthetic ones included). */
  protected flush(sessionId: string): void {
    if (!this.hub.watched(sessionId)) return;
    let from = (this.hub.published(sessionId) ?? this.store.lastSeq(sessionId)) + 1;
    for (;;) {
      const page = this.store.nodes(sessionId, from, 1000);
      for (const node of page) this.hub.publish(sessionId, { type: "nodeAdded", node });
      if (page.length === 0) break;
      from = page[page.length - 1]!.seq + 1;
      this.hub.setPublished(sessionId, from - 1);
      if (page.length < 1000) break;
    }
  }

  protected publish(sessionId: string, item: WatchItem): void {
    this.deps.sessionEvents?.published(sessionId, item);
    this.hub.publish(sessionId, item);
  }

  protected publishStatus(s: Session, runId?: string | null): void {
    this.publish(s.id, {
      type: "statusChanged",
      status: s.status,
      ...(s.statusReason === undefined ? {} : { reason: s.statusReason }),
      ...(runId ? { runId } : {}),
    });
  }

  /** Feeds the state machine, then does what its effects ask outside `control.db`. */
  protected async apply(sessionId: string, e: SessionEvent): Promise<Applied> {
    const applied = this.store.apply(sessionId, e);
    await this.after(sessionId, applied);
    return applied;
  }

  protected async after(sessionId: string, applied: Applied): Promise<void> {
    for (const effect of applied.effects) {
      if (effect.kind === "closeRun") this.#budgets.get(sessionId)?.endRun();
    }
    if (applied.effects.some((e) => e.kind === "stopProvider")) await this.stopLive(sessionId);
    this.flush(sessionId);
    this.publishStatus(applied.session, this.store.activeRun(sessionId));
    await this.drain();
  }

  // The outbox ------------------------------------------------------------------------------

  /** Delivers every queued effect, oldest first; one that fails holds its session's later ones. */
  protected drain(): Promise<void> {
    return this.#outbox.drain();
  }

  // Live instances --------------------------------------------------------------------------

  #capsOf(adapter: ProviderAdapter): Promise<Capabilities> {
    let c = this.#caps.get(adapter.harness);
    if (c === undefined) {
      c = Effect.runPromise(adapter.detect()).then((d) => d.capabilities);
      this.#caps.set(adapter.harness, c);
      c.catch(() => this.#caps.delete(adapter.harness));
    }
    return c;
  }

  /** The session's open instance, opened (with its resume reference) when there is none. */
  protected async ensureLive(s: Session): Promise<Live> {
    const open = this.lives.get(s.id);
    if (open) return open;
    const adapter = this.adapter(s.harness) ?? harnessUnknown(s.harness);
    const env = await this.deps.environment.prepare(s);
    // The session's own credential, never the starter's (Decision 14b).
    const liveEnv = { ...env, env: { ...env.env, FIN_CREDENTIAL: credentialPath(this.store.root, s.id) } };
    const caps = await this.#capsOf(adapter);
    const holder: { live?: Live } = {};
    const live = await openLive(
      adapter,
      {
        sessionId: s.id,
        projectId: s.projectId,
        provider: s.provider,
        model: s.model,
        role: s.role,
        mode: s.mode,
        system: env.system,
        cwd: env.cwd,
        env: liveEnv.env,
        providerRef: parseRef(this.store.providerRef(s.id)),
        ...toolsOf(this.store.tools(s.id)),
        gate: this.#gate(s.id, holder),
      },
      liveEnv,
      caps,
      (b) => this.#onEvent(s.id, b),
      (l) => this.#onEnd(l),
      this.sandboxFor(s),
    );
    holder.live = live;
    this.lives.set(s.id, live);
    const now = live.provider.ref();
    if (now !== null) this.store.setProviderRef(s.id, JSON.stringify(now));
    return live;
  }

  /** The session sandbox a CLI harness's instance opens in its scope (none for the built-in agent). */
  protected abstract sandboxFor(s: Session): ReturnType<SessionSandboxes["open"]>;

  /** Ends the session's instance (`stopProvider`); a tool still running finishes, unheard. */
  protected async stopLive(sessionId: string): Promise<void> {
    const live = this.lives.get(sessionId);
    if (!live) return;
    this.lives.delete(sessionId);
    this.#release(live);
    await closeLive(live);
  }

  #release(live: Live): void {
    const s = this.store.get(live.sessionId);
    if (s === undefined) return;
    for (const r of live.reservations.splice(0)) this.budget(s).release(r);
  }

  #onEvent(sessionId: string, b: BoundEvent): void {
    if (this.dead) return;
    this.background(sessionId, `the ${b.event.type} event`, () => this.#handle(sessionId, b));
  }

  #onEnd(live: Live): void {
    if (this.dead) return;
    this.background(live.sessionId, "an instance's end", async () => {
      // A stream that ended without `exited` is an instance that is gone all the same.
      if (this.lives.get(live.sessionId) === live) await this.#exited(live, { code: null, signal: null });
      if (!live.closed) await closeScope(live);
    });
  }

  /** The gate of one instance: a replaced instance's late calls are refused, never charged. */
  #gate(sessionId: string, holder: { live?: Live }): CallGate {
    const current = () => (holder.live !== undefined && this.lives.get(sessionId) === holder.live ? holder.live : undefined);
    return {
      beforeCall: (projected) => {
        const live = current();
        const s = this.store.get(sessionId);
        if (live === undefined || s === undefined) {
          return { ok: false, error: finError("SESSION_INTERRUPTED", "this provider instance is no longer the session's", { sessionId }) };
        }
        const a = this.budget(s).canCall(projected);
        if (a.ok) {
          live.reservations.push(a.reservation);
          return { ok: true };
        }
        // A per-run cap ends the run only; anything else stops the session.
        if (a.kind !== "modelCalls") this.background(sessionId, "a budget stop", () => this.requestStop(live, a));
        return { ok: false, error: budgetError(a) };
      },
      release: () => {
        const live = current();
        const s = this.store.get(sessionId);
        if (live && s) this.budget(s).release(live.reservations.shift());
      },
    };
  }

  // Events ----------------------------------------------------------------------------------

  async #handle(sessionId: string, { instance, event: e }: BoundEvent): Promise<void> {
    const live = this.lives.get(sessionId);
    if (live === undefined || live.instance !== instance) {
      this.dropped++;
      return;
    }
    if (e.type === "exited") return this.#exited(live, e);
    const runId = live.runId;
    if (runId === null) {
      // Nothing is running on this instance: a turn's tail after an interrupt or a stop.
      if (e.type !== "turnStarted") this.dropped++;
      return;
    }
    await this.#onRunEvent(live, runId, e);
  }

  async #onRunEvent(live: Live, runId: string, e: Exclude<ProviderEvent, { type: "exited" }>): Promise<void> {
    const sessionId = live.sessionId;
    const node = (kind: NewNode["kind"], payload: unknown) => this.append({ sessionId, runId, kind, payload });
    switch (e.type) {
      case "turnStarted":
        return;
      case "messageDelta":
        return this.publish(sessionId, { type: "delta", runId, kind: "message", text: e.text });
      case "reasoningDelta":
        return this.publish(sessionId, { type: "delta", runId, kind: "reasoning", text: e.text });
      case "toolOutputDelta": {
        const c = live.calls.get(e.providerId);
        return this.publish(sessionId, { type: "delta", runId, kind: "toolOutput", text: e.text, ...(c ? { callId: c.callId } : {}) });
      }
      case "message":
        return void (await node(e.role === "assistant" ? "assistantMessage" : "userMessage", { role: e.role, blocks: e.blocks }));
      case "reasoning": {
        const { type: _, ...payload } = e;
        return void (await node("reasoning", payload));
      }
      case "plan":
        return void (await node("plan", { steps: e.steps }));
      case "toolStarted":
        return this.#toolStarted(live, runId, e);
      case "toolCompleted":
        return this.#toolCompleted(live, e);
      case "request":
        // Through the provider permissions (7B Task 5, `provider-requests.ts`).
        return handleRequest(this.deps.requests, {
          session: this.session(sessionId),
          runId,
          node: async (payload) => void (await node("approval", payload)),
          respond: (a) => call(live.provider.respond(e.id, a)),
          log: (m, d) => this.log(m, { sessionId, ...d }),
        }, e);
      case "usage":
        return this.#usage(live, runId, e);
      case "turnCompleted":
        return this.runEnded(live, runId, e.stopReason);
      case "turnFailed":
        return this.#turnFailed(live, runId, finError(e.error.code, e.error.message, e.error.details, e.error.fix));
      case "capture":
        return this.record("a capture", () => this.journal.capture({ session: this.session(sessionId), runId, request: e.request, response: e.response }));
      case "notice":
        return void (await node("notice", { [e.name]: e.details ?? true }));
      case "summary":
        return void (await node("summary", { prompt: e.prompt, text: e.text }));
      case "unknown":
        return void (await node("notice", { unknownEvent: e.name }));
    }
  }

  async #toolStarted(live: Live, runId: string, e: Extract<ProviderEvent, { type: "toolStarted" }>): Promise<void> {
    const sessionId = live.sessionId;
    if (live.calls.has(e.providerId)) {
      await this.append({ sessionId, runId, kind: "notice", payload: { duplicateToolCall: e.providerId } });
      return;
    }
    const callId = this.ids.ulid();
    // The call is announced (its node and id stored) before anything runs it.
    const { inserted } = await this.append({ sessionId, runId, kind: "toolCall", providerId: e.providerId, callId, payload: { tool: e.tool, input: e.input } });
    if (!inserted) return;
    live.calls.set(e.providerId, { callId, runId });
    const s = this.session(sessionId);
    this.record("a tool call", () => this.journal.toolCall({ session: s, runId, callId, providerId: e.providerId, tool: e.tool, input: e.input }));
    const exec = this.deps.tools?.[s.harness];
    if (exec === undefined) return; // the provider runs its own tools (a CLI)
    const cap = this.budget(s).canTool();
    const refusal = live.pendingStop ? budgetError(live.pendingStop) : cap.ok ? null : budgetError(cap);
    if (refusal) {
      const err = await call(live.provider.respond(e.providerId, { kind: "toolResult", ok: false, output: errorShape(refusal) }));
      if (err) this.log("agent runtime: the provider refused a tool result", { sessionId, code: err.code });
      return;
    }
    this.#runTool(live, exec, { sessionId, projectId: s.projectId, runId, callId, providerId: e.providerId, tool: e.tool, input: e.input, cwd: live.env.cwd, env: live.env.env });
  }

  /** Runs one call off the session's queue (commands and events go on meanwhile). */
  #runTool(live: Live, exec: ToolExecutor, c: Parameters<ToolExecutor["run"]>[0]): void {
    const ac = new AbortController();
    live.running.set(c.callId, ac);
    void exec
      .run(c, ac.signal)
      .catch((e: unknown) => ({ ok: false, output: errorShape(toFinError(e)) }))
      .then((outcome) =>
        this.background(c.sessionId, "a tool result", async () => {
          live.running.delete(c.callId);
          // A replaced or stopped instance never hears it: its synthetic result stands.
          if (this.lives.get(c.sessionId) !== live) return;
          const err = await call(live.provider.respond(c.providerId, { kind: "toolResult", ok: outcome.ok, output: outcome.output }));
          if (err) this.log("agent runtime: the provider refused a tool result", { sessionId: c.sessionId, code: err.code });
        }),
      );
  }

  async #toolCompleted(live: Live, e: Extract<ProviderEvent, { type: "toolCompleted" }>): Promise<void> {
    const sessionId = live.sessionId;
    const c = live.calls.get(e.providerId);
    if (c === undefined) {
      await this.append({ sessionId, runId: live.runId!, kind: "notice", payload: { unmatchedToolResult: e.providerId } });
      return;
    }
    live.calls.delete(e.providerId);
    const a = await this.append({ sessionId, runId: c.runId, kind: "toolResult", providerId: e.providerId, callId: c.callId, payload: { ok: e.ok, output: e.output } });
    if (a.inserted) {
      this.record("a tool result", () => this.journal.toolResult({ sessionId, runId: c.runId, callId: c.callId, ok: e.ok, output: e.output, synthetic: false }));
      await this.crossings(live, c.runId, this.budget(this.session(sessionId)).chargeTool());
    }
    if (live.calls.size === 0 && this.lives.get(sessionId) === live) await this.settled(live);
  }

  /** The calls in flight have their results (the safe point): a pending budget stop goes on. */
  protected async settled(live: Live): Promise<void> {
    if (live.pendingStop) await this.stop(live, live.pendingStop);
  }

  async #usage(live: Live, runId: string, e: Extract<ProviderEvent, { type: "usage" }>): Promise<void> {
    const s = this.session(live.sessionId);
    const tokens = { input: e.input, output: e.output, cacheRead: e.cacheRead, cacheWrite: e.cacheWrite, reasoning: e.reasoning };
    const priced = this.pricing.price(s, tokens, e.costMicros);
    const reservation = live.reservations.shift() ?? null;
    // A non-local call with neither a reported cost nor a price is charged what was held for
    // it (its projection), never 0: an unknown price must not make the money budget fail open.
    const projection = reservation?.micros ?? this.pricing.projected(s, tokens) ?? 0;
    const micros = priced.basis === "unknown" && s.provider !== "local" ? Math.max(priced.micros, projection) : priced.micros;
    const basis = priced.basis;
    const b = this.budget(s);
    const crossings = b.charge(tokens, micros, { reservation });
    const usage: Usage = { ...tokens, moneyMicros: micros, costBasis: basis };
    const { run, session } = this.store.addUsage(runId, usage);
    await this.append({ sessionId: s.id, runId, kind: "usage", payload: usage });
    this.record("a turn", () => this.journal.turn({ session, runId, usage }));
    this.publish(s.id, { type: "usageUpdated", runId, usage: run.usage, session: session.usage, budget: this.budgetState(b) });
    await this.crossings(live, runId, crossings);
  }

  protected async crossings(live: Live, runId: string, c: Crossings): Promise<void> {
    for (const n of c.notices) {
      await this.append({ sessionId: live.sessionId, runId, kind: "notice", payload: { budget: n.kind, used: n.used, limit: n.limit, at: 0.8 } });
      this.deps.sessionEvents?.budget(live.sessionId, n);
    }
    if (c.stop) await this.requestStop(live, c.stop);
  }

  /**
   * A budget stop: now, or once the calls in flight have their results (the safe point), but
   * no later than `stopGraceMs`: then the calls still running are aborted and the stop goes on
   * (their calls get the interrupt's synthetic results).
   */
  protected async requestStop(live: Live, r: Reading): Promise<void> {
    if (this.lives.get(live.sessionId) !== live || live.runId === null) return;
    if (live.calls.size > 0) {
      live.pendingStop ??= r;
      if (live.stopDeadline === null) {
        live.stopDeadline = setTimeout(() => {
          live.stopDeadline = null;
          if (this.dead) return;
          this.background(live.sessionId, "a budget stop's deadline", async () => {
            if (this.lives.get(live.sessionId) !== live || live.pendingStop === null) return;
            for (const ac of live.running.values()) ac.abort(budgetError(live.pendingStop));
            await this.stop(live, live.pendingStop);
          });
        }, this.deps.stopGraceMs ?? STOP_GRACE_MS);
        live.stopDeadline.unref?.();
      }
      return;
    }
    await this.stop(live, r);
  }

  protected async stop(live: Live, r: Reading): Promise<void> {
    const sessionId = live.sessionId;
    live.pendingStop = null;
    if (live.stopDeadline !== null) clearTimeout(live.stopDeadline);
    live.stopDeadline = null;
    const runId = live.runId;
    if (runId !== null) await this.append({ sessionId, runId, kind: "error", payload: errorShape(budgetError(r)) });
    live.runId = null;
    await call(live.provider.interrupt());
    await this.apply(sessionId, { kind: "interrupted", reason: "BUDGET_EXCEEDED" });
    const ttl = this.deps.approvalTtlMs ?? 7 * DAY_MS;
    const opened = this.store.openApproval({
      sessionId,
      kind: "budget",
      subject: { kind: r.kind, used: r.used, limit: r.limit },
      argsHash: `budget:${r.kind}`,
      expiresAt: this.clock.now() + ttl,
    });
    this.#approvalOpened(opened.session, opened.approval);
  }

  #approvalOpened(s: Session, a: Approval): void {
    this.record("an approval", () => this.journal.approval({ session: s, approval: a }));
    this.publish(s.id, { type: "approvalOpened", approval: a });
  }

  /** The turn ended by itself; held steers become the next run. */
  protected async runEnded(live: Live, runId: string, stop: string): Promise<void> {
    live.runId = null;
    await this.apply(live.sessionId, { kind: "runEnded", runId, stop });
    if (live.held.length > 0 && this.lives.get(live.sessionId) === live && this.session(live.sessionId).status === "idle") {
      await this.promptIn(live.sessionId, live.held.splice(0).join("\n\n"), "steer");
    }
  }

  async #turnFailed(live: Live, runId: string, error: FinError): Promise<void> {
    await this.append({ sessionId: live.sessionId, runId, kind: "error", payload: errorShape(error) });
    if (RUN_ONLY_STOPS.has(error.code)) return this.runEnded(live, runId, error.code);
    live.runId = null;
    await this.apply(live.sessionId, { kind: "interrupted", reason: error.code });
  }

  /** The instance is gone: it fails its turn with `PROVIDER_EXITED` and interrupts the session. */
  async #exited(live: Live, e: { code: number | null; signal: string | null }): Promise<void> {
    if (this.lives.get(live.sessionId) !== live) return;
    // Detached, not closed: its stream may still carry late events, which are dropped.
    this.lives.delete(live.sessionId);
    this.#release(live);
    const runId = live.runId;
    live.runId = null;
    const err = finError("PROVIDER_EXITED", "the provider exited", { code: e.code, signal: e.signal }, "resume the session");
    if (runId !== null) await this.append({ sessionId: live.sessionId, runId, kind: "error", payload: errorShape(err) });
    const s = this.session(live.sessionId);
    if (s.status !== "interrupted" && s.status !== "ended") await this.apply(s.id, { kind: "interrupted", reason: err.code });
  }

  /** Starts a run in the session's queue (the caller holds it). */
  protected abstract promptIn(sessionId: string, text: string, trigger: "prompt" | "steer", attachments?: ReadonlyArray<unknown>): Promise<Run>;
}
