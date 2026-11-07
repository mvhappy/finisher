//! Captions (plan 4C Task 18, spec §5.5): splitting words into blocks by a style's rule
//! ([`blocks`]), SubRip import ([`parse_srt`]), re-timing edited text ([`align_text`]), the
//! built-in styles and safe zones ([`presets`], [`safe_zones`]), and a caption's effective
//! style ([`resolve_style`]).
//!
//! Evaluation-pure (Global Constraints, Determinism): exact rationals, and `f64` arithmetic
//! only.

use std::collections::BTreeMap;

use crate::error::CoreError;
use crate::layout::key_users;
use crate::model::{CaptionBlock, CaptionStyle, ItemBody, Project};
use crate::ops::util::not_found;
use crate::params::set_style;
use crate::tx::{ChangeKind, Diff, EntityKind};

pub(crate) mod align;
mod blocks;
mod overflow;
mod presets;
mod srt;

pub use align::{GRID, align_text};
pub use blocks::{BlockDraft, blocks};
pub use overflow::{caption_request, check_overflow, measure_caption};
pub use presets::{SafeZone, preset, presets, safe_box, safe_zone, safe_zones};
pub use srt::{Srt, SrtWarning, parse_srt};

/// The prefix of the built-in styles' ids.
pub const PRESET_PREFIX: &str = "preset:";

/// The style `reference` names: a preset (`preset:<name>`) or a project style (`st<n>`).
pub fn style_of<'a>(project: &'a Project, reference: &str) -> Option<&'a CaptionStyle> {
    if reference.starts_with(PRESET_PREFIX) {
        preset(reference)
    } else {
        project.styles.get(reference)
    }
}

/// [`style_of`], or `NOT_FOUND` (kind `style`).
pub fn find_style<'a>(
    project: &'a Project,
    reference: &str,
) -> Result<&'a CaptionStyle, CoreError> {
    style_of(project, reference).ok_or_else(|| not_found("style", reference))
}

/// The style a caption is laid out with: its style with its `overrides` applied, in path
/// order. `NOT_FOUND` for a missing style; `PARAM_UNKNOWN`/`PARAM_TYPE` for an override the
/// style scope does not take (the ops refuse those before they are stored).
pub fn resolve_style(project: &Project, block: &CaptionBlock) -> Result<CaptionStyle, CoreError> {
    let mut style = find_style(project, &block.style)?.clone();
    for (path, value) in &block.overrides {
        set_style(&mut style, path, Some(value.clone()))?;
    }
    Ok(style)
}

/// The caption items a transaction's overflow check measures (plan 4C Task 19; Task 20
/// measures them once, at the end of the transaction, never per op), in project order, each
/// once: every caption the diff added or changed; every user of a style it changed; every
/// caption measured with a font pin it changed (`font/<key>`, [`key_users`]); every caption of
/// a timeline it added or whose resolution it changed, and of a track it added. Removed
/// entities need nothing measured; ripple shifts move captions without changing them.
pub fn to_check(project: &Project, diff: &Diff) -> Vec<String> {
    to_check_at(project, diff)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// [`to_check`], each caption with the last op of the diff that put it on the list.
pub fn to_check_at(project: &Project, diff: &Diff) -> Vec<(String, u32)> {
    fn note(map: &mut BTreeMap<String, u32>, id: &str, op: u32) {
        let at = map.entry(id.to_string()).or_insert(op);
        *at = (*at).max(op);
    }
    let (mut items, mut styles) = (BTreeMap::new(), BTreeMap::new());
    let (mut timelines, mut tracks) = (BTreeMap::new(), BTreeMap::new());
    for c in diff
        .changes
        .iter()
        .filter(|c| c.change != ChangeKind::Removed)
    {
        match c.kind {
            EntityKind::Item => note(&mut items, &c.id, c.op),
            EntityKind::Style => note(&mut styles, &c.id, c.op),
            EntityKind::Font => {
                if let Some(key) = c.id.strip_prefix("font/") {
                    for it in key_users(project, key).1 {
                        note(&mut items, &it, c.op);
                    }
                }
            }
            EntityKind::Timeline
                if c.change == ChangeKind::Added || c.path.as_deref() == Some("resolution") =>
            {
                note(&mut timelines, &c.id, c.op);
            }
            EntityKind::Track if c.change == ChangeKind::Added => note(&mut tracks, &c.id, c.op),
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
                let op = [
                    items.get(&it.id),
                    styles.get(&block.style),
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
