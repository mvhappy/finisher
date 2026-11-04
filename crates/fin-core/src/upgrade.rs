//! Upcasters (sub-project 4 spec §3.2): old records and snapshots are upgraded in memory, on
//! read, by pure one-step converters on `serde_json::Value`, so old Rust types need not be
//! kept. A step must not change ids, timestamps or bindings.
//!
//! Schema version 1 is the first, so there are no steps yet. Adding version `n + 1` means
//! bumping `SCHEMA_VERSION` and appending a `Step { from: n, .. }` to each table, with a golden
//! test (frozen old shape in, expected new shape out).

use serde_json::{Value, json};

use crate::error::CoreError;
use crate::model::{Project, SCHEMA_VERSION};
use crate::record::{Record, decode_record, invalid_format};

/// Converts a value at schema version `from` to `from + 1`. The runner stamps the new version.
pub(crate) struct Step {
    pub(crate) from: u32,
    pub(crate) run: fn(Value) -> Result<Value, CoreError>,
}

const RECORD_STEPS: &[Step] = &[];
const SNAPSHOT_STEPS: &[Step] = &[];

/// A record value of any known schema version, upgraded to `SCHEMA_VERSION`. Upgrading also
/// stamps `intent.schemaVersion`, which follows the record's. Above `SCHEMA_VERSION` is
/// `NEWER_FORMAT`; an older version with no step is `SCHEMA_VERSION`; a value that is not a
/// record is `INVALID_FORMAT`.
pub fn upcast_record(v: Value) -> Result<Record, CoreError> {
    decode_record(upcast(v, RECORD_STEPS, SCHEMA_VERSION, "record")?)
}

/// A snapshot (model state) value of any known schema version, upgraded to `SCHEMA_VERSION`.
/// Errors as [`upcast_record`].
pub fn upcast_snapshot(v: Value) -> Result<Project, CoreError> {
    let v = upcast(v, SNAPSHOT_STEPS, SCHEMA_VERSION, "snapshot")?;
    serde_json::from_value(v).map_err(|e| invalid_format("snapshot", e))
}

/// Runs `steps` in order from `v`'s `schemaVersion` up to `target`.
pub(crate) fn upcast(
    mut v: Value,
    steps: &[Step],
    target: u32,
    what: &str,
) -> Result<Value, CoreError> {
    let mut version = schema_version(&v, what)?;
    if version > target {
        return Err(CoreError::new(
            "NEWER_FORMAT",
            format!("the {what} has schema version {version}; this Finisher reads up to {target}"),
        )
        .with_details(json!({ "kind": what, "schemaVersion": version, "supported": target }))
        .with_fix("update Finisher to open this project"));
    }
    while version < target {
        let Some(step) = steps.iter().find(|s| s.from == version) else {
            return Err(CoreError::new(
                "SCHEMA_VERSION",
                format!("no upgrade from {what} schema version {version}"),
            )
            .with_details(json!({ "kind": what, "schemaVersion": version, "supported": target })));
        };
        v = (step.run)(v)?;
        version += 1;
        stamp(&mut v, version);
    }
    Ok(v)
}

fn schema_version(v: &Value, what: &str) -> Result<u32, CoreError> {
    v.get("schemaVersion")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| invalid_format(what, "schemaVersion is missing or not a version number"))
}

fn stamp(v: &mut Value, version: u32) {
    if let Value::Object(map) = v {
        map.insert("schemaVersion".into(), json!(version));
        if let Some(Value::Object(intent)) = map.get_mut("intent") {
            intent.insert("schemaVersion".into(), json!(version));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic v0 → v1: v0 called `author` `actor` and stored `committedAt` in seconds.
    fn v0_to_v1(mut v: Value) -> Result<Value, CoreError> {
        let map = v
            .as_object_mut()
            .ok_or_else(|| invalid_format("record", "not an object"))?;
        if let Some(actor) = map.remove("actor") {
            map.insert("author".into(), actor);
        }
        if let Some(s) = map.get("committedAt").and_then(Value::as_i64) {
            map.insert("committedAt".into(), json!(s * 1000));
        }
        Ok(v)
    }

    /// Synthetic v1 → v2: adds a `note`.
    fn v1_to_v2(mut v: Value) -> Result<Value, CoreError> {
        assert_eq!(v["schemaVersion"], 1, "v1 → v2 runs after v0 → v1");
        v["note"] = json!("added in v2");
        Ok(v)
    }

    const STEPS: &[Step] = &[
        Step {
            from: 1,
            run: v1_to_v2,
        },
        Step {
            from: 0,
            run: v0_to_v1,
        },
    ];

    fn golden_v0() -> Value {
        json!({
            "txId": "01JC0000000000000000000001", "seq": 1, "kind": "edit", "schemaVersion": 0,
            "parentHash": null, "actor": "fixture", "committedAt": 1_700_000_000,
            "intent": { "txId": "01JC0000000000000000000001", "author": "fixture",
                        "branch": "main", "baseRev": 0, "schemaVersion": 0,
                        "ops": [{ "op": "clip.delete", "clip": "c1" }] },
            "resolved": [{ "op": "clip.delete", "clip": "c1" }],
            "inverse": [], "bindings": {}
        })
    }

    #[test]
    fn upcast_chain_runs_steps_in_order() {
        let out = upcast(golden_v0(), STEPS, 2, "record").unwrap();
        let mut expected = golden_v0();
        let map = expected.as_object_mut().unwrap();
        map.remove("actor");
        map.insert("author".into(), json!("fixture"));
        map.insert("committedAt".into(), json!(1_700_000_000_000_i64));
        map.insert("schemaVersion".into(), json!(2));
        map.insert("note".into(), json!("added in v2"));
        expected["intent"]["schemaVersion"] = json!(2);
        assert_eq!(out, expected);
    }

    #[test]
    fn upcast_to_v1_decodes_as_a_record() {
        let out = upcast(golden_v0(), STEPS, 1, "record").unwrap();
        let record = decode_record(out).unwrap();
        assert_eq!(record.author, "fixture");
        assert_eq!(record.committed_at, 1_700_000_000_000);
        assert_eq!(
            (record.schema_version, record.intent.schema_version),
            (1, 1)
        );
    }

    #[test]
    fn a_missing_step_is_refused() {
        let e = upcast(golden_v0(), &STEPS[..1], 2, "record").unwrap_err();
        assert_eq!(e.code, "SCHEMA_VERSION");
    }
}
