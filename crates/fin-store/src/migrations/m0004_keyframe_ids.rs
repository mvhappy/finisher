//! Migration 0004: the stable-keyframe-ids version bump (sub-project 4 spec §5.12.2).
//!
//! **Frozen at schema v4.** It writes literal values only, never current constants:
//!
//! - sets `meta` `formatVersion` and `minReaderVersion` to `"4"` (the runner sets
//!   `user_version` 4).
//!
//! No table changes and no data rewritten: the database now holds model v3 records and
//! snapshots (keyframe ids), and an older binary must refuse it (`DB_TOO_NEW`) rather than
//! fail row by row. Old v2 rows stay as written and are upgraded on every read.

use rusqlite::Transaction;

use crate::StoreError;

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(
        "UPDATE meta SET value = '4' WHERE key IN ('formatVersion', 'minReaderVersion');",
    )?;
    Ok(())
}
