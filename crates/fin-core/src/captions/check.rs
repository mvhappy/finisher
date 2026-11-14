//! Which captions a transaction's overflow check measures (plan 4C Tasks 19–20): only those
//! whose layout the transaction can have changed. Task 20 measures them once, at the end of
//! the transaction, never per op.
//!
//! A caption's layout reads its shown words' text ([`shown_words`]), its style and its
//! overrides on the layout paths ([`is_layout_path`]), its timeline's resolution and the pins
//! of its faces. So a caption is checked when the transaction:
//! - adds it (not by a restore op: an undo bringing back what was deleted re-checks nothing);
//! - changes its shown words' text (`caption.setText`, a trim or a start-edge move that shows
//!   other words), its style reference, its overrides on a layout path, or its track or
//!   timeline;
//! - changes its style on a layout path;
//! - changes a font pin it is measured with (`font/<key>`, [`key_users`]);
//! - adds its timeline or track (not by a restore op), or changes the timeline's resolution.
//!
//! Moves, trims that show the same words, colours, strokes, highlights and positions change
//! nothing measured; removed entities need nothing measured.

use std::collections::{BTreeMap, BTreeSet};

use crate::captions::shown_words;
use crate::layout::{TextStyle, key_users};
use crate::model::{CaptionBlock, CaptionStyle, ItemBody, ParamValue, Project};
use crate::ops::Op;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Whether style path `path` is read by layout: `text.*`, `background.padding`, `safeZone`,
/// `maxLines`.
pub fn is_layout_path(path: &str) -> bool {
    path.starts_with("text.") || matches!(path, "background.padding" | "safeZone" | "maxLines")
}

/// Whether `op` restores what an earlier op deleted (`item.restore`, `track.restore`,
/// `timeline.restore`): what it brings back was measured before it was deleted.
pub fn is_restore(op: &Op) -> bool {
    matches!(
        op,
        Op::ItemRestore(_) | Op::TrackRestore(_) | Op::TimelineRestore(_)
    )
}

/// What layout reads of a style.
#[derive(Debug, Clone, PartialEq)]
struct StyleLayout {
    text: TextStyle,
    padding: f64,
    safe_zone: String,
    max_lines: u32,
}

impl StyleLayout {
    fn of(style: &CaptionStyle) -> StyleLayout {
        StyleLayout {
            text: style.text.clone(),
            padding: style.background.as_ref().map_or(0.0, |b| b.padding),
            safe_zone: style.safe_zone.clone(),
            max_lines: style.max_lines,
        }
    }
}

/// What layout reads of a caption item, besides its style and pins.
#[derive(Debug, Clone, PartialEq)]
struct CaptionLayout {
    timeline: String,
    track: String,
    style: String,
    words: Vec<String>,
    overrides: BTreeMap<String, ParamValue>,
}

impl CaptionLayout {
    fn of(
        timeline: &str,
        track: &str,
        block: &CaptionBlock,
        duration: crate::time::Rational,
    ) -> Self {
        CaptionLayout {
            timeline: timeline.to_string(),
            track: track.to_string(),
            style: block.style.clone(),
            words: shown_words(&block.words, duration)
                .into_iter()
                .map(|w| w.text)
                .collect(),
            overrides: block
                .overrides
                .iter()
                .filter(|(p, _)| is_layout_path(p))
                .map(|(p, v)| (p.clone(), v.clone()))
                .collect(),
        }
    }
}

/// What layout reads of a project before a transaction: each caption's and each project
/// style's layout inputs, to compare the transaction's result with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutInputs {
    captions: BTreeMap<String, CaptionLayout>,
    styles: BTreeMap<String, StyleLayout>,
}

impl LayoutInputs {
    /// `project`'s layout inputs.
    pub fn of(project: &Project) -> LayoutInputs {
        let mut captions = BTreeMap::new();
        for tl in &project.timelines {
            for t in &tl.tracks {
                for it in &t.items {
                    if let ItemBody::Caption { block } = &it.body {
                        let c = CaptionLayout::of(&tl.id, &t.id, block, it.duration);
                        captions.insert(it.id.clone(), c);
                    }
                }
            }
        }
        let styles = project
            .styles
            .iter()
            .map(|(id, s)| (id.clone(), StyleLayout::of(s)))
            .collect();
        LayoutInputs { captions, styles }
    }
}

/// The caption items the overflow check measures (module docs), in project order, each once.
/// `before`: the layout inputs of the project the transaction started from; `restores`: the
/// indices of its restore ops ([`is_restore`]).
pub fn to_check(
    before: &LayoutInputs,
    project: &Project,
    diff: &Diff,
    restores: &BTreeSet<u32>,
) -> Vec<String> {
    to_check_at(before, project, diff, restores)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// [`to_check`], each caption with the last op of the diff that put it on the list.
pub fn to_check_at(
    before: &LayoutInputs,
    project: &Project,
    diff: &Diff,
    restores: &BTreeSet<u32>,
) -> Vec<(String, u32)> {
    fn note(map: &mut BTreeMap<String, u32>, id: &str, op: u32) {
        let at = map.entry(id.to_string()).or_insert(op);
        *at = (*at).max(op);
    }
    let (mut items, mut styles, mut pinned) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let (mut timelines, mut tracks) = (BTreeMap::new(), BTreeMap::new());
    for c in diff
        .changes
        .iter()
        .filter(|c| c.change != ChangeKind::Removed && !restores.contains(&c.op))
    {
        let added = c.change == ChangeKind::Added;
        match c.kind {
            EntityKind::Item => note(&mut items, &c.id, c.op),
            EntityKind::Style => {
                let now = project.styles.get(&c.id).map(StyleLayout::of);
                if now.as_ref() != before.styles.get(&c.id) {
                    note(&mut styles, &c.id, c.op);
                }
            }
            EntityKind::Font => {
                if let Some(key) = c.id.strip_prefix("font/") {
                    for it in key_users(project, key).1 {
                        note(&mut pinned, &it, c.op);
                    }
                }
            }
            EntityKind::Timeline if added || c.path.as_deref() == Some("resolution") => {
                note(&mut timelines, &c.id, c.op);
            }
            EntityKind::Track if added => note(&mut tracks, &c.id, c.op),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for tl in &project.timelines {
        for t in &tl.tracks {
            for it in &t.items {
                let ItemBody::Caption { block } = &it.body else {
                    continue;
                };
                let changed = items.get(&it.id).filter(|_| {
                    let now = CaptionLayout::of(&tl.id, &t.id, block, it.duration);
                    before.captions.get(&it.id) != Some(&now)
                });
                let op = [
                    changed,
                    styles.get(&block.style),
                    pinned.get(&it.id),
                    timelines.get(&tl.id),
                    tracks.get(&t.id),
                ]
                .into_iter()
                .flatten()
                .max();
                if let Some(op) = op {
                    out.push((it.id.clone(), *op));
                }
            }
        }
    }
    out
}
