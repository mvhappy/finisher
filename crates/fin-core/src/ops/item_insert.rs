//! `item.insert`: places an item of any kind on a track.
//!
//! - `ripple`: at a boundary (or in a gap / past the end); later items on the track move by
//!   its duration.
//! - `overwrite` (plan 4C Decision 18): anywhere; the range it covers is carved
//!   ([`crate::ops::util::carve`]: covered items removed, overlapping ones trimmed, an item
//!   strictly containing it `OVERWRITE_SPLITS_ITEM`). Nothing moves and no shift is recorded.
//!   Its inverse is `item.remove` (lift), then the carve's inverses (`strict` trims back, lift
//!   restores), so an undo never carves.
//!
//! Per kind (plan 4C Task 7; policy unless noted):
//! - `clip` on a video or audio track needs a `media` asset with that stream
//!   (`TRACK_KIND_MISMATCH`, structural) and a positive speed (structural);
//! - `image` (an `image`/`svg` asset), `graphic` (a `lottieTemplate` asset) and `adjustment`
//!   go on video tracks; `caption` goes on caption tracks;
//! - a caption's style exists and its overrides are style values (structural; ranges policy),
//!   its words are at least one, ordered and not overlapping (structural) and inside the item
//!   (policy, plan 4C Task 19);
//! - `nested` is `INVALID_OP` (its edits arrive in sub-project 6);
//! - a graphic's slots must be its template's (`SLOT_UNKNOWN`) with values of the slot's kind
//!   (`PARAM_TYPE`), both structural, in range (policy; plan 4C Task 11).

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Asset, AssetKind, Item, ItemBody, ParamValue, Project, Track, TrackKind};
use crate::ops::util::param_check::check_value;
use crate::ops::util::{
    OpDiff, TrackAt, asset, carve, check_caption, check_carve, check_frame, check_kind,
    check_source, check_speed, check_unlocked, clip_containing, frame_of, insert_inside,
    invalid_range, kind_name, normalize_body, place, policy_frame, ripple, track_index, wrong_op,
};
use crate::ops::{Bindings, DeleteMode, InsertMode, ItemInsert, ItemRemove, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff};

fn payload(op: &Op) -> Result<&ItemInsert, CoreError> {
    match op {
        Op::ItemInsert(i) => Ok(i),
        _ => Err(wrong_op(op)),
    }
}

/// Checks an item `body` against the track it goes on and the asset it names (resolved through
/// `bindings`): the structural checks always, the policy ones unless replaying. Shared with
/// `item.restore`.
pub(crate) fn check_body(
    project: &Project,
    track: &Track,
    body: &ItemBody,
    duration: Rational,
    frame: Rational,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    let named = |a: &str| -> Result<&Asset, CoreError> { asset(project, &bindings.resolve(a)?) };
    let (want, assets): (TrackKind, &[AssetKind]) = match body {
        ItemBody::Clip {
            asset: a,
            source_in,
            speed,
        } => {
            let a = named(a)?;
            check_kind(track, a)?;
            check_speed(*speed)?;
            if bindings.policy() {
                check_source(frame, a, *source_in, duration, *speed)?;
            }
            return Ok(());
        }
        // Nested items are refused by `item.insert` itself; what a restore puts back is
        // judged by sub-project 6.
        ItemBody::Nested { .. } => return Ok(()),
        ItemBody::Image { asset: a } => {
            named(a)?;
            (TrackKind::Video, &[AssetKind::Image, AssetKind::Svg])
        }
        ItemBody::Graphic { asset: a, slots } => {
            // Slots are checked against the template's (plan 4C Task 11).
            if let Some(template) = &named(a)?.lottie {
                for (slot, v) in slots {
                    let spec = crate::params::slot_spec(template, a, slot)?;
                    let path = format!("slots.{slot}");
                    check_value(project, spec, &path, &mut v.clone(), bindings)?;
                }
            }
            (TrackKind::Video, &[AssetKind::LottieTemplate])
        }
        ItemBody::Adjustment {} => (TrackKind::Video, &[]),
        ItemBody::Caption { block } => {
            check_caption(project, block, None, bindings)?;
            (TrackKind::Caption, &[])
        }
    };
    if !bindings.policy() {
        return Ok(());
    }
    check_frame(frame, duration)?;
    let kind = body.kind_name();
    if track.kind != want {
        return Err(CoreError::new(
            "TRACK_KIND_MISMATCH",
            format!(
                "a {kind} item cannot go on {} track {}",
                kind_name(track.kind),
                track.id
            ),
        )
        .with_details(
            json!({ "track": track.id, "trackKind": kind_name(track.kind), "kind": kind }),
        )
        .with_fix(format!("put it on a {} track", kind_name(want))));
    }
    if let Some(a) = body.asset() {
        let a = named(a)?;
        if !assets.contains(&a.kind) {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("asset {} cannot be shown by a {kind} item", a.id),
            )
            .with_details(json!({ "asset": a.id, "assetKind": a.kind, "kind": kind }))
            .with_fix(match body {
                ItemBody::Image { .. } => "use an image or svg asset",
                _ => "use a lottieTemplate asset",
            }));
        }
    }
    Ok(())
}

/// Resolves the ids a body names through `bindings`.
pub(crate) fn resolve_body(body: &mut ItemBody, bindings: &Bindings) -> Result<(), CoreError> {
    for r in body.refs_mut() {
        *r = bindings.resolve(r)?;
    }
    Ok(())
}

pub(crate) fn validate(
    project: &Project,
    op: &Op,
    bindings: &Bindings,
) -> Result<TrackAt, CoreError> {
    let i = payload(op)?;
    if let ItemBody::Nested { .. } = i.body {
        return Err(
            CoreError::new("INVALID_OP", "nested items cannot be inserted yet")
                .with_details(json!({ "kind": "nested" }))
                .with_fix("nested items arrive in sub-project 6"),
        );
    }
    let ti = track_index(project, &bindings.resolve(&i.track)?)?;
    let track = ti.get(project);
    check_body(
        project,
        track,
        &i.body,
        i.duration,
        frame_of(project, ti)?,
        bindings,
    )?;
    // Words of a new caption lie inside it (policy; later edits may move them out, Decision 13).
    if let ItemBody::Caption { block } = &i.body {
        check_caption(project, block, Some(i.duration), bindings)?;
    }
    // Policy (skipped on replay). An insert inside an item also overlaps it, which replay
    // still rejects through the structural track check.
    if bindings.policy() {
        check_unlocked(track, None, bindings)?;
        if i.at < Rational::zero() {
            return Err(invalid_range(
                "insert position must not be negative",
                json!({ "at": i.at, "min": Rational::zero() }),
                "insert at 0 or later",
            ));
        }
        if i.mode == InsertMode::Ripple
            && let Some(other) = clip_containing(track, i.at)?
        {
            return Err(insert_inside(other, i.at)?);
        }
    }
    // Structural (the one-frame remainder is policy), and before the id is allocated (an op
    // that fails takes no id).
    if i.mode == InsertMode::Overwrite {
        let frame = policy_frame(project, ti, bindings)?;
        check_carve(track, i.at, i.at.checked_add(i.duration)?, frame)?;
    }
    bindings.check_new(project, &i.id, IdKind::of_item(&i.body))?;
    Ok(ti)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let ti = validate(project, op, bindings)?;
    let i = payload(op)?;
    let mut body = i.body.clone();
    normalize_body(&mut body)?;
    resolve_body(&mut body, bindings)?;
    let id = bindings.create(project, &i.id, IdKind::of_item(&body), ids)?;
    let frame = policy_frame(project, ti, bindings)?;
    let track = ti.get_mut(project);
    let carved = match i.mode {
        InsertMode::Ripple => {
            ripple(track, i.at, i.duration)?;
            Vec::new()
        }
        InsertMode::Overwrite => carve(track, i.at, i.at.checked_add(i.duration)?, frame, diff)?,
    };
    place(track, Item::new(id.clone(), i.at, i.duration, body));
    diff.record_item(&track.id, &id, ChangeKind::Added);
    let mode = match i.mode {
        InsertMode::Ripple => {
            diff.shift(&track.id, i.at, i.duration);
            DeleteMode::Ripple
        }
        InsertMode::Overwrite => DeleteMode::Lift,
    };
    let mut inverse = vec![ItemRemove { id, mode }.into()];
    inverse.extend(carved);
    Ok(inverse)
}

/// Creates the item on its track, using the track and the asset it shows (an overwrite's diff
/// adds the items it removed and the edges it trimmed).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::ItemInsert(i) = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(&i.track), op_diff);
    f.creates.push(i.id.clone());
    f.uses.extend(i.body.asset().map(str::to_string));
    if let ItemBody::Caption { block } = &i.body
        && !block.style.starts_with(crate::captions::PRESET_PREFIX)
    {
        f.uses.push(block.style.clone());
    }
    if let ItemBody::Graphic { slots, .. } = &i.body {
        f.uses.extend(
            slots
                .values()
                .filter_map(ParamValue::asset)
                .map(str::to_string),
        );
    }
    f.uses.push(i.track.clone());
    f
}

/// `at`, on the insert's track.
pub(crate) fn positions_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::ItemInsert(i) => vec![(i.track.clone(), &mut i.at)],
        _ => Vec::new(),
    }
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// A ripple insert inverts to one `item.remove`; an overwrite to that plus one op per item it
/// carved (each records one change, after which the insert records its `added` one).
pub(crate) fn inverse_len(op: &Op, own: &Diff) -> usize {
    match op {
        Op::ItemInsert(i) if i.mode == InsertMode::Overwrite => own.changes.len(),
        _ => 1,
    }
}

/// The wire name.
pub(crate) const NAME: &str = "item.insert";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the new id, the track and what the body names.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    let Op::ItemInsert(i) = op else {
        return Vec::new();
    };
    let ItemInsert {
        id, track, body, ..
    } = &mut **i;
    let mut out = vec![id, track];
    out.extend(body.refs_mut());
    out
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    let Op::ItemInsert(i) = op else {
        return Vec::new();
    };
    let mut out = vec![i.id.as_str(), i.track.as_str()];
    out.extend(i.body.ids());
    out
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::ItemInsert(i) => Some(i.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
