//! How far an edge may move (policy): an item keeps at least one frame of its timeline, and a
//! clip shows only its asset's source (a nested item's source starts at 0).

use serde_json::json;

use crate::error::CoreError;
use crate::model::{ItemBody, Project};
use crate::ops::Edge;
use crate::ops::util::{TrackAt, asset, frame_of, invalid_range, media_duration};
use crate::time::Rational;

/// A delta range; `None` is unbounded on that side.
pub(crate) type Limits = (Option<Rational>, Option<Rational>);

/// The deltas a move of `edge` of item `index` on `ti` may take with its content anchored
/// (a start-edge move takes the source with it, as every trim mode does): the item keeps one
/// frame, and its source stays inside the asset.
pub(crate) fn edge_limits(
    project: &Project,
    ti: TrackAt,
    index: usize,
    edge: Edge,
) -> Result<Limits, CoreError> {
    let it = &ti.get(project).items[index];
    let frame = frame_of(project, ti)?;
    // The source a move may reveal: clips are bounded by their asset, nested items by 0.
    let (source_in, speed, length) = match &it.body {
        ItemBody::Clip {
            asset: a,
            source_in,
            speed,
        } => (
            Some(*source_in),
            *speed,
            Some(media_duration(asset(project, a)?)?),
        ),
        ItemBody::Nested { source_in, .. } => (Some(*source_in), Rational::from_int(1), None),
        _ => (None, Rational::from_int(1), None),
    };
    Ok(match edge {
        Edge::Start => (
            source_in
                .map(|s| s.checked_neg()?.checked_div(speed))
                .transpose()?,
            Some(it.duration.checked_sub(frame)?),
        ),
        Edge::End => (
            Some(frame.checked_sub(it.duration)?),
            match (source_in, length) {
                (Some(s), Some(l)) => Some(
                    l.checked_sub(s)?
                        .checked_div(speed)?
                        .checked_sub(it.duration)?,
                ),
                _ => None,
            },
        ),
    })
}

/// The tighter of two ranges.
pub(crate) fn intersect((a_min, a_max): Limits, (b_min, b_max): Limits) -> Limits {
    let pick =
        |a: Option<Rational>, b: Option<Rational>, f: fn(Rational, Rational) -> Rational| match (
            a, b,
        ) {
            (Some(a), Some(b)) => Some(f(a, b)),
            (a, b) => a.or(b),
        };
    (pick(a_min, b_min, Ord::max), pick(a_max, b_max, Ord::min))
}

/// `INVALID_RANGE` unless `delta` lies in `limits`.
pub(crate) fn check_delta(
    id: &str,
    edge: Edge,
    delta: Rational,
    limits: Limits,
) -> Result<(), CoreError> {
    let (min, max) = limits;
    if !(min.is_some_and(|m| delta < m) || max.is_some_and(|m| delta > m)) {
        return Ok(());
    }
    let range = |v: Option<Rational>| v.map_or("unbounded".to_string(), |v| v.to_string());
    Err(invalid_range(
        format!("trimming item {id} by {delta} would leave its source range or go below one frame"),
        json!({ "clip": id, "item": id, "edge": edge, "delta": delta, "min": min, "max": max }),
        format!("use a delta between {} and {}", range(min), range(max)),
    ))
}
