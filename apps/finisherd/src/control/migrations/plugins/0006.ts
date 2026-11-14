// plugins 0006 (5E Task 8 review, Decision 11): `plugin_suggestions` is gone. Suggestions are
// `pluginSuggestion` review items; nothing wrote the table in a release, so no rows move.
// Frozen once merged: never edit.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const plugins0006: Migration = {
  n: 6,
  up: (db: DatabaseSync) => {
    db.exec(`DROP TABLE IF EXISTS plugin_suggestions;`);
  },
};
