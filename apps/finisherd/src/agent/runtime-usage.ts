// A `usage` event charged to its run (plan 7A Task 6b, 7D Task 9; Review Focus 4): priced,
// written to the store, the ledger and the journal, with the budget's crossings returned. A
// turn's tail (the `usage` an instance emits after its run was stopped, interrupted or its
// instance replaced) goes to the run that instance served, never to another run, and its
// crossing is enforced against the session as it is now: a run going on stops (`BUDGET_EXCEEDED`,
// a `budget` approval); an idle session's next prompt meets the spent budget. Its 80 % notices
// go the way a run's do (`notices`: the node, then the hooks' tap).
import type { BudgetState, ProviderEvent, Session, Usage, WatchItem } from "@finisher/contracts";
import type { SessionEventTap } from "../hooks/session.ts";
import type { Budget, Crossings, Reading, Reservation } from "./budget.ts";
import type { Pricing, RuntimeJournal } from "./runtime-deps.ts";
import type { Live } from "./runtime-live.ts";
import type { AgentStore, Appended, NewNode } from "./store.ts";

type UsageEvent = Extract<ProviderEvent, { type: "usage" }>;

/** What the engine lends the accounting (its own methods, bound). */
export interface AccountingHost {
  readonly store: AgentStore;
  readonly pricing: Pricing;
  readonly journal: RuntimeJournal;
  budget(s: Session): Budget;
  budgetState(b: Budget): BudgetState;
  append(n: NewNode): Promise<Appended>;
  record(what: string, f: () => void): void;
  publish(sessionId: string, item: WatchItem): void;
  /** A crossing's stop against the session now: its current instance's run, if one is going on. */
  stop(sessionId: string, r: Reading): Promise<void>;
  /** A late event nothing can take (counted, as the engine's dropped events are). */
  dropped(): void;
  /** The hooks' tap (`RuntimeDeps.sessionEvents`). */
  readonly events?: SessionEventTap | undefined;
}

export class UsageAccounting {
  readonly #h: AccountingHost;

  constructor(h: AccountingHost) {
    this.#h = h;
  }

  /** Charges `e` to `runId` (settling `reservation`, if any) and answers the crossings. */
  async charge(s: Session, runId: string, e: UsageEvent, reservation: Reservation | null): Promise<Crossings> {
    const h = this.#h;
    const tokens = { input: e.input, output: e.output, cacheRead: e.cacheRead, cacheWrite: e.cacheWrite, reasoning: e.reasoning };
    const priced = h.pricing.price(s, tokens, e.costMicros);
    // A non-local call with neither a reported cost nor a price is charged what was held for
    // it (its projection), never 0: an unknown price must not make the money budget fail open.
    const projection = reservation?.micros ?? h.pricing.projected(s, tokens) ?? 0;
    const micros = priced.basis === "unknown" && s.provider !== "local" ? Math.max(priced.micros, projection) : priced.micros;
    // The adapter's basis first (7D Decision 15): a plan sign-in's money is shown, never charged.
    const basis = e.costBasis ?? priced.basis;
    const b = h.budget(s);
    const crossings = b.charge(tokens, basis === "subscription" ? 0 : micros, { reservation });
    const usage: Usage = { ...tokens, moneyMicros: micros, costBasis: basis };
    const { run, session } = h.store.addUsage(runId, usage);
    await h.append({ sessionId: s.id, runId, kind: "usage", payload: usage });
    h.record("a turn", () => h.journal.turn({ session, runId, usage }));
    h.publish(s.id, { type: "usageUpdated", runId, usage: run.usage, session: session.usage, budget: h.budgetState(b) });
    return crossings;
  }

  /**
   * A turn's tail from `origin` (closed, replaced, or between runs): charged to the run
   * `origin` last served (else the session's last run), its notices there, its stop against
   * the session's current state.
   */
  async tail(origin: Live, e: UsageEvent): Promise<void> {
    const h = this.#h;
    const sessionId = origin.sessionId;
    const s = h.store.get(sessionId);
    const runId = origin.served ?? h.store.runs(sessionId).at(-1)?.id;
    if (s === undefined || runId === undefined) return h.dropped();
    const c = await this.charge(s, runId, e, null);
    await this.notices(sessionId, runId, c.notices);
    if (c.stop) await h.stop(sessionId, c.stop);
  }

  /** The 80 % notices of a charge: each a `notice` node in `runId`, then the hooks' tap (7F Decision 18). */
  async notices(sessionId: string, runId: string, notices: Crossings["notices"]): Promise<void> {
    for (const n of notices) {
      await this.#h.append({ sessionId, runId, kind: "notice", payload: { budget: n.kind, used: n.used, limit: n.limit, at: 0.8 } });
      this.#h.events?.budget(sessionId, n);
    }
  }

  /**
   * Steers a turn's tail hands back (`notice {steerReturned}`, plan 7D Decision 13, owner decision
   * 2026-11-13: an interrupted turn's held steers, never sent): recorded in the run it served, so
   * the texts are not lost with the run's end.
   */
  async tailNotice(origin: Live, e: Extract<ProviderEvent, { type: "notice" }>): Promise<void> {
    const h = this.#h;
    const runId = origin.served ?? h.store.runs(origin.sessionId).at(-1)?.id;
    if (runId === undefined) return h.dropped();
    await h.append({ sessionId: origin.sessionId, runId, kind: "notice", payload: { [e.name]: e.details ?? true } });
  }
}
