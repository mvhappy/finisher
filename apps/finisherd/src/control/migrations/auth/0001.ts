// auth 0001 (5A Task 9, Decision 32): agent and MCP credentials. Frozen once merged: never edit,
// add the next step. No foreign key leaves the chain.
//
// - `scopes`, `projects`, `branches`: JSON text (`projects`/`branches` are `"*"` or a list);
// - `createdBy`: `<kind>:<id>` of the principal that minted it (the console);
// - times in ms since the epoch; `idleExpiryS` in seconds; `absoluteExpiryAt` `NULL` for none;
// - `revokedAt`/`revokedReason` (`owner | self | expired`) end it; nothing else does.
// A credential's key is never stored: it is derived from `credential.master` (Decision 6).
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const auth0001: Migration = {
  n: 1,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE credentials (
        id TEXT PRIMARY KEY, kind TEXT NOT NULL, label TEXT NOT NULL,
        scopes TEXT NOT NULL, projects TEXT NOT NULL, branches TEXT NOT NULL,
        createdBy TEXT NOT NULL, createdAt INTEGER NOT NULL, lastUsedAt INTEGER,
        idleExpiryS INTEGER NOT NULL, absoluteExpiryAt INTEGER,
        revokedAt INTEGER, revokedReason TEXT) STRICT;
    `);
  },
};
