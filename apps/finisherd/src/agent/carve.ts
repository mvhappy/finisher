// Child budgets carved from the parent's (sub-project 7 §13.1; plan 7B Decision 22, owner
// decision 2026-11-08 for the default).
//
// - **The carve** (at spawn, `childCarve`): per carved kind, what the starter asked
//   (`--budget`, already at most the role's: `roles.ts` `budgetFor`), else the child role's
//   budget capped at half the parent's remaining (`limit − used − reserved`). More than the
//   parent has left, or a default of nothing (the parent has at most one unit left), is
//   `BUDGET_EXCEEDED` on the parent, before anything is made. A kind whose limit is 0 is one the
//   parent has none of (0 is none, never unlimited: a free call still runs on it): its default
//   carve is 0, and asking for any of it is `BUDGET_EXCEEDED`.
// - **Reserved with the child's record** (`LedgerStore.carve`, inside `AgentStore.createSession`'s
//   transaction): the parent's `reserved` += the carve, the child's `budgets` rows hold it as
//   their limits, and `budget_carves` (agent migration 0007) records it; re-checked there, so a
//   race fails `BUDGET_EXCEEDED` on the parent and records nothing.
// - **Settled at the child's end** (`settleCarve`, in the transaction of the child's `ended`,
//   whichever path ends it: its brief, a person, a cascade, a delete): the parent's `reserved`
//   drops by the carve and its `used` rises by what the child spent. The carves the child still
//   holds for its own live children pass to the parent (held there instead), so their spend,
//   and a raise of them, reach the nearest live ancestor (review 7B Task 7: a carve held by an
//   ended child would let a raise escape the root). Recorded as settled, so it happens once. A
//   parent that is gone is left alone.
// - **Raised by a person only** (`raise`; agents hold no `agent.approve`): a child's raise
//   reserves as much on the holder of its carve (its parent, or the nearest live ancestor) within
//   that one's remaining, or is `BUDGET_EXCEEDED` there and raises nothing. That holder's own
//   limit never moves for it.
// - **Recovery** (`DbLedger.recover`, at start): per-call reservations return (7A), every open
//   carve is held again on its parent, then each carve whose child has ended or is gone is
//   settled, deepest first (a gone child's spend is unknown: it is charged its whole carve,
//   failing closed, and its own children's carves stay within it).
// - **Kinds:** tokens and money. The wall clock is not carved: it runs for a parent and its
//   children at once, so it is no shared stock; a child's is its own (the role's, or lower).
//   Money for the server's day is charged by real spend only: a settlement never touches it.
import type { BudgetSpec, FinError } from "@finisher/contracts";
import { finError } from "../errors.ts";
import type { BudgetLimits } from "./budget.ts";
import type { BudgetKind, Ledger, LedgerStore } from "./ledger.ts";

export const CARVED_KINDS = ["tokens", "money"] as const;
export type CarvedKind = (typeof CARVED_KINDS)[number];
/** A carve per kind; a kind left out is not carved. */
export type Carve = Partial<Record<CarvedKind, number>>;
/** A raise per kind, as a `budget` approval's `allow` asks it. */
export type Raise = { readonly [K in BudgetKind]?: number | undefined };

const SPEC: Record<CarvedKind, "tokens" | "moneyMicros"> = { tokens: "tokens", money: "moneyMicros" };

/** One open carve: `amount` of `kind` the child `sessionId` holds from `parentId`. */
export interface CarveRow {
  readonly sessionId: string;
  readonly parentId: string;
  readonly kind: CarvedKind;
  readonly amount: number;
}

/** What a ledger keeps of its carves beside its rows (`budget_carves`, or memory). */
export interface CarveRecords {
  /** The child's carves not yet settled. */
  open(sessionId: string): CarveRow[];
  record(row: CarveRow): void;
  grow(sessionId: string, kind: CarvedKind, by: number): void;
  settled(sessionId: string): void;
  /** What `parentId` has carved out of `kind` and not yet settled. */
  heldBy(parentId: string, kind: CarvedKind): number;
  /** `from`'s open carves of `kind` are held by `to` from now on. */
  pass(from: string, to: string, kind: CarvedKind): void;
}

const remaining = (r: Ledger): number => (r.limit === null ? 0 : Math.max(0, r.limit - r.used - r.reserved));

function overParent(parentId: string, kind: CarvedKind, asked: number, left: number, what: string): FinError {
  return finError(
    "BUDGET_EXCEEDED",
    `the parent session has ${left} ${kind === "money" ? "micros" : "tokens"} left, less than ${what}`,
    { session: parentId, scope: "parent", kind, asked, remaining: left },
    "ask for less, or a person raises the parent's budget first",
  );
}

/**
 * Decision 22: the carve of a child of `parentId`, and the child's budget with it as limits.
 * `planned` is the planner's budget (the role's, lowered by `asked`).
 */
export function childCarve(l: LedgerStore, parentId: string, planned: BudgetSpec, asked: BudgetSpec | undefined, defaults: BudgetLimits): { budget: BudgetSpec; carve: Carve } {
  const carve: Carve = {};
  const budget: Record<string, number> = { ...(planned as Record<string, number>) };
  for (const kind of CARVED_KINDS) {
    const key = SPEC[kind];
    const left = remaining(l.read(parentId, kind));
    const role = planned[key] ?? defaults[key];
    // A kind the parent has none of (limit 0) carves 0 by default.
    const none = l.read(parentId, kind).limit === 0;
    const want = asked?.[key] === undefined ? (none ? 0 : Math.min(role, Math.floor(left / 2))) : role;
    if (want > left) throw overParent(parentId, kind, want, left, `the ${want} asked`);
    if (want === 0 && role > 0 && !none) throw overParent(parentId, kind, role, left, "two to halve");
    carve[kind] = want;
    budget[key] = want;
  }
  return { budget: budget as BudgetSpec, carve };
}

/** Reserves `carve` on the parent and makes it the child's limits (the caller's transaction). */
export function carveIn(l: LedgerStore, r: CarveRecords, parentId: string, sessionId: string, carve: Carve): void {
  for (const kind of CARVED_KINDS) {
    const want = carve[kind];
    const left = remaining(l.read(parentId, kind));
    if (want !== undefined && want > left) throw overParent(parentId, kind, want, left, `the ${want} asked`);
  }
  for (const kind of CARVED_KINDS) {
    const amount = carve[kind];
    if (amount === undefined) continue;
    l.add(parentId, kind, { reserved: amount });
    l.setLimit(sessionId, kind, amount);
    r.record({ sessionId, parentId, kind, amount });
  }
}

/**
 * Returns the child's open carves to their parent, charging what it spent, and passes the
 * carves it holds for its own live children to that parent; once.
 */
export function settleIn(l: LedgerStore, r: CarveRecords, sessionId: string): void {
  const open = r.open(sessionId);
  if (open.length === 0) return;
  for (const c of open) {
    if (l.read(c.parentId, c.kind).limit === null) continue;
    const child = l.read(sessionId, c.kind);
    // A gone child's spend is unknown: its whole carve, which covers its own children's.
    if (child.limit === null) {
      l.add(c.parentId, c.kind, { reserved: -c.amount, used: c.amount });
      continue;
    }
    const held = r.heldBy(sessionId, c.kind);
    l.add(c.parentId, c.kind, { reserved: held - c.amount, used: child.used });
    if (held > 0) l.add(sessionId, c.kind, { reserved: -held });
    r.pass(sessionId, c.parentId, c.kind);
  }
  r.settled(sessionId);
}

/** Why `add` cannot be raised on `sessionId`: its parent has less left than a carved kind asks. */
export function raiseRefused(l: LedgerStore, r: CarveRecords, sessionId: string, add: Raise): FinError | undefined {
  for (const c of r.open(sessionId)) {
    const by = add[c.kind] ?? 0;
    const left = remaining(l.read(c.parentId, c.kind));
    if (by > left) return overParent(c.parentId, c.kind, by, left, `the ${by} asked for its child`);
  }
  return undefined;
}

/** Raises `sessionId`'s limits by `add`; a carved kind reserves as much on the parent (the caller's transaction). */
export function raiseIn(l: LedgerStore, r: CarveRecords, sessionId: string, add: Raise): void {
  const refused = raiseRefused(l, r, sessionId, add);
  if (refused !== undefined) throw refused;
  const open = r.open(sessionId);
  for (const [kind, by] of Object.entries(add) as Array<[BudgetKind, number | undefined]>) {
    if (by === undefined || by <= 0) continue;
    const c = open.find((x) => x.kind === kind);
    if (c !== undefined) {
      l.add(c.parentId, c.kind, { reserved: by });
      r.grow(sessionId, c.kind, by);
    }
    l.setLimit(sessionId, kind, (l.read(sessionId, kind).limit ?? 0) + by);
  }
}

/** `CarveRecords` in memory (`MemoryLedger`). */
export class MemoryCarves implements CarveRecords {
  readonly #rows = new Map<string, CarveRow[]>();

  open(sessionId: string): CarveRow[] {
    return this.#rows.get(sessionId) ?? [];
  }

  record(row: CarveRow): void {
    this.#rows.set(row.sessionId, [...this.open(row.sessionId).filter((x) => x.kind !== row.kind), row]);
  }

  grow(sessionId: string, kind: CarvedKind, by: number): void {
    this.#rows.set(sessionId, this.open(sessionId).map((x) => (x.kind === kind ? { ...x, amount: x.amount + by } : x)));
  }

  settled(sessionId: string): void {
    this.#rows.delete(sessionId);
  }

  pass(from: string, to: string, kind: CarvedKind): void {
    for (const [id, rows] of this.#rows) this.#rows.set(id, rows.map((x) => (x.parentId === from && x.kind === kind ? { ...x, parentId: to } : x)));
  }

  heldBy(parentId: string, kind: CarvedKind): number {
    let n = 0;
    for (const rows of this.#rows.values()) for (const x of rows) if (x.parentId === parentId && x.kind === kind) n += x.amount;
    return n;
  }
}
