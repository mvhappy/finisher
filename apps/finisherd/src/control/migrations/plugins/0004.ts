// plugins 0004 (sp8b Task 5, spec §3.3): why an installed code plugin does not run.
// - `plugins.problem`: a code (`PLUGIN_CRASH_LOOP` after three crashes in ten minutes; 8B Task
//   16 adds `SANDBOX_UNAVAILABLE`), NULL while it runs. Only the owner's `plugin.enable` clears it.
// `state` already admits `crashLoop` (0001's CHECK), so no table rebuild is needed.
// Frozen once merged: never edit.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const plugins0004: Migration = {
  n: 4,
  up: (db: DatabaseSync) => {
    db.exec(`ALTER TABLE plugins ADD COLUMN problem TEXT;`);
  },
};
