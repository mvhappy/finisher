// Chain `reviews`, migration 0002 (7B Task 12): `review_verdicts.session`, the session that
// judged a verdict. The `auto` merge trigger (`agent/auto-merge.ts`) checks that a critic's
// verdict comes from a session outside the authoring session's tree that started from the
// judged head; a critic verdict without one does not count (it fails closed). Sub-project 10
// writes it. An older reader ignores the column (`minReaderVersion` stays 1).
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const reviews0002: Migration = {
  n: 2,
  up: (db: DatabaseSync) => {
    db.exec("ALTER TABLE review_verdicts ADD COLUMN session TEXT");
  },
};
