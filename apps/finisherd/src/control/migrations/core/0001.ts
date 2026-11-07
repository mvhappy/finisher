// core 0001 (5A Task 1): `meta`, `migrations`, `server_ids`, `audit`. Frozen: never edit.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const core0001: Migration = {
  n: 1,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
      CREATE TABLE IF NOT EXISTS migrations (
        chain TEXT NOT NULL, n INTEGER NOT NULL, appliedAt INTEGER NOT NULL, PRIMARY KEY (chain, n)
      ) STRICT;
      CREATE TABLE server_ids (serverId TEXT PRIMARY KEY, envId TEXT NOT NULL, since INTEGER NOT NULL) STRICT;
      CREATE TABLE audit (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        at INTEGER NOT NULL,
        kind TEXT NOT NULL,
        principalKind TEXT NOT NULL,
        principalId TEXT NOT NULL,
        credentialKind TEXT,
        outcome TEXT NOT NULL,
        code TEXT,
        remote TEXT,
        count INTEGER NOT NULL DEFAULT 1,
        details TEXT NOT NULL DEFAULT '{}'
      ) STRICT;
      CREATE INDEX audit_at ON audit (at);
      CREATE INDEX audit_kind ON audit (kind, id);
    `);
  },
};
