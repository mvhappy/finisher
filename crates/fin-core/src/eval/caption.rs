//! A caption layer's words and layout. Measurement never blocks a plan (plan 4C Decision 36):
//! without a layout the plan says why — `noMeasurer`, `fontUnpinned` (the keys with no pin) or
//! `fontMissing` (the pinned keys whose bytes the measurer lacks). The request is the one the
//! edit path's overflow check builds ([`measure_caption`]).

use crate::captions::{measure_caption, shown_words};
use crate::error::CoreError;
use crate::eval::plan::{CaptionPlan, Unmeasured, UnmeasuredReason, WordPlan};
use crate::layout::TextMeasurer;
use crate::model::{CaptionBlock, Item, Project, Resolution};
use crate::time::Rational;

/// The caption's shown words ([`shown_words`]: item-relative times clamped to
/// `[0, duration]`; words a trim hides are not drawn), the spoken one marked, and its layout or the reason it has none. `Err` only when the measurer refuses the
/// request.
pub(crate) fn caption_plan(
    project: &Project,
    item: &Item,
    block: &CaptionBlock,
    local: Rational,
    resolution: Resolution,
    m: Option<&dyn TextMeasurer>,
) -> Result<CaptionPlan, CoreError> {
    let words = shown_words(&block.words, item.duration)
        .into_iter()
        .map(|w| WordPlan {
            active: w.start <= local && local < w.end,
            text: w.text,
            start: w.start,
            end: w.end,
        })
        .collect();
    let measured = match m {
        Some(m) => measure_caption(project, block, item.duration, resolution, m)?,
        None => Err(Unmeasured {
            reason: UnmeasuredReason::NoMeasurer,
            keys: Vec::new(),
        }),
    };
    let (layout, unmeasured) = match measured {
        Ok(layout) => (Some(layout), None),
        Err(why) => (None, Some(why)),
    };
    Ok(CaptionPlan {
        words,
        style: block.style.clone(),
        layout,
        unmeasured,
    })
}
