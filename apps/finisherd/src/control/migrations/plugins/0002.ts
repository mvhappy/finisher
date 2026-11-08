// plugins 0002 (8A Task 9b, spec §5.3 step 7): `plugin_suggestions`, an agent's suggestion of a
// plugin for a project, shown to a person as a suggestion (never as an approval). It stands in
// for 5E's `pluginSuggestion` review item until reviews land (a TASKS follow-up moves the rows).
// Frozen once merged: never edit.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const plugins0002: Migration = {
  n: 2,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE plugin_suggestions (
        id TEXT PRIMARY KEY, pluginId TEXT NOT NULL, projectId TEXT NOT NULL, reason TEXT NOT NULL,
        byKind TEXT NOT NULL, byId TEXT NOT NULL, createdAt INTEGER NOT NULL) WITHOUT ROWID;
      CREATE INDEX plugin_suggestions_project ON plugin_suggestions (projectId, createdAt);
    `);
  },
};
