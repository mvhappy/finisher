//! Measuring a caption (plan 4C Task 20, Decision 36): the one request both the edit path's
//! overflow check and eval build, from the caption's shown words ([`shown_words`]), its
//! effective style, its pinned faces ([`faces_for`]) and its timeline's safe zone.
//!
//! - No pin for a face or a script → unmeasured `fontUnpinned` (the keys that lack one);
//! - the measurer lacks bytes → unmeasured `fontMissing` (the pinned keys of the files);
//! - otherwise a layout. The overflow check refuses a layout that overflows
//!   (`CAPTION_OVERFLOW`); an unmeasured caption is never checked and never blocks an edit.
//!
//! Nothing measured is stored: the check accepts or refuses, eval plans carry layouts.

use std::collections::BTreeSet;

use serde_json::json;

use crate::captions::{LayoutInputs, resolve_style, safe_box, safe_zone, shown_words, to_check_at};
use crate::error::CoreError;
use crate::eval::{Unmeasured, UnmeasuredReason};
use crate::layout::{Measured, TextLayout, TextMeasurer, TextRequest, faces_for};
use crate::model::{CaptionBlock, CaptionStyle, ItemBody, Project, Resolution};
use crate::time::Rational;
use crate::tx::Diff;

fn unmeasured(reason: UnmeasuredReason, keys: Vec<String>) -> Unmeasured {
    Unmeasured { reason, keys }
}

/// The layout box width of `style` on a frame of `resolution`, in timeline pixels (the
/// timeline's resolution, as every style length is): the safe zone's width less the
/// background's padding on both sides (`safeZoneWidth − 2 × background.padding`, no
/// background being no padding), floored at 0. The stroke is not counted (a stroked glyph
/// may reach past the box by the stroke's width). `None` for an unknown zone (no width
/// limit).
fn box_width(style: &CaptionStyle, resolution: Resolution) -> Option<f64> {
    let zone = safe_zone(&style.safe_zone)?;
    let padding = style.background.as_ref().map_or(0.0, |b| b.padding);
    Some((safe_box(zone, resolution)[2] - 2.0 * padding).max(0.0))
}

/// The request that measures `block` (of an item `duration` long, whose shown words are
/// measured) on a timeline of `resolution`, or why it cannot be measured (`fontUnpinned`: a
/// style that does not resolve, or faces and scripts with no pin).
pub fn caption_request(
    project: &Project,
    block: &CaptionBlock,
    duration: Rational,
    resolution: Resolution,
) -> Result<TextRequest, Unmeasured> {
    let style = resolve_style(project, block)
        .map_err(|_| unmeasured(UnmeasuredReason::FontUnpinned, Vec::new()))?;
    let shown = shown_words(&block.words, duration);
    let faces = faces_for(project, &style, &shown)
        .map_err(|keys| unmeasured(UnmeasuredReason::FontUnpinned, keys))?;
    Ok(TextRequest {
        words: shown.into_iter().map(|w| w.text).collect(),
        max_width: box_width(&style, resolution),
        max_lines: Some(style.max_lines),
        style: style.text,
        faces,
    })
}

/// `block`'s layout (on an item `duration` long) through `m`, or why there is none. `Err` only
/// when the measurer refuses the request (a style value out of its range).
pub fn measure_caption(
    project: &Project,
    block: &CaptionBlock,
    duration: Rational,
    resolution: Resolution,
    m: &dyn TextMeasurer,
) -> Result<Result<TextLayout, Unmeasured>, CoreError> {
    let req = match caption_request(project, block, duration, resolution) {
        Ok(req) => req,
        Err(why) => return Ok(Err(why)),
    };
    Ok(match m.measure(&req)? {
        Measured::Layout(layout) => Ok(layout),
        Measured::Unavailable { missing } => {
            let mut keys: Vec<String> = project
                .fonts
                .pins
                .iter()
                .filter(|(_, p)| missing.contains(&p.sha256))
                .map(|(k, _)| k.clone())
                .collect();
            if keys.is_empty() {
                keys = missing;
            }
            Err(unmeasured(UnmeasuredReason::FontMissing, keys))
        }
    })
}

/// 1/64 px as px.
fn px(v: i32) -> f64 {
    f64::from(v) / 64.0
}

/// The end-of-transaction overflow check (Decision 36): each caption [`to_check_at`] lists
/// (`before`: the layout inputs the transaction started from; `restores`: its restore ops),
/// measured once at its timeline's resolution. A layout that overflows is `CAPTION_OVERFLOW
/// {item, lines, maxLines, width, maxWidth, missing, opIndex}` (widths in px, `missing` the
/// code points no face maps, `opIndex` the last op that made the caption need a check). An
/// unmeasured caption is skipped.
pub fn check_overflow(
    before: &LayoutInputs,
    project: &Project,
    diff: &Diff,
    restores: &BTreeSet<u32>,
    m: &dyn TextMeasurer,
) -> Result<(), CoreError> {
    for (id, op) in to_check_at(before, project, diff, restores) {
        let Some((block, duration, resolution)) = caption_on(project, &id) else {
            continue;
        };
        let Ok(layout) = measure_caption(project, block, duration, resolution, m)? else {
            continue;
        };
        if !layout.overflow {
            continue;
        }
        let style = resolve_style(project, block)?;
        let max_width = box_width(&style, resolution);
        return Err(CoreError::new(
            "CAPTION_OVERFLOW",
            format!(
                "caption {id} does not fit its safe zone: {} lines of {:.1} px (at most {} of {:.1} px)",
                layout.lines.len(),
                px(layout.width),
                style.max_lines,
                max_width.unwrap_or(f64::INFINITY),
            ),
        )
        .with_details(json!({
            "item": id,
            "lines": layout.lines.len(),
            "maxLines": style.max_lines,
            "width": px(layout.width),
            "maxWidth": max_width,
            "missing": layout.missing,
            "opIndex": op,
        }))
        .with_fix("shorten the text, split the caption, or reduce the text size"));
    }
    Ok(())
}

/// Caption `id`'s block, its duration and the resolution of the timeline it is on.
fn caption_on<'a>(
    project: &'a Project,
    id: &str,
) -> Option<(&'a CaptionBlock, Rational, Resolution)> {
    project.timelines.iter().find_map(|tl| {
        tl.tracks
            .iter()
            .flat_map(|t| t.items.iter())
            .find(|it| it.id == id)
            .and_then(|it| match &it.body {
                ItemBody::Caption { block } => Some((block, it.duration, tl.resolution)),
                _ => None,
            })
    })
}
