// catalog `core` 0001 (plan 5D Task 7a, Decision 23): `meta`, `migrations` and 5D's tables.
// Frozen once merged: never edit, add the next step.
//
// - `(parent, name)` and `(dirId, name)` are unique on the **raw** name bytes; `relKey` (the
//   NFC, maybe case-folded lookup key) is a plain index, since keys may collide (Decision 11).
// - `names` is a contentless FTS5 table (rowid = `files.id`) over the display name and the
//   directory's key. Task 7a measured it on the 100k tree at ~126 bytes per file with the
//   `dir` column, so directory tokens stay there (no `dirnames` table: it would save tens of
//   bytes); what did pass the budget was the probe JSON, hence `probe BLOB`
//   (`core/probe-codec.ts`: deflated, the row's own fields left out).
// - `migrations(chain, n)`: the column is `chain`, as in `control.db`, so the same runner
//   (`control/migrations.ts`) applies every extension's chain.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../../../control/migrations.ts";

export const catalogCore0001: Migration = {
  n: 1,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
      CREATE TABLE IF NOT EXISTS migrations (
        chain TEXT NOT NULL, n INTEGER NOT NULL, appliedAt INTEGER NOT NULL, PRIMARY KEY (chain, n)
      ) STRICT;
      CREATE TABLE dirs (id INTEGER PRIMARY KEY, parent INTEGER, name BLOB NOT NULL, nameNfc TEXT NOT NULL,
        relKey TEXT NOT NULL, mtimeNs INTEGER, fileId BLOB, devId TEXT, mount TEXT, placeholder TEXT,
        collision INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL, code TEXT, emptySince INTEGER,
        listedGen INTEGER, lastSeen INTEGER NOT NULL, UNIQUE (parent, name));
      CREATE INDEX dirs_key ON dirs (relKey);
      CREATE TABLE files (id INTEGER PRIMARY KEY, dirId INTEGER NOT NULL REFERENCES dirs (id), name BLOB NOT NULL,
        nameNfc TEXT NOT NULL, nameLossy INTEGER NOT NULL DEFAULT 0, relKey TEXT NOT NULL, ext TEXT,
        collision INTEGER NOT NULL DEFAULT 0, hardlink INTEGER NOT NULL DEFAULT 0, size INTEGER NOT NULL,
        mtimeNs INTEGER NOT NULL, fileId BLOB, placeholder TEXT, fp TEXT, fullHash TEXT, kind TEXT, probe BLOB,
        probeVersion INTEGER, durationMs INTEGER, width INTEGER, height INTEGER, state TEXT NOT NULL, code TEXT,
        renameOf INTEGER, firstSeen INTEGER NOT NULL, lastSeen INTEGER NOT NULL, lastChange INTEGER NOT NULL,
        missingSince INTEGER, UNIQUE (dirId, name));
      CREATE INDEX files_key ON files (relKey);
      CREATE INDEX files_fp ON files (fp) WHERE fp IS NOT NULL;
      CREATE INDEX files_hash ON files (fullHash) WHERE fullHash IS NOT NULL;
      CREATE INDEX files_fileid ON files (fileId) WHERE fileId IS NOT NULL;
      CREATE INDEX files_pending ON files (state, probeVersion);
      CREATE INDEX files_media ON files (kind, durationMs, width, height);
      CREATE INDEX files_size ON files (size);
      CREATE TABLE stages (fp TEXT NOT NULL, stage TEXT NOT NULL, version TEXT NOT NULL, state TEXT NOT NULL,
        code TEXT, jobId TEXT, attempts INTEGER NOT NULL DEFAULT 0, at INTEGER NOT NULL, PRIMARY KEY (fp, stage)) WITHOUT ROWID;
      CREATE INDEX stages_state ON stages (stage, state, at);
      CREATE VIRTUAL TABLE names USING fts5 (name, dir, content = '', contentless_delete = 1,
        tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
      CREATE TABLE scans (id INTEGER PRIMARY KEY, jobId TEXT, mode TEXT NOT NULL, trigger TEXT NOT NULL, gen INTEGER NOT NULL,
        startedAt INTEGER NOT NULL, finishedAt INTEGER, outcome TEXT, code TEXT, counts TEXT);
      INSERT INTO dirs (id, parent, name, nameNfc, relKey, state, lastSeen) VALUES (1, NULL, x'', '', '', 'present', 0);
    `);
  },
};
