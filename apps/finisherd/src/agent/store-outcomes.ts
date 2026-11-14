// The outcome ledger (plan 7A Task 11, Decision 27; spec §12.5; agent migration 0003): one row
// per `(txId, via)`, so the tracker is idempotent under repeated events and keeps the sequence
// (merged, then reverted, …), and `session_txs.outcome` holds the newest. `AgentStore.outcomes`
// owns one; the outcome tracker (`outcomes.ts`) writes it.
import type { StatementSync } from "node:sqlite";
import type { ControlStore } from "../control/db.ts";
import { finError } from "../errors.ts";
import { TX_OUTCOMES, type TxOutcome } from "./store-tables.ts";

/** How an outcome came about (5E's `review.verdict` `via`, spec §1.7 item 5). */
export type OutcomeVia = "merge" | "undo" | "revert" | "archive" | "critic" | "autoMerge";
/** Who brought it about (`review.verdict` `by`). */
export type OutcomeBy = "user" | "agent" | "critic" | "policy";

export const OUTCOME_VIAS: readonly OutcomeVia[] = ["merge", "undo", "revert", "archive", "critic", "autoMerge"];
export const OUTCOME_BYS: readonly OutcomeBy[] = ["user", "agent", "critic", "policy"];

/** One outcome of one agent transaction. */
export interface TxOutcomeRow {
  readonly txId: string;
  readonly via: OutcomeVia;
  readonly verdict: TxOutcome;
  readonly by: OutcomeBy;
  readonly sessionId: string;
  readonly at: number;
  /** Whether 5E's `review.verdict` was emitted for it. */
  readonly emitted: boolean;
  /** Beside it (agent 0008): `{rollback: {toRun, by}}` for an undo a rollback made (7B Task 8). */
  readonly details?: Readonly<Record<string, unknown>>;
}

interface Row {
  txId: string;
  via: string;
  verdict: string;
  by: string;
  sessionId: string;
  at: number;
  emitted: number;
  details: string | null;
}

const toRow = (r: Row): TxOutcomeRow => ({
  txId: r.txId,
  via: r.via as OutcomeVia,
  verdict: r.verdict as TxOutcome,
  by: r.by as OutcomeBy,
  sessionId: r.sessionId,
  at: r.at,
  emitted: r.emitted === 1,
  ...(r.details === null ? {} : { details: JSON.parse(r.details) as Record<string, unknown> }),
});

/** A transaction no merge carried out of its session's subtree. */
const UNMERGED = "NOT EXISTS (SELECT 1 FROM session_tx_outcomes o WHERE o.txId = t.txId AND o.verdict = 'merged')";

/** A `session_txs` row as the tracker needs it. */
export interface OpenTx {
  readonly txId: string;
  readonly sessionId: string;
  readonly at: number;
}

export class SessionTxOutcomes {
  readonly #cs: ControlStore;
  readonly #s: Record<string, StatementSync>;

  constructor(cs: ControlStore) {
    this.#cs = cs;
    const sql: Record<string, string> = {
      insert:
        "INSERT INTO session_tx_outcomes (txId, via, verdict, by, sessionId, at, details) VALUES (?, ?, ?, ?, ?, ?, ?) " +
        "ON CONFLICT (txId, via) DO NOTHING",
      latest: "UPDATE session_txs SET outcome = ? WHERE txId = ?",
      emitted: "UPDATE session_tx_outcomes SET emitted = 1 WHERE txId = ? AND via = ?",
      of: "SELECT txId, via, verdict, by, sessionId, at, emitted, details FROM session_tx_outcomes WHERE txId = ? ORDER BY seq",
      bySession: "SELECT txId, via, verdict, by, sessionId, at, emitted, details FROM session_tx_outcomes WHERE sessionId = ? ORDER BY seq",
      unmergedOfRoot: `SELECT t.txId, t.sessionId, t.at FROM session_txs t WHERE t.projectId = ? AND t.rootBranch = ? AND ${UNMERGED} ORDER BY t.at, t.txId`,
      unmergedOfSession: `SELECT t.txId, t.sessionId, t.at FROM session_txs t WHERE t.sessionId = ? AND ${UNMERGED} ORDER BY t.at, t.txId`,
    };
    this.#s = Object.fromEntries(Object.entries(sql).map(([k, v]) => [k, cs.db.prepare(v)]));
  }

  /**
   * Records one outcome and makes it the transaction's newest (`session_txs.outcome`), in one
   * transaction. False, changing nothing, when `(txId, via)` has one already.
   */
  record(o: Omit<TxOutcomeRow, "emitted">): boolean {
    if (!TX_OUTCOMES.includes(o.verdict)) throw finError("INVALID_REQUEST", "unknown outcome", { outcome: o.verdict });
    if (!OUTCOME_VIAS.includes(o.via)) throw finError("INVALID_REQUEST", "unknown outcome route", { via: o.via });
    if (!OUTCOME_BYS.includes(o.by)) throw finError("INVALID_REQUEST", "unknown outcome actor", { by: o.by });
    return this.#cs.tx(() => {
      const details = o.details === undefined ? null : JSON.stringify(o.details);
      if (Number(this.#s.insert!.run(o.txId, o.via, o.verdict, o.by, o.sessionId, o.at, details).changes) === 0) return false;
      this.#s.latest!.run(o.verdict, o.txId);
      return true;
    });
  }

  markEmitted(txId: string, via: OutcomeVia): void {
    this.#s.emitted!.run(txId, via);
  }

  /** A transaction's outcomes, in the order they were recorded. */
  of(txId: string): TxOutcomeRow[] {
    return (this.#s.of!.all(txId) as unknown as Row[]).map(toRow);
  }

  /** A session's outcomes, in the order they were recorded. */
  bySession(sessionId: string): TxOutcomeRow[] {
    return (this.#s.bySession!.all(sessionId) as unknown as Row[]).map(toRow);
  }

  /** The unmerged transactions of every session whose root was `root` (deleted ones too). */
  unmergedOfRoot(projectId: string, root: string): OpenTx[] {
    return this.#s.unmergedOfRoot!.all(projectId, root) as unknown as OpenTx[];
  }

  /** A session's unmerged transactions. */
  unmergedOfSession(sessionId: string): OpenTx[] {
    return this.#s.unmergedOfSession!.all(sessionId) as unknown as OpenTx[];
  }
}
