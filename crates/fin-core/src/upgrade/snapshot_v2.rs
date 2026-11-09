//! Snapshot step v2 → v3 (sub-project 4 spec §5.12.2). **Frozen**: it is written on
//! `serde_json::Value` with literal names and pinned by `tests/upgrade_golden.rs`; it never
//! changes once it ships, whatever later models look like.
//!
//! A v2 key is `{at, value, interp}`. The step gives every key of every item an
//! `id: "kf<n>"`, numbered from 1 in document order: timelines, their tracks, their items, then
//! each item's `keyframes` map in path order (effect paths included), then list order. Nothing
//! else changes; the runner stamps `schemaVersion: 3`. Rev-0 projects hold no keys, so the
//! step leaves them as they are.

use serde_json::{Value, json};

use crate::error::CoreError;
use crate::record::invalid_format;

pub(crate) fn run(mut v: Value) -> Result<Value, CoreError> {
    let bad = |why: &str| invalid_format("snapshot", format!("v2 project: {why}"));
    let timelines = match v.get_mut("timelines") {
        Some(Value::Array(t)) => t,
        _ => return Err(bad("no timelines array")),
    };
    let mut n: u64 = 0;
    for timeline in timelines {
        let tracks = match timeline.get_mut("tracks") {
            Some(Value::Array(t)) => t,
            _ => return Err(bad("a timeline has no tracks array")),
        };
        for track in tracks {
            let items = match track.get_mut("items") {
                Some(Value::Array(i)) => i,
                _ => return Err(bad("a track has no items array")),
            };
            for item in items {
                let Some(map) = item.get_mut("keyframes") else {
                    continue;
                };
                let map = map
                    .as_object_mut()
                    .ok_or_else(|| bad("keyframes is not an object"))?;
                for list in map.values_mut() {
                    let list = list
                        .as_array_mut()
                        .ok_or_else(|| bad("a keyframe list is not an array"))?;
                    for key in list {
                        let key = key
                            .as_object_mut()
                            .ok_or_else(|| bad("a keyframe is not an object"))?;
                        n += 1;
                        // `id` first, as the v3 model writes it.
                        let rest = std::mem::take(key);
                        key.insert("id".into(), json!(format!("kf{n}")));
                        key.extend(rest);
                    }
                }
            }
        }
    }
    Ok(v)
}
