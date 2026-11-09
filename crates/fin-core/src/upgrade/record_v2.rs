//! Record step v2 → v3 (sub-project 4 spec §5.12.2). **Frozen**: written on
//! `serde_json::Value` with literal names and pinned by `tests/upgrade_golden.rs`.
//!
//! A v2 record that holds no keyframe op and no key list is a valid v3 record as it is; the
//! runner stamps `schemaVersion: 3`. A v2 record with a `keyframe.*` op, or a non-empty
//! `keyframes` map anywhere in it (inside an `item.restore`, `effect.restore`,
//! `track.restore` or `timeline.restore`), is refused with `SCHEMA_VERSION`: its keys have no
//! ids, and no such record exists outside pre-release development builds (spec §5.12, owner
//! decision of 2026-11-09), so nothing replays it.

use serde_json::{Value, json};

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    if holds_keyframes(&v) {
        return Err(CoreError::new(
            "SCHEMA_VERSION",
            "a schema 2 record with keyframes is a pre-release format that cannot be upgraded",
        )
        .with_details(json!({
            "kind": "record",
            "schemaVersion": 2,
            "txId": v.get("txId").cloned().unwrap_or(Value::Null),
        }))
        .with_fix("recreate the project with this Finisher"));
    }
    Ok(v)
}

fn holds_keyframes(v: &Value) -> bool {
    match v {
        Value::Object(map) => {
            let op = map
                .get("op")
                .and_then(Value::as_str)
                .is_some_and(|o| o.starts_with("keyframe."));
            let keys = map
                .get("keyframes")
                .and_then(Value::as_object)
                .is_some_and(|k| !k.is_empty());
            op || keys || map.values().any(holds_keyframes)
        }
        Value::Array(items) => items.iter().any(holds_keyframes),
        _ => false,
    }
}
