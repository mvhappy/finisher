//! Record step v4 → v5 (sub-project 6 spec §6.3, plan 6B-1 Task 11). **Frozen**: written on
//! `serde_json::Value` with literal names and pinned by `tests/upgrade_golden.rs`.
//!
//! Schema 5 adds the optional `VideoInfo` fields `display`, `rotation` and `pixelAspect` and
//! the op `asset.reprobe`. A v4 record names no `asset.reprobe` and carries no such field, and
//! absent means unknown, so it is a valid v5 record as it is: the step is the identity, and
//! the runner stamps `schemaVersion: 5` (and `intent.schemaVersion`). The bump exists so an
//! older reader refuses a v5 record (`NEWER_FORMAT`) instead of dropping the new fields.
//!
//! A record labelled v4 that holds an `asset.reprobe` op, or an `asset.add` whose `video` has
//! `display`, `rotation` or `pixelAspect`, was never written by a v4 Finisher (damage, or a v5
//! record mislabelled): `INVALID_FORMAT`, rather than reading it as v5. Only the ops themselves
//! are looked at, in `intent.ops`, `resolved` and `inverse` (the only places a record holds
//! ops): a value inside an op (a graphic's text slots, an item's `transform.rotation`) is user
//! or v4 data and stays as it is, whatever its keys.

use serde_json::{Value, json};

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    if ["/intent/ops", "/resolved", "/inverse"]
        .iter()
        .filter_map(|at| v.pointer(at).and_then(Value::as_array))
        .flatten()
        .any(holds_v5)
    {
        return Err(CoreError::new(
            "INVALID_FORMAT",
            "a schema 4 record holds asset.reprobe or VideoInfo display fields, which schema 4 never wrote",
        )
        .with_details(json!({
            "kind": "record",
            "schemaVersion": 4,
            "txId": v.get("txId").cloned().unwrap_or(Value::Null),
        })));
    }
    Ok(v)
}

/// Whether the op `op` is a v5-only shape.
fn holds_v5(op: &Value) -> bool {
    match op.get("op").and_then(Value::as_str) {
        Some("asset.reprobe") => true,
        Some("asset.add") => op
            .get("video")
            .and_then(Value::as_object)
            .is_some_and(|video| {
                ["display", "rotation", "pixelAspect"]
                    .iter()
                    .any(|k| video.contains_key(*k))
            }),
        _ => false,
    }
}
