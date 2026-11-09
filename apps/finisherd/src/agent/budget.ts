// Session budgets (sub-project 7 §13; plan 7A Decisions 10, 20, 21): tokens, money and wall clock
// per session in the ledger, money per server per UTC day, and the per-run caps on model and
// tool calls. Checked before each model call (projected, and the projection's tokens and money
// are **reserved** in the ledger until the call is charged, so concurrent calls cannot overshoot
// together; review H3 and the 3b/6b/7 review) and after each turn and tool call: a notice once
// per kind at 80 %, a stop at 100 % (the runtime stops after the running tool call and opens a
// budget approval). Failed and retried attempts are charged too (review B2).
//
// What resets (review L4): the per-run call counters belong to the run and start at zero with
// each run, a resumed one included; the 80 % notices are remembered by this `Budget` only, so
// after a restart a notice may be shown once more. Both are deliberate: neither is money, and
// the ledger (used, reserved, limits) is what persists.
import type { Usage } from "./catalogue/catalogue.ts";
import type { Clock } from "./clock.ts";
import { type BudgetKind, BUDGET_KINDS, type LedgerStore, utcDay } from "./ledger.ts";

export interface BudgetLimits {
  readonly tokens: number;
  readonly moneyMicros: number;
  /** Wall clock with a run active. */
  readonly wallMs: number;
  readonly modelCallsPerRun: number;
  readonly toolCallsPerRun: number;
  /** Across every built-in session of the server, per UTC day. */
  readonly dailyMoneyMicros: number;
}

/** Decision 20's defaults (settings `agents.defaultBudget`, `agents.dailyMoneyMicros`). */
export const DEFAULT_BUDGET: BudgetLimits = {
  tokens: 2_000_000,
  moneyMicros: 10_000_000,
  wallMs: 60 * 60_000,
  modelCallsPerRun: 60,
  toolCallsPerRun: 200,
  dailyMoneyMicros: 50_000_000,
};

export type LimitKind = BudgetKind | "modelCalls" | "toolCalls" | "dailyMoney";

export interface Reading {
  readonly kind: LimitKind;
  readonly used: number;
  readonly limit: number;
}

/** Tokens and money held for one model call between `canCall` and `charge` (or `release`). */
export interface Reservation {
  readonly micros: number;
  readonly tokens: number;
  readonly day: string;
}

export type Allowance = { readonly ok: true; readonly reservation: Reservation | null } | ({ readonly ok: false } & Reading);

export interface Crossings {
  /** Kinds that crossed 80 % for the first time. */
  readonly notices: Reading[];
  /** The first kind at or over 100 %, if any (session kinds and the daily money cap). */
  readonly stop: Reading | null;
}

export interface BudgetOptions {
  /**
   * Prices a projected call in micros (the catalogue's `projectedMicros`); null when the price
   * is unknown, which reserves nothing: such a model is charged after the call only.
   */
  readonly price?: (u: Usage) => number | null;
  /** A cache-read token's weight in token budgets (the catalogue's `cacheReadWeight`). */
  readonly cacheReadWeight?: number;
}

export interface ChargeOptions {
  /** The reservation `canCall` made for this call: released as the actual cost is charged. */
  readonly reservation?: Reservation | null;
  /** Counts as a model call (false for a failed attempt the client retries). Default true. */
  readonly call?: boolean;
}

const NOTICE_AT = 0.8;

const LIMIT_OF: Record<BudgetKind, keyof BudgetLimits> = { tokens: "tokens", money: "moneyMicros", wallMs: "wallMs" };

export class Budget {
  private readonly noticed = new Set<LimitKind>();
  private runCalls = 0;
  private runTools = 0;
  /** When the active run's wall time was last committed to the ledger; null between runs. */
  private since: number | null = null;
  private readonly ledger: LedgerStore;
  private readonly sessionId: string;
  private readonly limits: BudgetLimits;
  private readonly clock: Clock;
  private readonly opts: BudgetOptions;

  constructor(
    ledger: LedgerStore,
    sessionId: string,
    limits: BudgetLimits,
    clock: Clock,
    opts: BudgetOptions = {},
  ) {
    this.ledger = ledger;
    this.sessionId = sessionId;
    this.limits = limits;
    this.clock = clock;
    this.opts = opts;
    for (const kind of BUDGET_KINDS) {
      if (ledger.read(sessionId, kind).limit === null) ledger.setLimit(sessionId, kind, limits[LIMIT_OF[kind]]);
    }
  }

  /** Token-budget weight of `u`: cache reads count at their price ratio. */
  tokensOf(u: Usage): number {
    const w = this.opts.cacheReadWeight ?? 1;
    return u.input + u.output + u.cacheWrite + Math.ceil(u.cacheRead * w);
  }

  /** A run begins: the per-run caps reset and the wall clock runs. */
  startRun(): void {
    this.runCalls = 0;
    this.runTools = 0;
    this.noticed.delete("modelCalls");
    this.noticed.delete("toolCalls");
    this.since = this.clock.now();
  }

  /** The run ended: its wall time is committed and the clock stops. */
  endRun(): void {
    this.tick();
    this.since = null;
  }

  /**
   * Whether a model call projected to use `projected` fits every budget, counting the tokens
   * and money other calls hold. When it fits, its projected tokens and cost are reserved until
   * `charge` or `release`.
   */
  canCall(projected: Usage): Allowance {
    this.tick();
    if (this.runCalls >= this.limits.modelCallsPerRun) {
      return { ok: false, kind: "modelCalls", used: this.runCalls, limit: this.limits.modelCallsPerRun };
    }
    const micros = Math.max(0, this.opts.price?.(projected) ?? 0);
    const wall = this.read("wallMs");
    if (wall.used >= wall.limit) return { ok: false, ...wall };
    const tokens = this.ledger.read(this.sessionId, "tokens");
    const tokenLimit = tokens.limit ?? this.limits.tokens;
    const wantTokens = this.tokensOf(projected);
    if (tokens.used + tokens.reserved + wantTokens > tokenLimit) {
      return { ok: false, kind: "tokens", used: tokens.used + tokens.reserved, limit: tokenLimit };
    }
    const money = this.ledger.read(this.sessionId, "money");
    const moneyLimit = money.limit ?? this.limits.moneyMicros;
    if (money.used + money.reserved + micros > moneyLimit) {
      return { ok: false, kind: "money", used: money.used + money.reserved, limit: moneyLimit };
    }
    const day = utcDay(this.clock.now());
    const daily = this.ledger.daily(day);
    if (daily.used + daily.reserved + micros > this.limits.dailyMoneyMicros) {
      return { ok: false, kind: "dailyMoney", used: daily.used + daily.reserved, limit: this.limits.dailyMoneyMicros };
    }
    if (micros === 0 && wantTokens === 0) return { ok: true, reservation: null };
    if (wantTokens > 0) this.ledger.add(this.sessionId, "tokens", { reserved: wantTokens });
    if (micros > 0) {
      this.ledger.add(this.sessionId, "money", { reserved: micros });
      this.ledger.addDaily(day, { reserved: micros });
    }
    return { ok: true, reservation: { micros, tokens: wantTokens, day } };
  }

  /** Returns a reservation unused (the call never reached the model). */
  release(r: Reservation | null | undefined): void {
    if (!r) return;
    if (r.tokens > 0) this.ledger.add(this.sessionId, "tokens", { reserved: -r.tokens });
    if (r.micros > 0) {
      this.ledger.add(this.sessionId, "money", { reserved: -r.micros });
      this.ledger.addDaily(r.day, { reserved: -r.micros });
    }
  }

  /**
   * After a model call or a failed attempt: charges its tokens and `micros`, settles the
   * reservation, and counts the call (unless `call: false`).
   */
  charge(u: Usage, micros: number, o: ChargeOptions = {}): Crossings {
    this.release(o.reservation);
    if (o.call ?? true) this.runCalls += 1;
    this.ledger.add(this.sessionId, "tokens", { used: this.tokensOf(u) });
    this.ledger.add(this.sessionId, "money", { used: micros });
    this.ledger.addDaily(utcDay(this.clock.now()), { used: micros });
    return this.crossings();
  }

  /** Whether one more tool call fits the run's cap. */
  canTool(): Allowance {
    if (this.runTools >= this.limits.toolCallsPerRun) {
      return { ok: false, kind: "toolCalls", used: this.runTools, limit: this.limits.toolCallsPerRun };
    }
    return { ok: true, reservation: null };
  }

  /** After a tool call: counts it. */
  chargeTool(): Crossings {
    this.runTools += 1;
    return this.crossings();
  }

  /** The current crossings without charging anything (the wall clock moves on its own). */
  check(): Crossings {
    return this.crossings();
  }

  /**
   * Raises a session limit by `by` (an approved `budget` approval); its 80 % notice may come again.
   * A child's raise reserves as much on its parent, or fails `BUDGET_EXCEEDED` there (`carve.ts`).
   */
  raise(kind: BudgetKind, by: number): void {
    this.ledger.raise(this.sessionId, { [kind]: by });
    this.noticed.delete(kind);
  }

  /** Every reading (money as spent, not held), for the budget facts at the head of a run. */
  readings(): Reading[] {
    this.tick();
    return [
      ...BUDGET_KINDS.map((k) => this.read(k)),
      this.daily(),
      { kind: "modelCalls", used: this.runCalls, limit: this.limits.modelCallsPerRun },
      { kind: "toolCalls", used: this.runTools, limit: this.limits.toolCallsPerRun },
    ];
  }

  private tick(): void {
    if (this.since === null) return;
    const now = this.clock.now();
    if (now > this.since) this.ledger.add(this.sessionId, "wallMs", { used: now - this.since });
    this.since = now;
  }

  private read(kind: BudgetKind): Reading {
    const l = this.ledger.read(this.sessionId, kind);
    return { kind, used: l.used, limit: l.limit ?? this.limits[LIMIT_OF[kind]] };
  }

  private daily(): Reading {
    const used = this.ledger.daily(utcDay(this.clock.now())).used;
    return { kind: "dailyMoney", used, limit: this.limits.dailyMoneyMicros };
  }

  private crossings(): Crossings {
    const notices: Reading[] = [];
    let stop: Reading | null = null;
    for (const r of this.readings()) {
      if (r.used >= r.limit * NOTICE_AT && !this.noticed.has(r.kind)) {
        this.noticed.add(r.kind);
        notices.push(r);
      }
      const stops = r.kind !== "modelCalls" && r.kind !== "toolCalls";
      if (stops && stop === null && r.used >= r.limit) stop = r;
    }
    return { notices, stop };
  }
}
