//! Snapshot step v3 → v4 (sub-project 6 spec §6.3, plan 6B-1 Task 11). **Frozen**: pinned by
//! `tests/upgrade_golden.rs`; it never changes once it ships, whatever later models look like.
//!
//! Schema 4 adds three optional fields to an asset's `video`: `display`, `rotation` and
//! `pixelAspect`. Absent means unknown, so a v3 project (whose assets have none) is a valid v4
//! project as it is: the step is the identity, and the runner stamps `schemaVersion: 4`. No
//! value is guessed for the new fields (rotation 0 would be a claim the v3 probe never made);
//! `asset.reprobe` fills them.

use serde_json::Value;

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    Ok(v)
}
