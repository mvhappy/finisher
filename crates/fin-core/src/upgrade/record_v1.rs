//! The record step v1 → v2 (plan 4C Decision 5). **Frozen** once 4C merges: it works on
//! `serde_json::Value` with literal field and op names, uses no current model types, and has
//! golden tests (`tests/upgrade_golden.rs`).
//!
//! In `intent.ops`, `resolved` and `inverse` (the only places a record holds ops):
//! - `clip.insert {id, track, at, asset, sourceIn, duration}` → `item.insert {id, track, at,
//!   duration, mode: "ripple", kind: "clip", asset, sourceIn}`;
//! - `clip.trim {clip, edge, delta}` → `item.trim {item, edge, delta, mode: "ripple"}`, and in
//!   `inverse` also `alone: true`: 4B's trims acted on one clip, so an undo of one made after
//!   the clip joined a link group (4C) trims that item alone, as every 4C trim inverse does;
//!   `intent.ops` and `resolved` replay where no group existed, so they stay as they were;
//! - `clip.delete {clip}` → `item.delete {item, mode: "ripple"}`;
//! - `clip.remove {id, ripple}` → `item.remove {id, mode}` (`ripple` → `"ripple"`, else
//!   `"lift"`);
//! - `clip.restore {clip, track, ripple}` → `item.restore {item: <clip> + kind: "clip", track,
//!   mode}`, where a missing `ripple` means `true` (4B's default);
//! - `asset.add` gains `kind: "media"`.
//!
//! Any other op is left as it is. Ids, timestamps and bindings are never touched (the runner
//! stamps `schemaVersion`).

use serde_json::{Map, Value, json};

use crate::error::CoreError;
use crate::record::invalid_format;

pub(crate) fn run(mut v: Value) -> Result<Value, CoreError> {
    if let Some(ops) = v.pointer_mut("/intent/ops") {
        each(ops, "intent.ops")?;
    }
    for list in ["resolved", "inverse"] {
        if let Some(ops) = v.get_mut(list) {
            each(ops, list)?;
        }
    }
    Ok(v)
}

fn each(ops: &mut Value, at: &str) -> Result<(), CoreError> {
    let Some(ops) = ops.as_array_mut() else {
        return Err(invalid_format("record", format!("{at} is not a list")));
    };
    for (i, op) in ops.iter_mut().enumerate() {
        let Some(map) = op.as_object_mut() else {
            return Err(invalid_format(
                "record",
                format!("{at}[{i}] is not an object"),
            ));
        };
        step(map).map_err(|why| invalid_format("record", format!("{at}[{i}]: {why}")))?;
        if at == "inverse" && map.get("op").and_then(Value::as_str) == Some("item.trim") {
            map.insert("alone".into(), json!(true));
        }
    }
    Ok(())
}

fn mode(ripple: Option<&Value>) -> Result<Value, String> {
    match ripple {
        None | Some(Value::Bool(true)) => Ok(json!("ripple")),
        Some(Value::Bool(false)) => Ok(json!("lift")),
        Some(other) => Err(format!("ripple {other} is not a boolean")),
    }
}

fn rename(map: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(v) = map.remove(from) {
        map.insert(to.into(), v);
    }
}

fn step(map: &mut Map<String, Value>) -> Result<(), String> {
    let name = map.get("op").and_then(Value::as_str).unwrap_or_default();
    match name {
        "asset.add" => {
            map.insert("kind".into(), json!("media"));
        }
        "clip.insert" => {
            map.insert("op".into(), json!("item.insert"));
            map.insert("mode".into(), json!("ripple"));
            map.insert("kind".into(), json!("clip"));
        }
        "clip.trim" => {
            map.insert("op".into(), json!("item.trim"));
            rename(map, "clip", "item");
            map.insert("mode".into(), json!("ripple"));
        }
        "clip.delete" => {
            map.insert("op".into(), json!("item.delete"));
            rename(map, "clip", "item");
            map.insert("mode".into(), json!("ripple"));
        }
        "clip.remove" => {
            let mode = mode(map.remove("ripple").as_ref())?;
            map.insert("op".into(), json!("item.remove"));
            map.insert("mode".into(), mode);
        }
        "clip.restore" => {
            let mode = mode(map.remove("ripple").as_ref())?;
            let Some(Value::Object(mut item)) = map.remove("clip") else {
                return Err("clip.restore has no clip object".into());
            };
            item.insert("kind".into(), json!("clip"));
            map.insert("op".into(), json!("item.restore"));
            map.insert("item".into(), Value::Object(item));
            map.insert("mode".into(), mode);
        }
        _ => {}
    }
    Ok(())
}
