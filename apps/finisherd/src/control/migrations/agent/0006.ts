// agent 0006 (7B Task 5, plan 7B Decisions 4 and 12, owner decision 2026-11-08; spec §7.3).
// Frozen once merged: never edit, add 0007. `agent_remembered`: the server's own record of each
// rule a person's `remember` wrote (`remember.ts`), the proof `rules.ts` asks of an `allow`
// before it lifts an `ask`: the rule's id, its layer (`local`: a row of `agent_rules`;
// `project`: a rule of the project's `finisher.rules.json`), the project, the session (a
// `session`-scoped local rule only: the row goes with the session), the exact command and the
// approval it answered. A rule's `source` is display only and never proves anything. A server
// before this step lifts nothing (it ignores the table: stricter), so `minReaderVersion` stays 4.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0006: Migration = {
  n: 6,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE agent_remembered (
        ruleId TEXT NOT NULL, layer TEXT NOT NULL, projectId TEXT NOT NULL, sessionId TEXT, command TEXT NOT NULL,
        approvalId TEXT NOT NULL, createdAt INTEGER NOT NULL, PRIMARY KEY (layer, projectId, ruleId)) WITHOUT ROWID;
      CREATE INDEX agent_remembered_session ON agent_remembered (sessionId) WHERE sessionId IS NOT NULL;
    `);
  },
};
