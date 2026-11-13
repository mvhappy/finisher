// agent 0012 (plan 7B Task 12, review finding 4; spec §6.5). Frozen once merged: never edit, add 0013.
// The column is additive, so `minReaderVersion` stays 8.
//
// - `sessions.baseHash`: the hash of the base branch's head the session started from, recorded
//   for a critic started on a branch under review (the reviewed head's hash, from a review plan
//   of it). The `auto` merge trigger compares seq and hash with the critic verdict's snapshot,
//   so a rewind to the same seq with other records is `criticStale`. Null for every other session.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0012: Migration = {
  n: 12,
  up: (db: DatabaseSync) => {
    db.exec("ALTER TABLE sessions ADD COLUMN baseHash TEXT;");
  },
};
