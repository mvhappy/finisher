// jobs 0001 (5C-1 Task 7, Decision 25): `jobs` and `job_items`. Frozen once merged: never edit.
// No foreign key leaves the chain; `isolation` holds Decision 32's state (JSON), `phase` is
// `run` or `hooks` (Decision 40).
// One edit after merge, while Finisher had no users: `jobs_dedupe` gained the owner and
// `createdAt` (5C-2 scoped dedupe to the owner, and `jobs_owner_created` then won the plan).
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const jobs0001: Migration = {
  n: 1,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE jobs (
        id TEXT PRIMARY KEY, kind TEXT NOT NULL, kindVersion INTEGER NOT NULL, family TEXT NOT NULL,
        params TEXT NOT NULL, ownerKind TEXT NOT NULL, ownerId TEXT NOT NULL, via TEXT, rights TEXT,
        project TEXT, library TEXT, parent TEXT, priority TEXT NOT NULL, state TEXT NOT NULL,
        phase TEXT NOT NULL DEFAULT 'run',
        pausedFrom TEXT, warm INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0,
        maxAttempts INTEGER NOT NULL, idemKey TEXT, dedupeKey TEXT, checkpoint TEXT, extra TEXT,
        result TEXT, error TEXT, isolation TEXT, createdAt INTEGER NOT NULL, startedAt INTEGER,
        finishedAt INTEGER, nextRunAt INTEGER);
      CREATE UNIQUE INDEX jobs_idem ON jobs (ownerKind, ownerId, idemKey) WHERE idemKey IS NOT NULL;
      CREATE INDEX jobs_dedupe ON jobs (kind, dedupeKey, ownerKind, ownerId, createdAt) WHERE dedupeKey IS NOT NULL;
      CREATE INDEX jobs_active ON jobs (state, priority, createdAt) WHERE finishedAt IS NULL;
      CREATE INDEX jobs_finished ON jobs (finishedAt) WHERE finishedAt IS NOT NULL;
      CREATE INDEX jobs_owner ON jobs (ownerKind, ownerId, createdAt);
      CREATE TABLE job_items (jobId TEXT NOT NULL REFERENCES jobs (id), item TEXT NOT NULL,
        value TEXT, error TEXT, PRIMARY KEY (jobId, item)) WITHOUT ROWID;
    `);
  },
};
