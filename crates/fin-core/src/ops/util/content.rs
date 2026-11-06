//! Item-relative times are content-anchored (plan 4C Decision 13): keyframe `at`s and caption
//! word times count from the item's start, so a start-edge move by `d` moves them by `−d`
//! (the content offset `by` a change records). Out-of-range values are kept, so an inverse
//! moves them back exactly; evaluation clamps.

use crate::error::CoreError;
use crate::model::{Item, ItemBody};
use crate::time::Rational;

/// Moves every item-relative time of `item` by `by`.
pub(crate) fn shift_content(item: &mut Item, by: Rational) -> Result<(), CoreError> {
    if by.is_zero() {
        return Ok(());
    }
    for keys in item.common.keyframes.values_mut() {
        for k in keys {
            k.at = k.at.checked_add(by)?;
        }
    }
    if let ItemBody::Caption { block } = &mut item.body {
        for w in &mut block.words {
            w.start = w.start.checked_add(by)?;
            w.end = w.end.checked_add(by)?;
        }
    }
    Ok(())
}

/// Moves the start edge of `item` by `delta` under its content: the source moves with the edge
/// (`sourceIn += delta × speed` for a clip, `+ delta` for a nested item, plan 4C Decision 14),
/// `duration −= delta`, and every item-relative time moves by `−delta`. `start` is the
/// caller's: a ripple trim keeps it, every other start-edge move adds `delta`. Returns the
/// content offset `by = −delta` the change records.
pub(crate) fn move_start_edge(item: &mut Item, delta: Rational) -> Result<Rational, CoreError> {
    match &mut item.body {
        ItemBody::Clip {
            source_in, speed, ..
        } => *source_in = source_in.checked_add(delta.checked_mul(*speed)?)?,
        ItemBody::Nested { source_in, .. } => *source_in = source_in.checked_add(delta)?,
        _ => {}
    }
    item.duration = item.duration.checked_sub(delta)?;
    let by = delta.checked_neg()?;
    shift_content(item, by)?;
    Ok(by)
}
