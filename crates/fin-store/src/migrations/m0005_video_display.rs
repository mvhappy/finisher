//! Migration 0005: the `VideoInfo` display geometry and `asset.reprobe` version bump (model
//! schema 4; sub-project 6 spec §6.3, plan 6B-1 Task 11).
//!
//! **Frozen at schema v5.** It writes literal values only, never current constants:
//!
//! - sets `meta` `formatVersion` and `minReaderVersion` to `"5"` (the runner sets
//!   `user_version` 5).
//!
//! No table changes and no data rewritten: the database now holds model v4 records and
//! snapshots (assets with `display`, `rotation` and `pixelAspect`; `asset.reprobe` ops), and
//! an older binary must refuse it (`DB_TOO_NEW`) rather than fail row by row. Old v1–v3 rows
//! stay as written and are upgraded on every read.

use rusqlite::Transaction;

use crate::StoreError;

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(
        "UPDATE meta SET value = '5' WHERE key IN ('formatVersion', 'minReaderVersion');",
    )?;
    Ok(())
}
