//! Migration 0003: the 4C version bump (plan 4C Decision 3).
//!
//! **Frozen at schema v3.** It writes literal values only, never current constants:
//!
//! - sets `meta` `formatVersion` and `minReaderVersion` to `"3"` (the runner sets
//!   `user_version` 3).
//!
//! No table changes and no data rewritten: a 4C database holds model v2 snapshots and records,
//! and a 4B binary must refuse it (`DB_TOO_NEW`) rather than misread it. Stored v1 diffs and
//! old `snapshots` rows stay as written and are upgraded on every read.

use rusqlite::Transaction;

use crate::StoreError;

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(
        "UPDATE meta SET value = '3' WHERE key IN ('formatVersion', 'minReaderVersion');",
    )?;
    Ok(())
}
