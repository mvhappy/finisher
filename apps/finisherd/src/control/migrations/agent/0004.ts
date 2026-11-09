// agent 0004 (7B Task 2, plan 7B Decisions 4, 6 and 17; spec §7.2, §9.4). Frozen once merged:
// never edit, add 0005. The chain's `minReaderVersion` is 4 from this step (`store.ts`): a
// server before it would open the database and silently enforce none of these rules.
//
// - `agent_rules`: the `local` layer of command rules (each person's own, never leaves the
//   server): `{id, projectId?, sessionId?, effect, command, where (JSON), source, createdBy,
//   createdAt}`. A row with neither id applies to every agent call; `projectId` narrows it to
//   one project, `sessionId` to one session.
// - `project_trust`: one row per project a person trusted: the hash of its `finisher.rules.json`
//   when it was trusted (`none` when it had none), who and when. 7F adds `hooksHash`'s value.
// - `sessions.rootSession` and `sessions.depth` for 7B's children: rows made before this step
//   are their own root at depth 0, and so is every new row whose writer names no root (the
//   trigger), until Task 6 writes both.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0004: Migration = {
  n: 4,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE agent_rules (
        id TEXT PRIMARY KEY, projectId TEXT, sessionId TEXT, effect TEXT NOT NULL, command TEXT NOT NULL,
        "where" TEXT, source TEXT NOT NULL, createdBy TEXT, createdAt INTEGER NOT NULL) WITHOUT ROWID;
      CREATE INDEX agent_rules_project ON agent_rules (projectId, createdAt);
      CREATE TABLE project_trust (
        projectId TEXT PRIMARY KEY, rulesHash TEXT NOT NULL, hooksHash TEXT, trustedAt INTEGER NOT NULL,
        by TEXT NOT NULL) WITHOUT ROWID;
      ALTER TABLE sessions ADD COLUMN rootSession TEXT;
      ALTER TABLE sessions ADD COLUMN depth INTEGER NOT NULL DEFAULT 0;
      UPDATE sessions SET rootSession = id;
      CREATE TRIGGER sessions_root_default AFTER INSERT ON sessions WHEN NEW.rootSession IS NULL
        BEGIN UPDATE sessions SET rootSession = NEW.id WHERE id = NEW.id; END;
    `);
  },
};
