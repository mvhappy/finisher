// agent 0011 (plan 7B Task 11, Decision 25; spec §5.4). Frozen once merged: never edit, add 0012.
// The column is additive, so `minReaderVersion` stays 8.
//
// - `session_state.reviews`: the `branchReview` items a session asked for (`review.request` from
//   its credential, for a branch of its subtree), as a JSON array of item ids, oldest first.
//   A session that ends with one recorded does not emit `branchReady` again (`review-link.ts`).
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0011: Migration = {
  n: 11,
  up: (db: DatabaseSync) => {
    db.exec("ALTER TABLE session_state ADD COLUMN reviews TEXT NOT NULL DEFAULT '[]';");
  },
};
