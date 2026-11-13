//! Migration 0004: the derived `drops` index of sub-project 5E (plan 5E Decision 4).
//!
//! **Frozen at schema v4.** It writes literal values only, never current constants:
//!
//! - creates `drops` (one row per origin a `drop` record names: the record's `tx_id`, its
//!   `branch` and `seq`, the `origin` and whether it is `dropped` (1) or undropped (0)) and its
//!   index on `(branch, seq)`. Derived: rebuilt from `txs` like `record_links`;
//! - sets `meta` `formatVersion` and `minReaderVersion` to `"4"` (the runner sets
//!   `user_version` 4), so a 4C binary refuses the database (`DB_TOO_NEW`) rather than write
//!   records the index does not follow.
//!
//! No data rewritten: a 4C database holds no `drop` record (a 4C reader cannot read one), so
//! `drops` starts empty. Like 0002, it only creates tables and indexes, so a fresh database
//! (`db::SCHEMA`, which holds the same text) and a migrated one have the same schema.

use rusqlite::Transaction;

use crate::StoreError;

const V4_TABLES: &str = "
CREATE TABLE IF NOT EXISTS drops (
    tx_id TEXT NOT NULL, branch TEXT NOT NULL, seq INTEGER NOT NULL, origin TEXT NOT NULL,
    dropped INTEGER NOT NULL, PRIMARY KEY (tx_id, origin)
);
CREATE INDEX IF NOT EXISTS drops_at ON drops (branch, seq);
";

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(V4_TABLES)?;
    txn.execute_batch(
        "UPDATE meta SET value = '4' WHERE key IN ('formatVersion', 'minReaderVersion');",
    )?;
    Ok(())
}
