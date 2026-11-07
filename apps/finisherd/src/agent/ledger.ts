// The budget ledger behind an interface (plan 7A Decision 21): in memory in 7A-1; `control.db`'s
// `budgets` table in 7A-2 (`AgentStore.ledger()`).

/** The kinds a session's ledger holds. Per-run call caps are counted by `Budget` itself. */
export type BudgetKind = "tokens" | "money" | "wallMs";

export const BUDGET_KINDS: readonly BudgetKind[] = ["tokens", "money", "wallMs"];

export interface Ledger {
  /** null until a limit is set. */
  readonly limit: number | null;
  readonly used: number;
  /** Carved out for children (7B); returned when they end. */
  readonly reserved: number;
}

export interface LedgerStore {
  read(sessionId: string, kind: BudgetKind): Ledger;
  add(sessionId: string, kind: BudgetKind, delta: { used?: number; reserved?: number }): Ledger;
  setLimit(sessionId: string, kind: BudgetKind, limit: number): void;
  /** Money (micros) charged on `utcDay` (`YYYY-MM-DD`) across the server's sessions. */
  dailyMoney(utcDay: string): number;
  addDailyMoney(utcDay: string, micros: number): void;
}

const EMPTY: Ledger = { limit: null, used: 0, reserved: 0 };

export class MemoryLedger implements LedgerStore {
  private readonly rows = new Map<string, Ledger>();
  private readonly days = new Map<string, number>();

  read(sessionId: string, kind: BudgetKind): Ledger {
    return this.rows.get(`${sessionId}/${kind}`) ?? EMPTY;
  }

  add(sessionId: string, kind: BudgetKind, delta: { used?: number; reserved?: number }): Ledger {
    const old = this.read(sessionId, kind);
    const row = {
      limit: old.limit,
      used: old.used + (delta.used ?? 0),
      reserved: Math.max(0, old.reserved + (delta.reserved ?? 0)),
    };
    this.rows.set(`${sessionId}/${kind}`, row);
    return row;
  }

  setLimit(sessionId: string, kind: BudgetKind, limit: number): void {
    this.rows.set(`${sessionId}/${kind}`, { ...this.read(sessionId, kind), limit });
  }

  dailyMoney(utcDay: string): number {
    return this.days.get(utcDay) ?? 0;
  }

  addDailyMoney(utcDay: string, micros: number): void {
    this.days.set(utcDay, this.dailyMoney(utcDay) + micros);
  }
}

/** The UTC day of `ms`, `YYYY-MM-DD`. */
export function utcDay(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}
