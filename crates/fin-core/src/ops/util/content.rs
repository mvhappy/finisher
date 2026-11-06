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
