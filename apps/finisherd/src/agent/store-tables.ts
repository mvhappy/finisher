// The agent store's side tables (plan 7A Decisions 1, 14a, 21; spec §5.1, §5.2): `session_txs`,
// `session_state` and the budget ledger (`budgets`, `budget_days`, 7B's `budget_carves`).
// `AgentStore` owns one of each and answers them; nothing else touches these tables.
import type { StatementSync } from "node:sqlite";
import type { Attribution } from "@finisher/core-types";
import type { ControlStore } from "../control/db.ts";
import { finError } from "../errors.ts";
import type { FinError } from "@finisher/contracts";
import { type Carve, type CarvedKind, carveIn, type CarveRecords, type CarveRow, type Raise, raiseIn, raiseRefused, settleIn } from "./carve.ts";
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
  /** The newest outcome (`session_tx_outcomes` holds them all; Task 11). */
  readonly outcome?: TxOutcome;
  /** The session's project and root branch when it was written (agent 0002, 0003): they outlive the session. */
  readonly projectId?: string;
  readonly rootBranch?: string;
  /** The run it was made in (agent 0008): the session's active run, else its last. */
  readonly runId?: string;
}

interface TxRow {
  txId: string;
  sessionId: string;
  callId: string | null;
  attribution: string;
  at: number;
  outcome: string | null;
  projectId: string | null;
  rootBranch: string | null;
  runId: string | null;
}

const TX_COLUMNS = "txId, sessionId, callId, attribution, at, outcome, projectId, rootBranch, runId";

function toTx(r: TxRow): SessionTx {
  return {
    txId: r.txId,
    sessionId: r.sessionId,
    callId: r.callId,
    attribution: r.attribution as Attribution,
    at: r.at,
    ...(r.outcome === null ? {} : { outcome: r.outcome as TxOutcome }),
    ...(r.projectId === null ? {} : { projectId: r.projectId }),
    ...(r.rootBranch === null ? {} : { rootBranch: r.rootBranch }),
    ...(r.runId === null ? {} : { runId: r.runId }),
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
      // The project and root are the session's, kept on the row: the row outlives its session
      // (agent 0002, 0003); the run is its active one, else its last (0008).
      insert:
        "INSERT INTO session_txs (txId, sessionId, callId, attribution, at, projectId, rootBranch, runId) " +
        "VALUES (?, ?, ?, ?, ?, (SELECT projectId FROM sessions WHERE id = ?), (SELECT rootBranch FROM sessions WHERE id = ?), " +
        "COALESCE((SELECT runId FROM sessions WHERE id = ?), (SELECT id FROM runs WHERE sessionId = ? ORDER BY seq DESC LIMIT 1))) " +
        "ON CONFLICT (txId) DO NOTHING",
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
  record(t: Omit<SessionTx, "outcome" | "projectId" | "rootBranch" | "runId">): void {
    if (!ATTRIBUTIONS.includes(t.attribution)) throw finError("INVALID_REQUEST", "unknown attribution", { attribution: t.attribution });
    this.#s.insert!.run(t.txId, t.sessionId, t.callId, t.attribution, t.at, t.sessionId, t.sessionId, t.sessionId, t.sessionId);
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

/** `CarveRecords` over `budget_carves` (7B Task 7, agent 0007). */
class DbCarves implements CarveRecords {
  readonly #s: Record<string, StatementSync>;

  constructor(cs: ControlStore) {
    this.#s = prepare(cs, {
      open: "SELECT sessionId, parentId, kind, amount FROM budget_carves WHERE sessionId = ? AND settled = 0 ORDER BY kind",
      record: "INSERT INTO budget_carves (sessionId, kind, parentId, amount) VALUES (?, ?, ?, ?)",
      grow: "UPDATE budget_carves SET amount = amount + ? WHERE sessionId = ? AND kind = ? AND settled = 0",
      settled: "UPDATE budget_carves SET settled = 1 WHERE sessionId = ? AND settled = 0",
      heldBy: "SELECT COALESCE(SUM(amount), 0) AS n FROM budget_carves WHERE parentId = ? AND kind = ? AND settled = 0",
      pass: "UPDATE budget_carves SET parentId = ? WHERE parentId = ? AND kind = ? AND settled = 0",
      // Recovery: every open carve held on its parent again, then the children no longer live.
      rehold:
        "UPDATE budgets SET reserved = (SELECT SUM(c.amount) FROM budget_carves c WHERE c.parentId = budgets.sessionId AND c.kind = budgets.kind AND c.settled = 0) " +
        "WHERE EXISTS (SELECT 1 FROM budget_carves c WHERE c.parentId = budgets.sessionId AND c.kind = budgets.kind AND c.settled = 0)",
      // Deepest first, by the chain of open carves (a gone child has no `sessions.depth`).
      notLive:
        "WITH RECURSIVE up (sessionId, at, d) AS (SELECT DISTINCT sessionId, parentId, 1 FROM budget_carves WHERE settled = 0 " +
        "UNION SELECT up.sessionId, c.parentId, up.d + 1 FROM up JOIN budget_carves c ON c.sessionId = up.at AND c.settled = 0) " +
        "SELECT up.sessionId FROM up LEFT JOIN sessions s ON s.id = up.sessionId WHERE s.id IS NULL OR s.status = 'ended' " +
        "GROUP BY up.sessionId ORDER BY MAX(up.d) DESC, up.sessionId",
    });
  }

  open(sessionId: string): CarveRow[] {
    return this.#s.open!.all(sessionId) as unknown as CarveRow[];
  }

  record(r: CarveRow): void {
    this.#s.record!.run(r.sessionId, r.kind, r.parentId, r.amount);
  }

  grow(sessionId: string, kind: CarvedKind, by: number): void {
    this.#s.grow!.run(by, sessionId, kind);
  }

  settled(sessionId: string): void {
    this.#s.settled!.run(sessionId);
  }

  heldBy(parentId: string, kind: CarvedKind): number {
    return (this.#s.heldBy!.get(parentId, kind) as { n: number }).n;
  }

  pass(from: string, to: string, kind: CarvedKind): void {
    this.#s.pass!.run(to, from, kind);
  }

  /** After the per-call reservations were returned: the open carves held again; the children no longer live. */
  rehold(): string[] {
    this.#s.rehold!.run();
    return (this.#s.notLive!.all() as Array<{ sessionId: string }>).map((r) => r.sessionId);
  }
}

/** `LedgerStore` over `budgets` and `budget_days` (Task 4's interface; each call one statement), and 7B's carves. */
export class DbLedger implements LedgerStore {
  readonly #s: Record<string, StatementSync>;
  readonly #cs: ControlStore;
  readonly #carves: DbCarves;

  constructor(cs: ControlStore) {
    this.#cs = cs;
    this.#carves = new DbCarves(cs);
    this.#s = prepare(cs, {
      clearReserved: "UPDATE budgets SET reserved = 0 WHERE reserved != 0",
      clearDailyReserved: "UPDATE budget_days SET reserved = 0 WHERE reserved != 0",
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
  carve(parentId: string, sessionId: string, carve: Carve): void {
    this.#cs.tx(() => carveIn(this, this.#carves, parentId, sessionId, carve));
  }

  settleCarve(sessionId: string): void {
    this.#cs.tx(() => settleIn(this, this.#carves, sessionId));
  }

  raise(sessionId: string, add: Raise): void {
    this.#cs.tx(() => raiseIn(this, this.#carves, sessionId, add));
  }

  canRaise(sessionId: string, add: Raise): FinError | undefined {
    return raiseRefused(this, this.#carves, sessionId, add);
  }

  tx<A>(f: () => A): A {
    return this.#cs.tx(f);
  }

  /**
   * Recovery (spec §13.1; the caller's transaction, before anything else runs): every per-call
   * reservation returns, every open carve is held on its parent again, and each carve whose
   * child has ended or is gone is settled.
   */
  recover(): void {
    this.#s.clearReserved!.run();
    this.#s.clearDailyReserved!.run();
    for (const id of this.#carves.rehold()) settleIn(this, this.#carves, id);
  }
}

/**
 * The session tree (7B Task 6): parents, children and what is live, over `sessions`
 * (`parent`, `rootSession`, `depth`, agent migration 0004), and a session's last assistant
 * message (a child's summary).
 */
export class SessionTree {
  readonly #s: Record<string, StatementSync>;

  constructor(store: ControlStore) {
    const sql: Record<string, string> = {
      children: "SELECT id FROM sessions WHERE parent = ? ORDER BY id",
      liveChildren: "SELECT id FROM sessions WHERE parent = ? AND status != 'ended' ORDER BY id",
      liveBelow: "SELECT COUNT(*) AS n FROM sessions WHERE rootSession = ? AND id != ? AND status != 'ended'",
      liveBelowIds: "SELECT id FROM sessions WHERE rootSession = ? AND id != ? AND status != 'ended' ORDER BY depth DESC, id",
      liveRoots: "SELECT id, rootBranch FROM sessions WHERE projectId = ? AND status != 'ended' AND rootBranch IS NOT NULL",
      active: "SELECT id, harness FROM sessions WHERE status IN ('running', 'waitingApproval')",
      lastAssistant: "SELECT seq FROM nodes WHERE sessionId = ? AND kind = 'assistantMessage' ORDER BY seq DESC LIMIT 1",
    };
    this.#s = Object.fromEntries(Object.entries(sql).map(([k, q]) => [k, store.db.prepare(q)]));
  }

  #ids(name: string, ...args: string[]): string[] {
    return (this.#s[name]!.all(...args) as { id: string }[]).map((r) => r.id);
  }

  /** Every child of `parent`, ended ones too, oldest first. */
  children(parent: string): string[] {
    return this.#ids("children", parent);
  }

  /** The children of `parent` that have not ended. */
  liveChildren(parent: string): string[] {
    return this.#ids("liveChildren", parent);
  }

  /** How many sessions below the root session `root` have not ended. */
  liveDescendants(root: string): number {
    return (this.#s.liveBelow!.get(root, root) as { n: number }).n;
  }

  /** The sessions below `root` that have not ended, deepest first. */
  liveDescendantIds(root: string): string[] {
    return this.#ids("liveBelowIds", root, root);
  }

  /** The root branches of `projectId`'s sessions that have not ended (Decision 18). */
  liveRoots(projectId: string): Array<{ readonly id: string; readonly rootBranch: string }> {
    return this.#s.liveRoots!.all(projectId) as Array<{ id: string; rootBranch: string }>;
  }

  /** The sessions in a run (`running`, `waitingApproval`) and their harnesses (Decision 19). */
  active(): Array<{ readonly id: string; readonly harness: string }> {
    return this.#s.active!.all() as Array<{ id: string; harness: string }>;
  }

  /** The seq of the session's last `assistantMessage` node, if any. */
  lastAssistantSeq(sessionId: string): number | undefined {
    return (this.#s.lastAssistant!.get(sessionId) as { seq: number } | undefined)?.seq;
  }
}
