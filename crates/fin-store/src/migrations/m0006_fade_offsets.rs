//! Migration 0006: the model-v4 version bump (graphic `sourceIn`, audio fade offsets; sub-project
//! 4 spec §5.13).
//!
//! **Frozen at schema v6.** It writes literal values only, never current constants:
//!
//! - sets `meta` `formatVersion` and `minReaderVersion` to `"6"` (the runner sets
//!   `user_version` 6).
//!
//! No table changes and no data rewritten: the database now holds model v4 records and
//! snapshots, and an older binary must refuse it (`DB_TOO_NEW`) rather than fail row by row.
//! Old v2 and v3 rows stay as written and are upgraded on every read.

use rusqlite::Transaction;

use crate::StoreError;

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(
        "UPDATE meta SET value = '6' WHERE key IN ('formatVersion', 'minReaderVersion');",
    )?;
    Ok(())
}
