// The budget ledger behind an interface (plan 7A Decision 21): in memory in 7A-1; `control.db`'s
// `budgets` table in 7A-2 (`AgentStore.ledger()`). Children's carve-outs (7B): `carve.ts`.
import type { FinError } from "@finisher/contracts";
import { type Carve, carveIn, MemoryCarves, type Raise, raiseIn, raiseRefused, settleIn } from "./carve.ts";

/** The kinds a session's ledger holds. Per-run call caps are counted by `Budget` itself. */
export type BudgetKind = "tokens" | "money" | "wallMs";

export const BUDGET_KINDS: readonly BudgetKind[] = ["tokens", "money", "wallMs"];

export interface Ledger {
  /** null until a limit is set. */
  readonly limit: number | null;
  readonly used: number;
  /**
   * Held, not yet spent: the projected cost of a model call between `Budget.canCall` and its
   * `charge` (review H3), and the carve-outs of live children (7B, `carve.ts`). Returned when settled.
   */
  readonly reserved: number;
}

/** The server's money for one UTC day across sessions. */
export interface Daily {
  readonly used: number;
  readonly reserved: number;
}

export interface LedgerStore {
  read(sessionId: string, kind: BudgetKind): Ledger;
  add(sessionId: string, kind: BudgetKind, delta: { used?: number; reserved?: number }): Ledger;
  setLimit(sessionId: string, kind: BudgetKind, limit: number): void;
  /** Money (micros) charged and held on `utcDay` (`YYYY-MM-DD`) across the server's sessions. */
  daily(utcDay: string): Daily;
  addDaily(utcDay: string, delta: { used?: number; reserved?: number }): Daily;
  /** Reserves `carve` on the parent as the child's limits; `BUDGET_EXCEEDED` on the parent when it does not fit. */
  carve(parentId: string, sessionId: string, carve: Carve): void;
  /** Returns the child's carve to its parent, charging what the child spent; once (`carve.ts`). */
  settleCarve(sessionId: string): void;
  /** Raises the limits by `add` (a person's `budget` approval); a child's reserves as much on its parent, or fails on the parent. */
  raise(sessionId: string, add: Raise): void;
  /** The `BUDGET_EXCEEDED` that `raise(sessionId, add)` would fail with, if any. */
  canRaise(sessionId: string, add: Raise): FinError | undefined;
}

const EMPTY: Ledger = { limit: null, used: 0, reserved: 0 };

export class MemoryLedger implements LedgerStore {
  private readonly rows = new Map<string, Ledger>();
  private readonly days = new Map<string, Daily>();
  private readonly carves = new MemoryCarves();

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

  daily(utcDay: string): Daily {
    return this.days.get(utcDay) ?? { used: 0, reserved: 0 };
  }

  addDaily(utcDay: string, delta: { used?: number; reserved?: number }): Daily {
    const old = this.daily(utcDay);
    const row = { used: old.used + (delta.used ?? 0), reserved: Math.max(0, old.reserved + (delta.reserved ?? 0)) };
    this.days.set(utcDay, row);
    return row;
  }

  carve(parentId: string, sessionId: string, carve: Carve): void {
    carveIn(this, this.carves, parentId, sessionId, carve);
  }

  settleCarve(sessionId: string): void {
    settleIn(this, this.carves, sessionId);
  }

  raise(sessionId: string, add: Raise): void {
    raiseIn(this, this.carves, sessionId, add);
  }

  canRaise(sessionId: string, add: Raise): FinError | undefined {
    return raiseRefused(this, this.carves, sessionId, add);
  }
}

/** The UTC day of `ms`, `YYYY-MM-DD`. */
export function utcDay(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}
