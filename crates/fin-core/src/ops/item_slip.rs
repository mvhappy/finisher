//! `item.slip`: moves the source of a clip or nested item under it (`sourceIn += delta`, in
//! source seconds). Its place on the timeline and its item-relative times stay (plan 4C
//! Decision 13: a slip moves no content offset). Other item kinds have no source to slip
//! (`INVALID_OP`, in every apply mode). The source range is policy (a clip's within its asset, a
//! nested item's from 0); replay's track check still refuses a negative `sourceIn`.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ItemBody, Project};
use crate::ops::util::{
    OpDiff, asset, check_unlocked, invalid_range, item_index, media_duration, wrong_op,
};
use crate::ops::{Bindings, ItemSlip, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::Diff;

/// The parameter a slip sets (plan 4C Decision 20).
const SOURCE_IN: &str = "sourceIn";

fn payload(op: &Op) -> Result<&ItemSlip, CoreError> {
    match op {
        Op::ItemSlip(s) => Ok(s),
        _ => Err(wrong_op(op)),
    }
}

/// Checks the slip of `id`; the source range only under policy.
fn validate(
    project: &Project,
    id: &str,
    delta: Rational,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    let (ti, ci) = item_index(project, id)?;
    let track = ti.get(project);
    let it = &track.items[ci];
    let (source_in, max) = match &it.body {
        ItemBody::Clip {
            asset: a,
            source_in,
            speed,
        } => {
            let length = media_duration(asset(project, a)?)?;
            let max = length
                .checked_sub(*source_in)?
                .checked_sub(it.duration.checked_mul(*speed)?)?;
            (*source_in, Some(max))
        }
        ItemBody::Nested { source_in, .. } => (*source_in, None),
        _ => {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("a {} item has no source to slip", it.body.kind_name()),
            )
            .with_details(json!({ "item": id, "kind": it.body.kind_name() }))
            .with_fix("slip a clip or a nested item"));
        }
    };
    if !bindings.policy() {
        return Ok(());
    }
    check_unlocked(track, Some(id), bindings)?;
    let min = source_in.checked_neg()?;
    if delta < min || max.is_some_and(|m| delta > m) {
        let range = max.map_or("unbounded".to_string(), |m| m.to_string());
        return Err(invalid_range(
            format!("slipping item {id} by {delta} would leave its source range"),
            json!({ "item": id, "delta": delta, "min": min, "max": max }),
            format!("use a delta between {min} and {range}"),
        ));
    }
    Ok(())
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let s = payload(op)?;
    let id = bindings.resolve(&s.item)?;
    validate(project, &id, s.delta, bindings)?;
    let (ti, ci) = item_index(project, &id)?;
    let track = ti.get_mut(project);
    let track_id = track.id.clone();
    let it = &mut track.items[ci];
    match &mut it.body {
        ItemBody::Clip { source_in, .. } | ItemBody::Nested { source_in, .. } => {
            *source_in = source_in.checked_add(s.delta)?;
        }
        _ => return Err(wrong_op(op)),
    }
    diff.record_path(&track_id, &id, SOURCE_IN);
    Ok(vec![
        ItemSlip {
            item: id,
            delta: s.delta.checked_neg()?,
        }
        .into(),
    ])
}

/// Sets the item's `sourceIn` (Decision 20).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ItemSlip(s) = op {
        f.touches
            .push((s.item.clone(), Touch::Param(SOURCE_IN.to_string())));
    }
    f
}

/// No timeline positions: `delta` is a length.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values: `delta` is a source length.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The op's inverse is one op.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "item.slip";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemSlip(s) => vec![&mut s.item],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemSlip(s) => vec![&s.item],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
