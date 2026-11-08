// catalog `core` 0002 (plan 5D Task 7b, Decisions 22, 25): what the diff needs beyond 0001.
// Frozen once merged: never edit, add the next step.
//
// - `files.dev`: the entry's device, so file ids are rename hints keyed `dev:fileId`
//   (a file id alone repeats across nested mounts);
// - `dirs.emptyJob`: the job whose empty listing set `emptySince`, since only another job's
//   empty listing at least 60 s later confirms a drop to zero;
// - `dirs.listedAt`: the start of the listing whose `mtimeNs` and `listedGen` are recorded, so
//   an older listing applied late never overwrites a newer one's;
// - `files.missingGen`: the generation in which the row went missing, so `purgeMissing` needs
//   full scans since as well as days (a forward clock jump alone purges nothing);
// - `dirs.entries`, `dirs.volume`: how many entries the recorded listing held and the nested
//   mount's volume it reported (`DirDone.volume`, JSON), both handed back in `in/dirs.ndjson`
//   so the walker can apply its N → 0 and nested-mount rules;
// - `files_rename`: the pending renames a missing row may resolve;
// - `files_order`: the stage pump's `pending` order (`state`, `lastChange`, `id`).
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../../../control/migrations.ts";

export const catalogCore0002: Migration = {
  n: 2,
  up: (db: DatabaseSync) => {
    db.exec(`
      ALTER TABLE files ADD COLUMN dev TEXT;
      ALTER TABLE files ADD COLUMN missingGen INTEGER;
      ALTER TABLE dirs ADD COLUMN emptyJob TEXT;
      ALTER TABLE dirs ADD COLUMN listedAt INTEGER;
      ALTER TABLE dirs ADD COLUMN entries INTEGER;
      ALTER TABLE dirs ADD COLUMN volume TEXT;
      CREATE INDEX files_rename ON files (renameOf) WHERE renameOf IS NOT NULL;
      CREATE INDEX files_order ON files (state, lastChange, id);
    `);
  },
};
