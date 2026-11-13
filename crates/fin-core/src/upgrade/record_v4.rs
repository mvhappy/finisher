//! Record step v4 → v5 (sub-project 6 spec §6.3, plan 6B-1 Task 11). **Frozen**: written on
//! `serde_json::Value` with literal names and pinned by `tests/upgrade_golden.rs`.
//!
//! Schema 5 adds the optional `VideoInfo` fields `display`, `rotation` and `pixelAspect` and
//! the op `asset.reprobe`. A v4 record names no `asset.reprobe` and carries no such field, and
//! absent means unknown, so it is a valid v5 record as it is: the step is the identity, and
//! the runner stamps `schemaVersion: 5` (and `intent.schemaVersion`). The bump exists so an
//! older reader refuses a v5 record (`NEWER_FORMAT`) instead of dropping the new fields.
//!
//! A record labelled v4 that holds an `asset.reprobe` op, or a `video` object with `display`,
//! `rotation` or `pixelAspect`, was never written by a v4 Finisher (damage, or a v5 record
//! mislabelled): `INVALID_FORMAT`, rather than reading it as v5. An item's
//! `transform.rotation` is not a `video` field and stays as it is.

use serde_json::{Value, json};

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    if holds_v5(&v, false) {
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

/// Whether `v` holds a v5-only shape; `in_video`: `v` is the value of a `video` key.
fn holds_v5(v: &Value, in_video: bool) -> bool {
    match v {
        Value::Object(map) => {
            let op = map.get("op").and_then(Value::as_str) == Some("asset.reprobe");
            let fields = in_video
                && ["display", "rotation", "pixelAspect"]
                    .iter()
                    .any(|k| map.contains_key(*k));
            op || fields || map.iter().any(|(k, x)| holds_v5(x, k == "video"))
        }
        Value::Array(items) => items.iter().any(|x| holds_v5(x, false)),
        _ => false,
    }
}
