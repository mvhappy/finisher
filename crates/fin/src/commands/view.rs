//! Read commands: `fin status` and `fin view` (plan 4C Task 25). Both cover every timeline
//! (`--timeline` narrows `view` to one); the screenplay lines are [`crate::view`]'s.

use clap::Args;
use fin_core::{CoreError, Project, Timeline, Track, TrackKind};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Globals, branch, fonts, target_branch};
use crate::addr;
use crate::backend::Backend;
use crate::view::{self, Filter, budget};

/// Summarise the project: branch and head (`rev`: the snapshot read, as `fin view` reports
/// it), lease and read-only state, last export, unmerged branches, every timeline and its
/// tracks, asset ids, font state and warnings.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusArgs {}

/// The timelines as screenplay lines, in time order, within an optional token budget.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewArgs {
    /// Only this track, e.g. "t1".
    #[arg(long, value_name = "t")]
    #[serde(default)]
    pub track: Option<String>,
    /// Only lines overlapping this time range: "2s..5s", "2s.." or "..5s".
    #[arg(long, value_name = "a..b", allow_hyphen_values = true)]
    #[serde(default)]
    pub range: Option<String>,
    /// At most about this many tokens (chars / 4): details, caption text, then runs of
    /// items are folded, then the tail is cut, each step listed in `elided`.
    #[arg(long, value_name = "tokens")]
    #[serde(default)]
    pub budget: Option<u64>,
    /// List each animated path's keyframes as `<key>@<at>` (item-relative).
    #[arg(long)]
    #[serde(default)]
    pub detail: bool,
}

fn kind(track: &Track) -> &'static str {
    match track.kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Caption => "caption",
    }
}

/// Every timeline with its tracks (item count and end).
fn timelines(p: &Project) -> Result<Vec<Value>, CoreError> {
    p.timelines
        .iter()
        .map(|tl| {
            let tracks = tl
                .tracks
                .iter()
                .map(|t| {
                    Ok(json!({ "id": t.id, "name": t.name, "kind": kind(t),
                               "items": t.items.len(), "end": addr::track_end(p, &t.id)? }))
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(json!({
                "id": tl.id, "name": tl.name, "rate": tl.rate,
                "resolution": format!("{}x{}", tl.resolution.width, tl.resolution.height),
                "end": view::timeline_end(tl)?, "markers": tl.markers.len(), "tracks": tracks,
            }))
        })
        .collect()
}

fn unmeasured_list(marks: &std::collections::BTreeMap<String, &'static str>) -> Vec<Value> {
    marks
        .iter()
        .map(|(item, reason)| json!({ "item": item, "reason": reason }))
        .collect()
}

pub fn status(backend: &dyn Backend, g: &Globals) -> Result<Value, CoreError> {
    let reference = target_branch(backend, g)?;
    let snap = backend.snapshot(&reference)?;
    let p = &snap.project;
    let project = json!({ "id": p.id, "name": p.name });
    if backend.dry_run_only() {
        let marks = view::unmeasured(p, None);
        return Ok(json!({
            "project": project, "branch": reference, "head": snap.rev, "rev": snap.rev,
            "server": false, "timelines": timelines(p)?, "assets": p.assets.keys().collect::<Vec<_>>(),
            "fonts": { "unmeasured": unmeasured_list(&marks) },
        }));
    }
    let st = backend.project_call("project.status", json!({}))?;
    let views = branch::list(backend, true)?;
    let current = branch::find(&views, &reference).ok();
    let id = current
        .as_ref()
        .and_then(|v| v["id"].as_str())
        .unwrap_or(&reference)
        .to_string();
    let head = st["branches"]
        .as_array()
        .and_then(|bs| bs.iter().find(|b| b["branch"] == json!(id)))
        .map_or(json!(snap.rev), |b| b["head"].clone());
    let unmerged: Vec<Value> = views
        .iter()
        .filter(|v| v["state"] == "active" && v["id"] != "main")
        .map(|v| json!({ "id": v["id"], "name": v["display"] }))
        .collect();
    let files = fonts::file_states(backend)?;
    let marks = view::unmeasured(p, files.as_ref());
    let list = |k: &str| st["fonts"][k].as_array().cloned().unwrap_or_default();
    Ok(json!({
        "project": project,
        "branch": current.as_ref().map_or(json!(reference), |v| v["display"].clone()),
        "head": head,
        "rev": snap.rev,
        "lease": st["lease"],
        "readOnly": st["readOnly"],
        "lastExport": st["lastExportMs"],
        "unmergedBranches": unmerged,
        "timelines": timelines(p)?,
        "assets": p.assets.keys().collect::<Vec<_>>(),
        "fonts": { "missing": list("missing"), "fetching": list("fetching"),
                   "failed": list("failed"), "unmeasured": unmeasured_list(&marks) },
        "warnings": st["warnings"].as_array().cloned().unwrap_or_default(),
    }))
}

/// The timelines `fin view` shows: `--timeline`'s, the one holding `--track`, else all.
fn shown<'p>(p: &'p Project, g: &Globals, a: &ViewArgs) -> Result<Vec<&'p Timeline>, CoreError> {
    let named = g
        .timeline
        .as_deref()
        .map(|id| addr::timeline(p, Some(id)))
        .transpose()?;
    if let Some(t) = &a.track {
        let (tl, _) = addr::track(p, t)?; // NOT_FOUND with alternatives for an unknown track
        if named.is_some_and(|n| n.id != tl.id) {
            return Err(addr::not_found("track", t, Vec::new()).with_fix(format!(
                "{t} is on timeline {}; drop --timeline or name a track of it",
                tl.id
            )));
        }
        return Ok(vec![tl]);
    }
    Ok(match named {
        Some(tl) => vec![tl],
        None => p.timelines.iter().collect(),
    })
}

pub fn view(backend: &dyn Backend, g: &Globals, a: &ViewArgs) -> Result<Value, CoreError> {
    let snap = backend.snapshot(&target_branch(backend, g)?)?;
    let p = &snap.project;
    let tls = shown(p, g, a)?;
    let (from, to) = match &a.range {
        Some(r) => addr::range(p, r)?,
        None => (None, None),
    };
    let has_captions = tls.iter().any(|tl| {
        tl.tracks
            .iter()
            .any(|t| t.kind == TrackKind::Caption && !t.items.is_empty())
    });
    let files = if has_captions {
        fonts::file_states(backend)?
    } else {
        None
    };
    let marks = view::unmeasured(p, files.as_ref());
    let filter = Filter {
        track: a.track.as_deref(),
        from,
        to,
        detail: a.detail,
    };
    let sections = view::sections(&tls, filter, &marks)?;
    let (lines, elided, tokens) = budget::fit(sections, a.budget);
    let mut out = json!({
        "rev": snap.rev, "timeline": g.timeline, "lines": lines, "elided": elided, "tokens": tokens,
    });
    let mut resolved = json!({});
    if let Some(t) = &a.track {
        resolved["track"] = json!(t);
    }
    if a.range.is_some() {
        resolved["range"] = json!({ "from": from, "to": to });
    }
    if let Some(b) = a.budget {
        resolved["budget"] = json!(b);
    }
    if a.detail {
        resolved["detail"] = json!(true);
    }
    if resolved != json!({}) {
        out["resolved"] = resolved;
    }
    Ok(out)
}
