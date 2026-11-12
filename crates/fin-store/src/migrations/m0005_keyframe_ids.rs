//! Migration 0005: the stable-keyframe-ids version bump (sub-project 4 spec §5.12.2).
//!
//! **Frozen at schema v5.** It writes literal values only, never current constants:
//!
//! - sets `meta` `formatVersion` and `minReaderVersion` to `"5"` (the runner sets
//!   `user_version` 5).
//!
//! No table changes and no data rewritten: the database now holds model v3 records and
//! snapshots (keyframe ids), and an older binary must refuse it (`DB_TOO_NEW`) rather than
//! fail row by row. Old v2 rows stay as written and are upgraded on every read.

use rusqlite::Transaction;

use crate::StoreError;

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(
        "UPDATE meta SET value = '5' WHERE key IN ('formatVersion', 'minReaderVersion');",
    )?;
    Ok(())
}
