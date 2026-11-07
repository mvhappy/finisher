// Session budgets (sub-project 7 §13; plan 7A Decisions 10, 20, 21): tokens, money and wall clock
// per session in the ledger, money per server per UTC day, and the per-run caps on model and
// tool calls. Checked before each model call (projected) and after each turn and tool call:
// a notice once per kind at 80 %, a stop at 100 % (the runtime stops after the running tool
// call and opens a budget approval).
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

export type Allowance = { readonly ok: true } | ({ readonly ok: false } & Reading);

export interface Crossings {
  /** Kinds that crossed 80 % for the first time. */
  readonly notices: Reading[];
  /** The first kind at or over 100 %, if any (session kinds and the daily money cap). */
  readonly stop: Reading | null;
}

export interface BudgetOptions {
  /** Prices a projected call in micros (the catalogue's `price`); 0 when absent. */
  readonly price?: (u: Usage) => number;
  /** A cache-read token's weight in token budgets (the catalogue's `cacheReadWeight`). */
  readonly cacheReadWeight?: number;
}

const NOTICE_AT = 0.8;

const LIMIT_OF: Record<BudgetKind, keyof BudgetLimits> = { tokens: "tokens", money: "moneyMicros", wallMs: "wallMs" };

export class Budget {
  private readonly noticed = new Set<LimitKind>();
  private runCalls = 0;
  private runTools = 0;
  /** When the active run's wall time was last committed to the ledger; null between runs. */
  private since: number | null = null;

  constructor(
    private readonly ledger: LedgerStore,
    private readonly sessionId: string,
    private readonly limits: BudgetLimits,
    private readonly clock: Clock,
    private readonly opts: BudgetOptions = {},
  ) {
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

  /** Whether a model call projected to use `projected` fits every budget. */
  canCall(projected: Usage): Allowance {
    this.tick();
    if (this.runCalls >= this.limits.modelCallsPerRun) {
      return { ok: false, kind: "modelCalls", used: this.runCalls, limit: this.limits.modelCallsPerRun };
    }
    const micros = this.opts.price?.(projected) ?? 0;
    const wall = this.read("wallMs");
    if (wall.used >= wall.limit) return { ok: false, ...wall };
    const tokens = this.read("tokens");
    if (tokens.used + this.tokensOf(projected) > tokens.limit) return { ok: false, ...tokens };
    const money = this.read("money");
    if (money.used + micros > money.limit) return { ok: false, ...money };
    const daily = this.daily();
    if (daily.used + micros > daily.limit) return { ok: false, ...daily };
    return { ok: true };
  }

  /** Whether one more tool call fits the run's cap. */
  canTool(): Allowance {
    if (this.runTools >= this.limits.toolCallsPerRun) {
      return { ok: false, kind: "toolCalls", used: this.runTools, limit: this.limits.toolCallsPerRun };
    }
    return { ok: true };
  }

  /** After a model call: charges its tokens and `micros` and counts the call. */
  charge(u: Usage, micros: number): Crossings {
    this.runCalls += 1;
    this.ledger.add(this.sessionId, "tokens", { used: this.tokensOf(u) });
    this.ledger.add(this.sessionId, "money", { used: micros });
    this.ledger.addDailyMoney(utcDay(this.clock.now()), micros);
    return this.crossings();
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

  /** Raises a session limit by `by` (an approved `budget` approval); its 80 % notice may come again. */
  raise(kind: BudgetKind, by: number): void {
    const { limit } = this.ledger.read(this.sessionId, kind);
    this.ledger.setLimit(this.sessionId, kind, (limit ?? 0) + by);
    this.noticed.delete(kind);
  }

  /** Every reading, for the budget facts message at the head of a run. */
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
    const used = this.ledger.dailyMoney(utcDay(this.clock.now()));
    return { kind: "dailyMoney", used, limit: this.limits.dailyMoneyMicros };
  }

  private crossings(): Crossings {
    const all = this.readings();
    const notices: Reading[] = [];
    let stop: Reading | null = null;
    for (const r of all) {
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
