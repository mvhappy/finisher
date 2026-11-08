// agent 0003 (7A-2 Task 11, the outcome tracker; plan 7A Decision 27, spec §12.5). Frozen once
// merged: never edit, add 0004.
//
// - `session_txs.rootBranch`: the session's root branch id when the row was written, so a
//   merge that lands after the session is gone is still judged against its subtree (rows made
//   before this step take it from their session, when it is still there).
// - `session_tx_outcomes`: one row per `(txId, via)` (one verdict per transaction and route;
//   a later verdict does not erase an earlier one: the dataset builder reads the sequence,
//   `seq`, the order they were recorded in),
//   with who (`by`), when, and whether 5E's `review.verdict` was emitted for it. Kept and
//   pruned with their `session_txs` rows (`AgentStore.purgeTxs`).
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0003: Migration = {
  n: 3,
  up: (db: DatabaseSync) => {
    db.exec(`
      ALTER TABLE session_txs ADD COLUMN rootBranch TEXT;
      UPDATE session_txs SET rootBranch = (SELECT rootBranch FROM sessions WHERE sessions.id = session_txs.sessionId);
      CREATE TABLE session_tx_outcomes (
        seq INTEGER PRIMARY KEY, txId TEXT NOT NULL, via TEXT NOT NULL, verdict TEXT NOT NULL,
        by TEXT NOT NULL, sessionId TEXT NOT NULL, at INTEGER NOT NULL, emitted INTEGER NOT NULL DEFAULT 0);
      CREATE UNIQUE INDEX session_tx_outcomes_tx ON session_tx_outcomes (txId, via);
      CREATE INDEX session_tx_outcomes_session ON session_tx_outcomes (sessionId, seq);
    `);
  },
};
