// plugins 0001 (8A Task 9a, spec §5.1): `registries`, `plugins`, `plugin_grants` and
// `plugin_plans`. Frozen once merged: never edit. No foreign key leaves the chain.
//
// - `registries`: the index of the local-folder registries; the folder and the trust live in
//   `<server-data>/plugins/{registries,trust}` (fin-registry owns them), this table adds who
//   added each and what the last refresh found.
// - `plugins`: one row per installed plugin (one version is current; an update installs beside
//   and switches). `dir` is the package directory relative to `<server-data>/plugins`.
// - `plugin_grants`: the permissions a person approved, one row each, for one version.
// - `plugin_plans`: install plans waiting for approval (Task 9b); `base` is the installed
//   version and grants the plan was made against, re-checked at approval (review M2).
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const plugins0001: Migration = {
  n: 1,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE registries (
        id TEXT PRIMARY KEY, kind TEXT NOT NULL, rootSha256 TEXT NOT NULL, freshness TEXT NOT NULL,
        addedBy TEXT NOT NULL, addedAt INTEGER NOT NULL,
        lastRefreshAt INTEGER, lastRefreshBy TEXT, lastRefreshByHuman INTEGER, lastRefreshOk INTEGER,
        lastRefreshCode TEXT) WITHOUT ROWID;
      CREATE TABLE plugins (
        id TEXT PRIMARY KEY, version TEXT NOT NULL,
        source TEXT NOT NULL CHECK (source IN ('bundled', 'registry', 'dev')),
        registryId TEXT, targetSha256 TEXT, treeHash TEXT NOT NULL, dir TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('content', 'code')),
        enabled INTEGER NOT NULL DEFAULT 0,
        state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'disabled', 'revoked', 'crashLoop')),
        autoUpdate INTEGER NOT NULL DEFAULT 0, installedBy TEXT NOT NULL,
        installedAt INTEGER NOT NULL, updatedAt INTEGER NOT NULL) WITHOUT ROWID;
      CREATE INDEX plugins_registry ON plugins (registryId) WHERE registryId IS NOT NULL;
      CREATE TABLE plugin_grants (
        pluginId TEXT NOT NULL, version TEXT NOT NULL, permission TEXT NOT NULL,
        grantedBy TEXT NOT NULL, grantedAt INTEGER NOT NULL);
      CREATE INDEX plugin_grants_plugin ON plugin_grants (pluginId, version);
      CREATE TABLE plugin_plans (
        id TEXT PRIMARY KEY, digest TEXT NOT NULL, pluginId TEXT NOT NULL, version TEXT NOT NULL,
        source TEXT NOT NULL, registryId TEXT, action TEXT NOT NULL,
        byKind TEXT NOT NULL, byId TEXT NOT NULL, human INTEGER NOT NULL, base TEXT,
        createdAt INTEGER NOT NULL, expiresAt INTEGER NOT NULL) WITHOUT ROWID;
      CREATE INDEX plugin_plans_expires ON plugin_plans (expiresAt);
    `);
  },
};
