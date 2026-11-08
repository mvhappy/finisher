// The agent store's side tables (plan 7A Decisions 1, 14a, 21; spec §5.1, §5.2): `session_txs`,
// `session_state` and the budget ledger (`budgets`, `budget_days`). `AgentStore` owns one of
// each and answers them; nothing else touches these tables.
import type { StatementSync } from "node:sqlite";
import type { Attribution } from "@finisher/core-types";
import type { ControlStore } from "../control/db.ts";
import { finError } from "../errors.ts";
import type { BudgetKind, Daily, Ledger, LedgerStore } from "./ledger.ts";

/** What became of an agent's transaction (spec §12.5; `outcomes.ts`, Task 11, sets it). */
export type TxOutcome = "merged" | "undone" | "reverted" | "abandoned";

export const TX_OUTCOMES: readonly TxOutcome[] = ["merged", "undone", "reverted", "abandoned"];
export const ATTRIBUTIONS: readonly Attribution[] = ["exact", "inferred", "ambiguous", "none"];

/** One `session_txs` row. */
export interface SessionTx {
  readonly txId: string;
  readonly sessionId: string;
  /** The tool call that made it, when known (`FIN_CALL`); null otherwise (spec §12.3). */
  readonly callId: string | null;
  readonly attribution: Attribution;
  readonly at: number;
  readonly outcome?: TxOutcome;
}

interface TxRow {
  txId: string;
  sessionId: string;
  callId: string | null;
  attribution: string;
  at: number;
  outcome: string | null;
}

const TX_COLUMNS = "txId, sessionId, callId, attribution, at, outcome";

function toTx(r: TxRow): SessionTx {
  return {
    txId: r.txId,
    sessionId: r.sessionId,
    callId: r.callId,
    attribution: r.attribution as Attribution,
    at: r.at,
    ...(r.outcome === null ? {} : { outcome: r.outcome as TxOutcome }),
  };
}

/** A recorded `fin try` and the default it replaced (contracts `SessionTry`). */
export interface SessionTryRow {
  readonly branch: string;
  readonly previousDefault: string;
}

/** A session's own branch state (`agent.self`): its default branch and `fin try` list. */
export interface SessionBranchState {
  readonly defaultBranch: string;
  readonly tries: ReadonlyArray<SessionTryRow>;
}

const prepare = (cs: ControlStore, sql: Record<string, string>): Record<string, StatementSync> =>
  Object.fromEntries(Object.entries(sql).map(([k, v]) => [k, cs.db.prepare(v)]));

/** `session_txs`: every agent commit, undo, redo, revert, cherry-pick and merge (spec §5.2). */
export class SessionTxs {
  readonly #s: Record<string, StatementSync>;

  constructor(cs: ControlStore) {
    this.#s = prepare(cs, {
      // The project is the session's, kept on the row: the row outlives its session (agent 0002).
      insert:
        "INSERT INTO session_txs (txId, sessionId, callId, attribution, at, projectId) " +
        "VALUES (?, ?, ?, ?, ?, (SELECT projectId FROM sessions WHERE id = ?)) ON CONFLICT (txId) DO NOTHING",
      of: `SELECT ${TX_COLUMNS} FROM session_txs WHERE txId IN (SELECT value FROM json_each(?))`,
      outcome: "UPDATE session_txs SET outcome = ? WHERE txId = ?",
      bySession: `SELECT ${TX_COLUMNS} FROM session_txs WHERE sessionId = ? AND at >= ? ORDER BY at, txId`,
      byCall: `SELECT ${TX_COLUMNS} FROM session_txs WHERE callId = ? ORDER BY at, txId`,
    });
  }

  /**
   * Records one transaction under its session. The authorisation wrapper's after stage calls it
   * for every write of an agent principal, whatever the attribution; a second call for the same
   * txId (a retried after stage, an idempotent replay) changes nothing.
   */
  record(t: Omit<SessionTx, "outcome">): void {
    if (!ATTRIBUTIONS.includes(t.attribution)) throw finError("INVALID_REQUEST", "unknown attribution", { attribution: t.attribution });
    this.#s.insert!.run(t.txId, t.sessionId, t.callId, t.attribution, t.at, t.sessionId);
  }

  /** The rows of those of `txIds` an agent made; the others are absent from the map. */
  of(txIds: ReadonlyArray<string>): Map<string, SessionTx> {
    const out = new Map<string, SessionTx>();
    if (txIds.length === 0) return out;
    for (const r of this.#s.of!.all(JSON.stringify(txIds)) as unknown as TxRow[]) out.set(r.txId, toTx(r));
    return out;
  }

  setOutcome(txId: string, outcome: TxOutcome): boolean {
    if (!TX_OUTCOMES.includes(outcome)) throw finError("INVALID_REQUEST", "unknown outcome", { outcome });
    return Number(this.#s.outcome!.run(outcome, txId).changes) > 0;
  }

  /** The session's transactions from `since` (ms) on, oldest first. */
  bySession(sessionId: string, since = 0): SessionTx[] {
    return (this.#s.bySession!.all(sessionId, since) as unknown as TxRow[]).map(toTx);
  }

  /** The transactions one tool call made (`agent.toolResult.txIds`). */
  byCall(callId: string): SessionTx[] {
    return (this.#s.byCall!.all(callId) as unknown as TxRow[]).map(toTx);
  }
}

/**
 * `session_state` (spec §5.1, Decision 14a): per session, never shared, so one session's
 * `fin try` changes nothing for another session or for a person's `fin`. The caller checks
 * that a try branch lies in the session's subtree (reach, Task 7).
 */
export class SessionStates {
  readonly #s: Record<string, StatementSync>;

  constructor(cs: ControlStore) {
    this.#s = prepare(cs, {
      insert: "INSERT INTO session_state (sessionId, defaultBranch, tries) VALUES (?, ?, '[]')",
      get: "SELECT defaultBranch, tries FROM session_state WHERE sessionId = ?",
      set: "UPDATE session_state SET defaultBranch = ?, tries = ? WHERE sessionId = ?",
    });
  }

  create(sessionId: string, defaultBranch: string): void {
    this.#s.insert!.run(sessionId, defaultBranch);
  }

  get(sessionId: string): SessionBranchState | undefined {
    const r = this.#s.get!.get(sessionId) as { defaultBranch: string; tries: string } | undefined;
    return r === undefined ? undefined : { defaultBranch: r.defaultBranch, tries: JSON.parse(r.tries) as SessionTryRow[] };
  }

  #put(sessionId: string, s: SessionBranchState): SessionBranchState {
    this.#s.set!.run(s.defaultBranch, JSON.stringify(s.tries), sessionId);
    return s;
  }

  /** Makes `branch` the default: the root, or a recorded try (`BRANCH_NOT_ALLOWED` otherwise). */
  setDefault(sessionId: string, s: SessionBranchState, root: string | null, branch: string): SessionBranchState {
    const allowed = branch === root || branch === s.defaultBranch || s.tries.some((t) => t.branch === branch);
    if (!allowed) {
      throw finError("BRANCH_NOT_ALLOWED", "the default branch must be the session's root or a recorded try", { branch }, "record the try first");
    }
    return this.#put(sessionId, { ...s, defaultBranch: branch });
  }

  /** Notes a try branch with the default it would replace; a branch already recorded is kept as it was. */
  recordTry(sessionId: string, s: SessionBranchState, branch: string): SessionBranchState {
    if (s.tries.some((t) => t.branch === branch)) return s;
    return this.#put(sessionId, { ...s, tries: [...s.tries, { branch, previousDefault: s.defaultBranch }] });
  }

  /**
   * Ends a try. If it was the default, the default it replaced comes back; a later try that
   * replaced it inherits what it replaced, so no default ever points at a cleared try.
   */
  clearTry(sessionId: string, s: SessionBranchState, branch: string): SessionBranchState {
    const gone = s.tries.find((t) => t.branch === branch);
    if (gone === undefined) return s;
    const tries = s.tries
      .filter((t) => t.branch !== branch)
      .map((t) => (t.previousDefault === branch ? { ...t, previousDefault: gone.previousDefault } : t));
    const defaultBranch = s.defaultBranch === branch ? gone.previousDefault : s.defaultBranch;
    return this.#put(sessionId, { defaultBranch, tries });
  }
}

/** `LedgerStore` over `budgets` and `budget_days` (Task 4's interface; each call one statement). */
export class DbLedger implements LedgerStore {
  readonly #s: Record<string, StatementSync>;

  constructor(cs: ControlStore) {
    this.#s = prepare(cs, {
      read: 'SELECT "limit" AS "limit", used, reserved FROM budgets WHERE sessionId = ? AND kind = ?',
      add:
        "INSERT INTO budgets (sessionId, kind, used, reserved) VALUES (?, ?, ?, MAX(0, ?)) ON CONFLICT (sessionId, kind) DO UPDATE SET " +
        'used = used + excluded.used, reserved = MAX(0, reserved + ?) RETURNING "limit" AS "limit", used, reserved',
      limit: 'INSERT INTO budgets (sessionId, kind, "limit") VALUES (?, ?, ?) ON CONFLICT (sessionId, kind) DO UPDATE SET "limit" = excluded."limit"',
      daily: "SELECT used, reserved FROM budget_days WHERE day = ?",
      addDaily:
        "INSERT INTO budget_days (day, used, reserved) VALUES (?, ?, MAX(0, ?)) ON CONFLICT (day) DO UPDATE SET " +
        "used = used + excluded.used, reserved = MAX(0, reserved + ?) RETURNING used, reserved",
    });
  }

  read(sessionId: string, kind: BudgetKind): Ledger {
    const r = this.#s.read!.get(sessionId, kind) as Ledger | undefined;
    return r === undefined ? { limit: null, used: 0, reserved: 0 } : { limit: r.limit, used: r.used, reserved: r.reserved };
  }

  add(sessionId: string, kind: BudgetKind, delta: { used?: number; reserved?: number }): Ledger {
    const reserved = delta.reserved ?? 0;
    const r = this.#s.add!.get(sessionId, kind, delta.used ?? 0, reserved, reserved) as unknown as Ledger;
    return { limit: r.limit, used: r.used, reserved: r.reserved };
  }

  setLimit(sessionId: string, kind: BudgetKind, limit: number): void {
    this.#s.limit!.run(sessionId, kind, limit);
  }

  daily(utcDay: string): Daily {
    const r = this.#s.daily!.get(utcDay) as Daily | undefined;
    return r === undefined ? { used: 0, reserved: 0 } : { used: r.used, reserved: r.reserved };
  }

  addDaily(utcDay: string, delta: { used?: number; reserved?: number }): Daily {
    const reserved = delta.reserved ?? 0;
    const r = this.#s.addDaily!.get(utcDay, delta.used ?? 0, reserved, reserved) as unknown as Daily;
    return { used: r.used, reserved: r.reserved };
  }
}
