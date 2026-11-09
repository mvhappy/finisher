//! Record step v3 → v4 (sub-project 6 spec §6.3, plan 6B-1 Task 11). **Frozen**: pinned by
//! `tests/upgrade_golden.rs`.
//!
//! Schema 4 adds the optional `VideoInfo` fields `display`, `rotation` and `pixelAspect` and
//! the op `asset.reprobe`. A v3 record names no `asset.reprobe` and carries no such field, and
//! absent means unknown, so it is a valid v4 record as it is: the step is the identity, and
//! the runner stamps `schemaVersion: 4` (and `intent.schemaVersion`). The bump exists so an
//! older reader refuses a v4 record (`NEWER_FORMAT`) instead of dropping the new fields.

use serde_json::Value;

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    Ok(v)
}
