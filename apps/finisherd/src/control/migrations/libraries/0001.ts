// libraries 0001 (5D Task 8, Decisions 15, 17, 48): the registry of libraries. Frozen once
// merged: never edit, add the next step. Catalogs are not here (they are per-library files,
// Decision 23); no foreign key leaves the chain.
//
// - `nameKey`, `rootKey`: the case-folded NFC lookup keys of the name and of the canonical
//   root (`fin-node` `libraryCanonicalRoot`); unique, overlap by prefix is checked in code;
// - `aliases`, `include`, `exclude`, `kinds`, `scan`, `stages`, `volume`: JSON text;
// - `storage`: the *detected* kind (`local`, `network`, `sync:<provider>`); `storageOverride`
//   the owner's choice (`local | network | sync`), which wins;
// - `state`: `online | offline | scanning | paused`; `gen`: the last full-scan generation.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const libraries0001: Migration = {
  n: 1,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE libraries (
        id TEXT PRIMARY KEY, name TEXT NOT NULL, nameKey TEXT NOT NULL UNIQUE,
        root TEXT NOT NULL, rootKey TEXT NOT NULL UNIQUE, aliases TEXT NOT NULL, volume TEXT NOT NULL,
        storage TEXT NOT NULL, storageOverride TEXT, state TEXT NOT NULL, offlineReason TEXT,
        include TEXT, exclude TEXT NOT NULL, kinds TEXT NOT NULL, scan TEXT NOT NULL,
        hydrate INTEGER NOT NULL, bandwidthMiBs REAL, stages TEXT NOT NULL, gen INTEGER NOT NULL,
        createdBy TEXT NOT NULL, createdAt INTEGER NOT NULL, lastPollAt INTEGER, lastFullAt INTEGER);
    `);
  },
};
