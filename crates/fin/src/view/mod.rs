//! `fin view`'s screenplay (plan 4C Task 25, Decision 46): per timeline a header line, then
//! one line per marker, item and transition in time order (markers first at a time, then
//! items in track order, then transitions), captions with their text inline. [`budget`]
//! degrades the lines in a fixed order to fit a token budget and says what it elided.
//!
//! Captions that will not measure are marked `(unmeasured: <reason>)`: `font unpinned` (no
//! pin for the style's face or a script of the text, [`fin_core::layout::faces_for`]),
//! `font missing`, `font fetching` or `font fetch failed` (from the server's file states,
//! the same `fonts.status` data `fin fonts ls --pinned` shows), `style missing`. Offline
//! there are no file states, so only `font unpinned` and `style missing` are marked.

pub mod budget;
pub mod lines;

use std::collections::BTreeMap;

use fin_core::captions::resolve_style;
use fin_core::layout::faces_for;
use fin_core::{CoreError, ItemBody, Project, Rational, Timeline};
use serde_json::Value;

/// One line's source.
#[derive(Debug, Clone)]
pub struct Entry {
    pub start: Rational,
    pub end: Option<Rational>,
    /// 0 marker, 1 item, 2 transition: the order at one time.
    pub rank: u8,
    /// The track's index in its timeline (0 for markers).
    pub track: usize,
    /// The track's name, `──` for a marker.
    pub label: String,
    /// The item kind (`clip`, `caption`, ...); `marker`, `transition`.
    pub kind: &'static str,
    pub id: String,
    pub head: String,
    /// What the `detail` step drops.
    pub detail: Vec<String>,
    /// A caption's words (the `text` step keeps the first [`budget::SHORT_WORDS`]).
    pub words: Option<Vec<String>>,
    /// Kept at every step: a caption's style and unmeasured mark.
    pub tail: String,
    /// A transition's items.
    pub between: Option<[String; 2]>,
    /// How many lines the `merge` step folded into this one (0: not a merged line).
    pub folded: usize,
}

/// One timeline's lines.
#[derive(Debug, Clone)]
pub struct Section {
    pub timeline: String,
    pub header: String,
    pub entries: Vec<Entry>,
}

/// What [`sections`] keeps: one track, and a time range (half-open; a point marker counts
/// when `from <= at < to`); and how much of an item it shows ([`Filter::detail`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct Filter<'a> {
    pub track: Option<&'a str>,
    pub from: Option<Rational>,
    pub to: Option<Rational>,
    /// `--detail`: an item's keyframes are listed with their ids and times.
    pub detail: bool,
}

impl Filter<'_> {
    fn keeps(&self, start: Rational, end: Option<Rational>) -> bool {
        let after = match end {
            Some(e) => self.from.is_none_or(|f| e > f),
            None => self.from.is_none_or(|f| start >= f),
        };
        after && self.to.is_none_or(|t| start < t)
    }
}

/// A timeline's end: its latest item end.
pub fn timeline_end(tl: &Timeline) -> Result<Rational, CoreError> {
    let mut end = Rational::zero();
    for it in tl.tracks.iter().flat_map(|t| t.items.iter()) {
        end = end.max(it.end()?);
    }
    Ok(end)
}

/// The sections of `timelines`, each in time order.
pub fn sections(
    timelines: &[&Timeline],
    filter: Filter<'_>,
    unmeasured: &BTreeMap<String, &'static str>,
) -> Result<Vec<Section>, CoreError> {
    let mut out = Vec::new();
    for tl in timelines {
        let mut entries = Vec::new();
        if filter.track.is_none() {
            for m in &tl.markers {
                let end = m.duration.map(|d| m.at.checked_add(d)).transpose()?;
                let mut detail = Vec::new();
                if let Some(c) = &m.color {
                    detail.push(c.clone());
                }
                entries.push(Entry {
                    start: m.at,
                    end,
                    rank: 0,
                    track: 0,
                    label: "──".into(),
                    kind: "marker",
                    id: m.id.clone(),
                    head: format!("marker {} {:?}", m.id, m.label),
                    detail,
                    words: None,
                    tail: String::new(),
                    between: None,
                    folded: 0,
                });
            }
        }
        for (ti, t) in tl.tracks.iter().enumerate() {
            if filter.track.is_some_and(|id| id != t.id) {
                continue;
            }
            for it in &t.items {
                let (head, detail) = lines::item_parts(it, filter.detail);
                let (words, tail) = match &it.body {
                    ItemBody::Caption { block } => {
                        let mut tail = format!("({})", block.style);
                        if !block.overrides.is_empty() {
                            let keys: Vec<&str> =
                                block.overrides.keys().map(String::as_str).collect();
                            tail = format!("({} +{})", block.style, keys.join(","));
                        }
                        if let Some(why) = unmeasured.get(&it.id) {
                            tail.push_str(&format!(" (unmeasured: {why})"));
                        }
                        (
                            Some(block.words.iter().map(|w| w.text.clone()).collect()),
                            tail,
                        )
                    }
                    _ => (None, String::new()),
                };
                entries.push(Entry {
                    start: it.start,
                    end: Some(it.end()?),
                    rank: 1,
                    track: ti,
                    label: t.name.clone(),
                    kind: it.kind_name(),
                    id: it.id.clone(),
                    head,
                    detail,
                    words,
                    tail,
                    between: None,
                    folded: 0,
                });
            }
            for tr in &t.transitions {
                let Some(b) = t.items.iter().find(|i| i.id == tr.between[1]) else {
                    continue;
                };
                let half = tr.duration.checked_div(Rational::from_int(2))?;
                let kind = serde_json::to_value(tr.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let mut detail = Vec::new();
                if tr.curve != fin_core::Curve::Linear {
                    detail.push("equalPower".to_string());
                }
                entries.push(Entry {
                    start: b.start.checked_sub(half)?,
                    end: Some(b.start.checked_add(half)?),
                    rank: 2,
                    track: ti,
                    label: t.name.clone(),
                    kind: "transition",
                    id: tr.id.clone(),
                    head: format!(
                        "{} {kind} {}→{} {}",
                        tr.id,
                        tr.between[0],
                        tr.between[1],
                        lines::secs(tr.duration)
                    ),
                    detail,
                    words: None,
                    tail: String::new(),
                    between: Some(tr.between.clone()),
                    folded: 0,
                });
            }
        }
        entries.retain(|e| filter.keeps(e.start, e.end));
        entries.sort_by_key(|a| (a.start, a.rank, a.track));
        out.push(Section {
            timeline: tl.id.clone(),
            header: lines::header(tl, timeline_end(tl)?),
            entries,
        });
    }
    Ok(out)
}

/// Why each caption of `project` will not measure (see the module docs). `files`: the
/// server's `fonts.status` files by SHA-256, `None` offline.
pub fn unmeasured(
    project: &Project,
    files: Option<&BTreeMap<String, Value>>,
) -> BTreeMap<String, &'static str> {
    let mut out = BTreeMap::new();
    for it in project
        .timelines
        .iter()
        .flat_map(|tl| tl.tracks.iter())
        .flat_map(|t| t.items.iter())
    {
        let ItemBody::Caption { block } = &it.body else {
            continue;
        };
        let Ok(style) = resolve_style(project, block) else {
            out.insert(it.id.clone(), "style missing");
            continue;
        };
        let faces = match faces_for(project, &style, &block.words) {
            Ok(faces) => faces,
            Err(_) => {
                out.insert(it.id.clone(), "font unpinned");
                continue;
            }
        };
        let Some(files) = files else {
            continue;
        };
        let why = faces.iter().find_map(|f| {
            let file = files.get(&f.sha256);
            match file.and_then(|v| v["state"]["state"].as_str()) {
                Some("cached") => None,
                Some("queued" | "fetching") => Some("font fetching"),
                Some("failed") if file.is_some_and(|v| !v["entry"].is_null()) => {
                    Some("font fetch failed")
                }
                _ => Some("font missing"),
            }
        });
        if let Some(why) = why {
            out.insert(it.id.clone(), why);
        }
    }
    out
}
