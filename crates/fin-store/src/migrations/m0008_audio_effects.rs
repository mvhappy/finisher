//! Migration 0008: the audio effect kinds version bump (model schema 6; sub-project 6 spec
//! §8.4, plan 6C Task 5).
//!
//! **Frozen at schema v8.** It writes literal values only, never current constants:
//!
//! - sets `meta` `formatVersion` and `minReaderVersion` to `"8"` (the runner sets
//!   `user_version` 8).
//!
//! No table changes and no data rewritten: the database now holds model v6 records and
//! snapshots (items with `eq`, `compressor`, `gate` and `deEsser` effects), and an older binary
//! must refuse it (`DB_TOO_NEW`) rather than fail row by row. Old v1–v5 rows stay as written
//! and are upgraded on every read.

use rusqlite::Transaction;

use crate::StoreError;

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(
        "UPDATE meta SET value = '8' WHERE key IN ('formatVersion', 'minReaderVersion');",
    )?;
    Ok(())
}
