// agent 0002 (7A-2 review of Tasks 3b, 6b and 7). Frozen once merged: never edit, add 0003.
//
// - `sessions.readyAt`: when the session first became `idle` (its credential was minted
//   before). A resume mints a missing credential only for a session that never got there (a
//   crash between its record and the mint); one that did had a credential that ended. Rows
//   made before this step are taken as ready (the safe side: no credential is minted again).
// - `session_txs.projectId`: the session's project, so its transactions outlive the session
//   (the outcome tracker attributes late merges) and are pruned with the project or by an
//   explicit purge only.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0002: Migration = {
  n: 2,
  up: (db: DatabaseSync) => {
    db.exec(`
      ALTER TABLE sessions ADD COLUMN readyAt INTEGER;
      UPDATE sessions SET readyAt = createdAt;
      ALTER TABLE session_txs ADD COLUMN projectId TEXT;
      UPDATE session_txs SET projectId = (SELECT projectId FROM sessions WHERE sessions.id = session_txs.sessionId);
      CREATE INDEX session_txs_project ON session_txs (projectId) WHERE projectId IS NOT NULL;
    `);
  },
};
