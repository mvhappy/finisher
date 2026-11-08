// plugins 0003 (8A Task 9b review, M1/M2): which trust root an installed plugin came from, and
// who has plans open.
// - `plugins.rootSha256`: the SHA-256 of the root of the registry the installed version was
//   verified under. A registry is named by its id, but trust belongs to the root: removing a
//   registry and adding another folder under the same id must not let updates or automatic
//   updates come from the new root unnoticed. Rows made before this step take the root their
//   registry has now (the only root they can have come from); a row whose registry is gone
//   stays NULL and so matches no registry.
// - `plugin_plans.rootSha256`: the root the plan's package was verified under, re-checked at
//   approval. `unusable`: a plan a non-human principal made keeps its row (so an approval of it
//   is refused as `HUMAN_ONLY`) but its staged files are gone at once.
// - indexes for the per-principal caps on open plans and suggestions.
// Frozen once merged: never edit.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const plugins0003: Migration = {
  n: 3,
  up: (db: DatabaseSync) => {
    db.exec(`
      ALTER TABLE plugins ADD COLUMN rootSha256 TEXT;
      UPDATE plugins SET rootSha256 = (SELECT r.rootSha256 FROM registries r WHERE r.id = plugins.registryId)
        WHERE registryId IS NOT NULL;
      ALTER TABLE plugin_plans ADD COLUMN rootSha256 TEXT;
      ALTER TABLE plugin_plans ADD COLUMN unusable INTEGER NOT NULL DEFAULT 0;
      CREATE INDEX plugin_plans_maker ON plugin_plans (byKind, byId);
      CREATE INDEX plugin_suggestions_maker ON plugin_suggestions (byKind, byId, projectId);
    `);
  },
};
