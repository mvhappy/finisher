// agent 0010 (plan 7D Task 4, Decision 2; spec §4.5). Frozen once merged: never edit, add 0011.
// The column is additive, so `minReaderVersion` stays 8.
//
// - `sessions.tools`: the role's tool allow-set the session started with, as a JSON array of
//   names (`SessionPlan.tools`), so a resumed Claude Code instance gets the same `--tools` as
//   its first one even when the role file changed since. NULL for a session that recorded none
//   (the built-in agent's before 7D). No backfill: there are no users yet.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0010: Migration = {
  n: 10,
  up: (db: DatabaseSync) => {
    db.exec("ALTER TABLE sessions ADD COLUMN tools TEXT;");
  },
};
