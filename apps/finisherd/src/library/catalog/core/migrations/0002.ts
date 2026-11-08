// catalog `core` 0002 (plan 5D Task 7b, Decisions 22, 25): what the diff needs beyond 0001.
// Frozen once merged: never edit, add the next step.
//
// - `files.dev`: the entry's device, so file ids are rename hints keyed `dev:fileId`
//   (a file id alone repeats across nested mounts);
// - `dirs.emptyJob`: the job whose empty listing set `emptySince`, since only another job's
//   empty listing at least 60 s later confirms a drop to zero;
// - `files_rename`: the pending renames a missing row may resolve.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../../../control/migrations.ts";

export const catalogCore0002: Migration = {
  n: 2,
  up: (db: DatabaseSync) => {
    db.exec(`
      ALTER TABLE files ADD COLUMN dev TEXT;
      ALTER TABLE dirs ADD COLUMN emptyJob TEXT;
      CREATE INDEX files_rename ON files (renameOf) WHERE renameOf IS NOT NULL;
    `);
  },
};
