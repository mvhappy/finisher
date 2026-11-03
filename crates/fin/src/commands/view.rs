//! Read commands: `fin status` and `fin view`.

use clap::Args;
use fin_core::{CoreError, Track, TrackKind};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::addr;
use crate::backend::Backend;

/// Summarise the project: revision, rate, tracks and assets.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusArgs {}

/// List clips per track with their timeline and source positions.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewArgs {
    /// Only this track, e.g. "t1".
    #[arg(long, value_name = "t")]
    #[serde(default)]
    pub track: Option<String>,
    /// Only clips overlapping this time range: "2s..5s", "2s.." or "..5s".
    #[arg(long, value_name = "a..b", allow_hyphen_values = true)]
    #[serde(default)]
    pub range: Option<String>,
}

fn kind(track: &Track) -> &'static str {
    match track.kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
    }
}

pub fn status(backend: &dyn Backend) -> Result<Value, CoreError> {
    let snap = backend.snapshot()?;
    let p = &snap.project;
    let tracks = p
        .timeline
        .tracks
        .iter()
        .map(|t| Ok(json!({ "id": t.id, "kind": kind(t), "clips": t.items.len(), "end": addr::track_end(p, &t.id)? })))
        .collect::<Result<Vec<_>, CoreError>>()?;
    Ok(json!({
        "rev": snap.rev,
        "project": { "id": p.id, "name": p.name, "rate": p.rate },
        "tracks": tracks,
        "assets": p.assets.keys().collect::<Vec<_>>(),
    }))
}

pub fn view(backend: &dyn Backend, a: &ViewArgs) -> Result<Value, CoreError> {
    let snap = backend.snapshot()?;
    let p = &snap.project;
    if let Some(t) = &a.track {
        addr::track_end(p, t)?; // NOT_FOUND with alternatives for an unknown track
    }
    let (from, to) = match &a.range {
        Some(r) => addr::range(p, r)?,
        None => (None, None),
    };
    let mut tracks = Vec::new();
    for t in p.timeline.tracks.iter().filter(|t| a.track.as_ref().is_none_or(|id| &t.id == id)) {
        let mut clips = Vec::new();
        for c in &t.items {
            let end = c.start.checked_add(c.duration)?;
            if from.is_some_and(|f| end <= f) || to.is_some_and(|t| c.start >= t) {
                continue;
            }
            clips.push(json!({ "id": c.id, "start": c.start, "end": end, "asset": c.asset, "sourceIn": c.source_in }));
        }
        tracks.push(json!({ "id": t.id, "kind": kind(t), "clips": clips }));
    }
    let mut resolved = json!({});
    if let Some(t) = &a.track {
        resolved["track"] = json!(t);
    }
    if a.range.is_some() {
        resolved["range"] = json!({ "from": from, "to": to });
    }
    Ok(json!({ "rev": snap.rev, "rate": p.rate, "tracks": tracks, "resolved": resolved }))
}
