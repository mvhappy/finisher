//! A caption layer's words and layout. Measurement never blocks a plan (plan 4C Decision 36):
//! without a layout the plan says why.
//!
//! As built in Task 15, no caption is measured yet. Task 18 gave styles their text style
//! (`captions::resolve_style`) and the face resolution from pins (`layout::faces_for`); Task
//! 20 wires them into [`measure`] here. Until then a plan reports `noMeasurer` when none was
//! given and `fontUnpinned` (no face resolves) when one was, and never calls the measurer.

use crate::eval::plan::{CaptionPlan, Unmeasured, UnmeasuredReason, WordPlan};
use crate::layout::{TextLayout, TextMeasurer};
use crate::model::{CaptionBlock, Item, Project, Resolution};
use crate::time::Rational;

/// The caption's words (item-relative times clamped to `[0, duration]`, the spoken one
/// marked) and its layout or the reason it has none.
pub(crate) fn caption_plan(
    project: &Project,
    item: &Item,
    block: &CaptionBlock,
    local: Rational,
    resolution: Resolution,
    m: Option<&dyn TextMeasurer>,
) -> CaptionPlan {
    let clamp = |t: Rational| t.max(Rational::zero()).min(item.duration);
    let words = block
        .words
        .iter()
        .map(|w| {
            let (start, end) = (clamp(w.start), clamp(w.end));
            WordPlan {
                text: w.text.clone(),
                start,
                end,
                active: start <= local && local < end,
            }
        })
        .collect();
    let (layout, unmeasured) = match measure(project, block, resolution, m) {
        Ok(layout) => (Some(layout), None),
        Err(why) => (None, Some(why)),
    };
    CaptionPlan {
        words,
        style: block.style.clone(),
        layout,
        unmeasured,
    }
}

/// The caption's layout, or why there is none.
fn measure(
    _project: &Project,
    _block: &CaptionBlock,
    _resolution: Resolution,
    m: Option<&dyn TextMeasurer>,
) -> Result<TextLayout, Unmeasured> {
    let reason = match m {
        None => UnmeasuredReason::NoMeasurer,
        Some(_) => UnmeasuredReason::FontUnpinned,
    };
    Err(Unmeasured {
        reason,
        keys: Vec::new(),
    })
}
