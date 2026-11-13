//! Read commands: `fin status` and `fin view` (plan 4C Task 25). Both cover every timeline
//! (`--timeline` narrows `view` to one); the screenplay lines are [`fin_core::view`]'s.

use clap::Args;
use fin_core::{CoreError, Project, Rational, Timeline, Track, TrackKind};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Globals, branch, fonts, inspect, target_branch};
use crate::backend::Backend;
use crate::output::usage;
use crate::{addr, follow};
use fin_core::view::{self, Filter, budget};

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
    /// At most about this many tokens (chars / 4) for the whole JSON response: details,
    /// caption text, then runs of items are folded, then the tail is cut (headers last), each
    /// step listed in `elided`.
    #[arg(long, value_name = "tokens")]
    #[serde(default)]
    pub budget: Option<u64>,
    /// List each animated path's keyframes as `<key>@<at>` (item-relative) and each effect's
    /// stored params.
    #[arg(long)]
    #[serde(default)]
    pub detail: bool,
    /// Every line, however long the response: no default page. Without it or `--budget` a
    /// response over [`DEFAULT_PAGE_TOKENS`] keeps its first lines and says where the rest
    /// starts (`elided` tail entries with `next`).
    #[arg(long, conflicts_with = "budget")]
    #[serde(default)]
    pub full: bool,
    /// Skip the first n lines of the one timeline shown (needs --timeline or --track): a tail
    /// entry's `next` names it.
    #[arg(long, value_name = "n")]
    #[serde(default)]
    pub offset: Option<usize>,
}

/// The tokens (chars / 4) a `fin view` without `--budget` or `--full` prints at most: past them
/// the view is a page (its first lines at full detail; review batch L, owner decision: bounded
/// default reads). About 16,000 characters.
pub const DEFAULT_PAGE_TOKENS: u64 = 4_000;

fn kind(track: &Track) -> &'static str {
    match track.kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Caption => "caption",
    }
}

/// Every timeline with its tracks (item count and end), at most [`inspect::SUMMARY_CAP`] tracks
/// each (`tracksTotal`, `tracksNext`: the `fin track ls` page that follows).
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
            let mut doc = json!({
                "id": tl.id, "name": tl.name, "rate": tl.rate,
                "resolution": format!("{}x{}", tl.resolution.width, tl.resolution.height),
                "end": view::timeline_end(tl)?, "markers": tl.markers.len(), "tracks": tracks,
            });
            let next = format!(
                "track ls --timeline {} --offset {}",
                tl.id,
                inspect::SUMMARY_CAP
            );
            inspect::cut(&mut doc, "tracks", Some(&next));
            Ok(doc)
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
        let mut doc = json!({
            "project": project, "branch": reference, "head": snap.rev, "rev": snap.rev,
            "server": false, "timelines": timelines(p)?, "assets": p.assets.keys().collect::<Vec<_>>(),
            "fonts": { "unmeasured": unmeasured_list(&marks) },
        });
        summarise(&mut doc);
        return Ok(doc);
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
    let mut doc = json!({
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
    });
    summarise(&mut doc);
    Ok(doc)
}

/// `fin status` is a summary (review batch L): its project-sized lists keep
/// [`inspect::SUMMARY_CAP`] entries, with `<key>Total` when cut, and `<key>Next` where a command
/// lists the rest: `timelines` (total only), each timeline's `tracks` ([`timelines`]),
/// `assets`, `unmergedBranches`, `warnings` (total only), `fonts.missing|fetching|failed`
/// (`fin fonts ls --pinned`) and `fonts.unmeasured` (total only).
fn summarise(doc: &mut Value) {
    let assets = format!("asset ls --offset {}", inspect::SUMMARY_CAP);
    inspect::cut(doc, "timelines", None);
    inspect::cut(doc, "assets", Some(&assets));
    inspect::cut(doc, "unmergedBranches", Some("branch ls"));
    inspect::cut(doc, "warnings", None);
    for key in ["missing", "fetching", "failed"] {
        inspect::cut(&mut doc["fonts"], key, Some("fonts ls --pinned"));
    }
    inspect::cut(&mut doc["fonts"], "unmeasured", None);
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

/// `fin view`'s document for `lines` and `elided`, and its size in chars as printed (compact
/// JSON and a newline): `{rev, timeline, lines, elided, lineTokens, responseTokens, resolved?}`.
/// `responseTokens` is `ceil(chars / 4)` of this very document, its own digits included.
fn respond(base: &Value, resolved: &Value, lines: &[String], elided: &[Value]) -> (Value, u64) {
    let mut out = base.clone();
    out["lines"] = json!(lines);
    out["elided"] = json!(elided);
    out["lineTokens"] = json!(budget::tokens(lines));
    if resolved != &json!({}) {
        out["resolved"] = resolved.clone();
    }
    let mut guess = 0;
    loop {
        out["responseTokens"] = json!(guess);
        let chars = out.to_string().chars().count() as u64 + 1;
        let t = chars.div_ceil(4);
        // Rising only (more digits, more chars), so this ends.
        if t <= guess {
            return (out, chars);
        }
        guess = t;
    }
}

/// `fin view`. Frames in `--range` (`f30..f31`) count at the rate of the timelines shown:
/// `--timeline`'s, the one holding `--track`, or, with neither, every timeline's when they all
/// share one. When the shown timelines' rates differ a frame is ambiguous and refused (`USAGE`,
/// `details.reason: MIXED_RATES`, fix: name a timeline or use seconds); seconds, clock times
/// and rationals resolve whatever the rates.
pub fn view(backend: &dyn Backend, g: &Globals, a: &ViewArgs) -> Result<Value, CoreError> {
    let snap = backend.snapshot(&target_branch(backend, g)?)?;
    let p = &snap.project;
    let tls = shown(p, g, a)?;
    let (from, to) = match &a.range {
        Some(r) => addr::range(p, &tls, r)?,
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
    let mut sections = view::sections(&tls, filter, &marks)?;
    if let Some(n) = a.offset {
        if tls.len() != 1 {
            return Err(usage(
                "--offset counts the lines of one timeline",
                json!({ "field": "offset", "timelines": tls.len() }),
                "name the timeline with --timeline (or a track with --track)",
            ));
        }
        sections[0].entries.retain(|e| e.index >= n);
    }
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
    if a.full {
        resolved["full"] = json!(true);
    }
    if let Some(n) = a.offset {
        resolved["offset"] = json!(n);
    }
    let base = json!({ "rev": snap.rev, "timeline": g.timeline });
    let page = Page {
        args: a,
        range: (from, to),
        single: tls.len() == 1,
    };
    let doc = |lines: &[String], elided: &[Value]| {
        respond(&base, &resolved, lines, &page.decorate(elided))
    };
    let size = |l: &[String], e: &[Value]| doc(l, e).1;
    let (lines, elided) = match a.budget {
        None if !a.full => budget::fit_page(sections, DEFAULT_PAGE_TOKENS, &size),
        b => budget::fit(sections, b, &size),
    };
    let (out, _) = doc(&lines, &elided);
    Ok(out)
}

/// What a `tail` entry's `next` repeats: the command's own flags.
struct Page<'a> {
    args: &'a ViewArgs,
    /// The resolved `--range`, kept as it is by the next page.
    range: (Option<Rational>, Option<Rational>),
    /// One timeline is shown (`--timeline`, `--track`, or a project with one).
    single: bool,
}

impl Page<'_> {
    /// `elided` with a follow-up on each tail entry (review batch L), one that always
    /// progresses: `next`, the exact `fin view` showing the cut lines (the same project, branch,
    /// `--track`, `--range`, `--budget`, `--detail`, narrowed to the entry's `--timeline`, from
    /// `--offset <offset>`); or, when no command would show more than this one did (nothing of
    /// a shown timeline fits), `next: null` with `blocked`: why, and what to run instead.
    fn decorate(&self, elided: &[Value]) -> Vec<Value> {
        elided
            .iter()
            .map(|e| {
                let mut e = e.clone();
                if e["step"] == "tail"
                    && let Some(tl) = e["timeline"].as_str().map(str::to_string)
                {
                    self.follow_up(&mut e, &tl);
                }
                e
            })
            .collect()
    }

    fn follow_up(&self, e: &mut Value, tl: &str) {
        let offset = e["offset"].as_u64();
        let at = self.args.offset.unwrap_or(0) as u64;
        // Narrowing to one timeline progresses; within one, only a later offset does.
        if !self.single || offset.is_some_and(|o| o > at) {
            e["next"] = json!(self.next(tl, offset));
            return;
        }
        e["next"] = Value::Null;
        e["blocked"] = if e["header"] == true {
            json!(format!(
                "the header alone is over the budget: {} lists the tracks, then view one with --track",
                follow::cmd(&format!("track ls --timeline {tl}"))
            ))
        } else {
            json!(format!(
                "the next line alone is over the budget: raise --budget, or {}",
                follow::cmd(&format!("view --timeline {tl} --offset {at} --full"))
            ))
        };
    }

    fn next(&self, timeline: &str, offset: Option<u64>) -> String {
        let mut cmd = format!("view --timeline {timeline}");
        if let Some(t) = &self.args.track {
            cmd.push_str(&format!(" --track {}", follow::quote(t)));
        }
        if let (None, None) = self.range {
        } else {
            let end = |r: Option<Rational>| r.map(|t| t.to_string()).unwrap_or_default();
            cmd.push_str(&format!(
                " --range {}..{}",
                end(self.range.0),
                end(self.range.1)
            ));
        }
        if let Some(o) = offset.filter(|&o| o > 0) {
            cmd.push_str(&format!(" --offset {o}"));
        }
        if let Some(b) = self.args.budget {
            cmd.push_str(&format!(" --budget {b}"));
        }
        if self.args.detail {
            cmd.push_str(" --detail");
        }
        follow::cmd(&cmd)
    }
}
