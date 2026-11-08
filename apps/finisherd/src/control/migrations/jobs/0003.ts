// jobs 0003 (5C-1 Task 12, the store budget): `list`'s order (`createdAt DESC, id DESC`) read
// from an index instead of a sort of every row (6 ms p50 at 10,000 jobs, 0.25 ms with it), and
// the owner's list the same way. Indexes only: an older reader still reads the chain. Frozen
// once merged: never edit.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const jobs0003: Migration = {
  n: 3,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE INDEX jobs_created ON jobs (createdAt, id);
      DROP INDEX jobs_owner;
      CREATE INDEX jobs_owner_created ON jobs (ownerKind, ownerId, createdAt, id);
    `);
  },
};
