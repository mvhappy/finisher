// agent 0008 (7B Task 8, plan 7B Decision 23; spec §5.3). Frozen once merged: never edit, add
// 0009.
//
// - `nodes.rolledBack`: 1 for a node of a run a rollback took back; kept for display and audit,
//   left out of the built-in loop's next request (`historyFromNodes`).
// - `session_txs.runId`: the run a transaction was made in (the session's active run, else its
//   last), so a rollback takes exactly the later runs' transactions. Rows before this step
//   have none; a rollback places them by time (`at` against the runs' `startedAt`).
// - `session_tx_outcomes.details`: JSON beside an outcome (`{rollback: {toRun, by}}` for an
//   undo a rollback made).
//
// A server before this step would send a rolled-back conversation to the model again, its
// transactions undone under it, so the chain's `minReaderVersion` is 8 from this step.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0008: Migration = {
  n: 8,
  up: (db: DatabaseSync) => {
    db.exec(`
      ALTER TABLE nodes ADD COLUMN rolledBack INTEGER NOT NULL DEFAULT 0;
      ALTER TABLE session_txs ADD COLUMN runId TEXT;
      ALTER TABLE session_tx_outcomes ADD COLUMN details TEXT;
    `);
  },
};
