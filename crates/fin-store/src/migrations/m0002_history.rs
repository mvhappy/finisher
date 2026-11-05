//! Migration 0002: the derived history tables of sub-project 4B (plan 4B Decision 5).
//!
//! **Frozen at schema v2.** It writes literal values only, never current constants:
//!
//! - creates `branches` (the branch index) and `record_links` (one row per non-`edit` record,
//!   with its kind, origin, origin hash, source branch, merge group, targets and name), with
//!   their indexes. Both are derived: they can be rebuilt from `txs`;
//! - inserts the `main` row (a 4A database has no other branch);
//! - sets `meta` `formatVersion` and `minReaderVersion` to `"2"`.
//!
//! It never runs `ALTER TABLE`: it only creates tables, so a fresh database (`db::SCHEMA`, which
//! holds the same text) and a migrated one have the same schema. A 4A database holds only `edit`
//! records, so `record_links` starts empty.

use rusqlite::Transaction;

use crate::StoreError;

const V2_TABLES: &str = "
CREATE TABLE IF NOT EXISTS branches (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, parent TEXT, fork_seq INTEGER NOT NULL,
    fork_hash TEXT, created_seq INTEGER, created_by TEXT NOT NULL, created_at INTEGER NOT NULL,
    archived INTEGER NOT NULL, merged INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS record_links (
    tx_id TEXT PRIMARY KEY, branch TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL,
    author TEXT NOT NULL, origin TEXT, origin_hash TEXT, from_branch TEXT, merge_id TEXT,
    targets TEXT, name TEXT, UNIQUE (branch, seq)
);
CREATE INDEX IF NOT EXISTS record_links_origin ON record_links (origin) WHERE origin IS NOT NULL;
CREATE INDEX IF NOT EXISTS record_links_origin_hash ON record_links (origin_hash) WHERE origin_hash IS NOT NULL;
CREATE INDEX IF NOT EXISTS record_links_merge ON record_links (merge_id) WHERE merge_id IS NOT NULL;
";

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(V2_TABLES)?;
    txn.execute_batch(
        "INSERT OR IGNORE INTO branches VALUES ('main','main',NULL,0,NULL,NULL,'',0,0,0);
         UPDATE meta SET value = '2' WHERE key IN ('formatVersion','minReaderVersion');",
    )?;
    Ok(())
}
