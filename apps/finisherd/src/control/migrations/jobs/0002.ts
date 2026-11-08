// jobs 0002 (5C-1, 9b review M5): `stats` keeps a finished job's run statistics (`runMicros`,
// `helpers`, `isolated`, JSON) with its terminal row, so a `job.done` re-sent at the next
// start carries them. Frozen once merged: never edit.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const jobs0002: Migration = {
  n: 2,
  up: (db: DatabaseSync) => {
    db.exec("ALTER TABLE jobs ADD COLUMN stats TEXT;");
  },
};
