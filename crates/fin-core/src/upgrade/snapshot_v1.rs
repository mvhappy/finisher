//! Snapshot step v1 → v2 (plan 4C Decisions 5 and 9). **Frozen**: it is written on
//! `serde_json::Value` with literal names and pinned by `tests/upgrade_golden.rs`; it never
//! changes once 4C ships, whatever later models look like.
//!
//! A v1 project has one `timeline {id, tracks}` whose tracks hold clips
//! `{id, asset, start, sourceIn, duration}`, and media assets
//! `{id, path, duration, video, audio, fingerprint}` (`video`/`audio` may be `null`). The step:
//! - moves `timeline` into `timelines[0]`, named `"Timeline 1"`, at the project `rate` and
//!   1080×1920 (owner decision);
//! - names the tracks by kind in track order (`V1`, `V2`, `A1`, …); flags and gain stay at
//!   their omitted defaults;
//! - gives every item `kind: "clip"` (speed and common fields at their omitted defaults);
//! - gives every asset `kind: "media"`, dropping a `null` `video` or `audio`;
//! - adds nothing for `bins`, `styles` and `fonts` (empty is the omitted default).
//!
//! The runner stamps `schemaVersion: 2`. The result equals `new_project`'s output for a
//! rev-0 project, and is the canonical v2 JSON (what encoding the decoded project gives).
//! Ids are never touched.

use serde_json::{Map, Value, json};

use crate::error::CoreError;
use crate::record::invalid_format;

pub(crate) fn run(mut v: Value) -> Result<Value, CoreError> {
    let bad = |why: &str| invalid_format("snapshot", format!("v1 project: {why}"));
    let map = v.as_object_mut().ok_or_else(|| bad("not an object"))?;
    let rate = map.get("rate").cloned().ok_or_else(|| bad("no rate"))?;
    let mut timeline = match map.remove("timeline") {
        Some(Value::Object(t)) => t,
        _ => return Err(bad("no timeline object")),
    };
    let mut counts: Map<String, Value> = Map::new();
    let tracks = match timeline.get_mut("tracks") {
        Some(Value::Array(tracks)) => tracks,
        _ => return Err(bad("timeline has no tracks array")),
    };
    for track in tracks.iter_mut() {
        let track = track
            .as_object_mut()
            .ok_or_else(|| bad("a track is not an object"))?;
        let letter = match track.get("kind").and_then(Value::as_str) {
            Some("video") => "V",
            Some("audio") => "A",
            _ => return Err(bad("a track kind is not video or audio")),
        };
        let n = counts.get(letter).and_then(Value::as_u64).unwrap_or(0) + 1;
        counts.insert(letter.to_string(), json!(n));
        track.insert("name".into(), json!(format!("{letter}{n}")));
        let items = match track.get_mut("items") {
            Some(Value::Array(items)) => items,
            _ => return Err(bad("a track has no items array")),
        };
        for item in items.iter_mut() {
            let item = item
                .as_object_mut()
                .ok_or_else(|| bad("an item is not an object"))?;
            item.insert("kind".into(), json!("clip"));
        }
    }
    timeline.insert("name".into(), json!("Timeline 1"));
    timeline.insert("rate".into(), rate);
    timeline.insert(
        "resolution".into(),
        json!({ "width": 1080, "height": 1920 }),
    );
    map.insert(
        "timelines".into(),
        Value::Array(vec![Value::Object(timeline)]),
    );
    let assets = match map.get_mut("assets") {
        Some(Value::Object(assets)) => assets,
        _ => return Err(bad("no assets object")),
    };
    for asset in assets.values_mut() {
        let asset = asset
            .as_object_mut()
            .ok_or_else(|| bad("an asset is not an object"))?;
        asset.insert("kind".into(), json!("media"));
        for stream in ["video", "audio"] {
            if asset.get(stream) == Some(&Value::Null) {
                asset.remove(stream);
            }
        }
    }
    Ok(v)
}
