//! Source times (plan 4C Decision 14): a clip reads `sourceIn + local × speed`, a nested item
//! evaluates its timeline at `sourceIn + local`. Exact rationals; the source extent is policy,
//! so a time past the asset (a transition's handle, a slip) is reported as is.

use crate::error::CoreError;
use crate::eval::plan::SourceRef;
use crate::model::{Item, ItemBody};
use crate::time::Rational;

/// `sourceIn + local × speed`.
pub(crate) fn clip_time(
    source_in: Rational,
    speed: Rational,
    local: Rational,
) -> Result<Rational, CoreError> {
    source_in.checked_add(local.checked_mul(speed)?)
}

/// The asset an item shows and the time to read from it: a clip's source time, an image's 0,
/// a graphic's template time (`local`). Other kinds have none.
pub(crate) fn source_of(item: &Item, local: Rational) -> Result<Option<SourceRef>, CoreError> {
    let r = |asset: &String, time| {
        Some(SourceRef {
            asset: asset.clone(),
            time,
        })
    };
    Ok(match &item.body {
        ItemBody::Clip {
            asset,
            source_in,
            speed,
        } => r(asset, clip_time(*source_in, *speed, local)?),
        ItemBody::Image { asset } => r(asset, Rational::zero()),
        ItemBody::Graphic { asset, .. } => r(asset, local),
        ItemBody::Caption { .. } | ItemBody::Adjustment {} | ItemBody::Nested { .. } => None,
    })
}
