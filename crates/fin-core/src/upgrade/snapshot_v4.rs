//! Snapshot step v4 → v5 (sub-project 6 spec §6.3, plan 6B-1 Task 11). **Frozen**: pinned by
//! `tests/upgrade_golden.rs`; it never changes once it ships, whatever later models look like.
//!
//! Schema 5 adds three optional fields to an asset's `video`: `display`, `rotation` and
//! `pixelAspect`. Absent means unknown, so a v4 project (whose assets have none) is a valid v5
//! project as it is: the step is the identity, and the runner stamps `schemaVersion: 5`. No
//! value is guessed for the new fields (rotation 0 would be a claim the v4 probe never made);
//! `asset.reprobe` fills them.

use serde_json::Value;

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    Ok(v)
}
