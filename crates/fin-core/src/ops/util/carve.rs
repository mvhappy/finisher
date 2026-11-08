//! Carving a range out of a track (overwrite edits, plan 4C Decision 18).

use serde_json::json;

use crate::error::CoreError;
use crate::model::Track;
use crate::ops::util::{OpDiff, end, invalid_range, move_start_edge};
use crate::ops::{DeleteMode, Edge, ItemRestore, ItemTrim, Op, TrimMode};
use crate::time::Rational;
use crate::tx::ChangeKind;

/// Clears `[s, e)` on `track`, moving nothing else. Items fully inside it are removed (inverse:
/// `item.restore` in `lift` mode); one overlapping `s` is trimmed at its end, one overlapping
/// `e` at its start (`updated` with `edge`, and `by` on the start edge: its content stays
/// where it was on the timeline). An item strictly containing `[s, e)` is
/// `OVERWRITE_SPLITS_ITEM` in every apply mode, and nothing changes: an op allocates at most
/// one id (Decision 17), so it is split first (`compose::overwrite`). With `frame` (policy,
/// `None` on replay), a trimmed item keeps at least one frame: a range edge less than a frame
/// inside an item is `INVALID_RANGE`, and nothing changes.
///
/// Returns the inverse ops: the `strict` trims back, then the restores. Neither carves, so an
/// undo that meets a later edit of the range is `CLIP_OVERLAP` (B4).
pub(crate) fn carve(
    track: &mut Track,
    s: Rational,
    e: Rational,
    frame: Option<Rational>,
    d: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    if s >= e {
        return Ok(Vec::new());
    }
    check_carve(track, s, e, frame)?;
    let mut trims = Vec::new();
    let mut restores = Vec::new();
    let mut i = 0;
    while i < track.items.len() {
        let it = &mut track.items[i];
        let (start, stop) = (it.start, end(it)?);
        if stop <= s || start >= e {
            i += 1;
            continue;
        }
        if start >= s && stop <= e {
            let item = track.items.remove(i);
            d.record_item(&track.id, &item.id, ChangeKind::Removed);
            restores.push(
                ItemRestore {
                    item,
                    track: track.id.clone(),
                    mode: DeleteMode::Lift,
                }
                .into(),
            );
            continue;
        }
        let (edge, delta) = if start < s {
            let delta = s.checked_sub(stop)?;
            it.duration = it.duration.checked_add(delta)?;
            d.record_edge(&track.id, &it.id, Edge::End, None);
            (Edge::End, delta)
        } else {
            let delta = e.checked_sub(start)?;
            let by = move_start_edge(it, delta)?;
            it.start = e;
            d.record_edge(&track.id, &it.id, Edge::Start, Some(by));
            (Edge::Start, delta)
        };
        trims.push(
            ItemTrim {
                item: it.id.clone(),
                edge,
                delta: delta.checked_neg()?,
                mode: TrimMode::Strict,
                alone: false,
            }
            .into(),
        );
        i += 1;
    }
    trims.extend(restores);
    Ok(trims)
}

/// What [`carve`] refuses, checked before an op allocates anything: `OVERWRITE_SPLITS_ITEM`
/// when an item on `track` strictly contains `[s, e)` (structural); with `frame` (policy), an
/// `INVALID_RANGE` when the carve would leave an item it trims shorter than one frame (a range
/// edge off the frame grid, less than a frame inside the item).
pub(crate) fn check_carve(
    track: &Track,
    s: Rational,
    e: Rational,
    frame: Option<Rational>,
) -> Result<(), CoreError> {
    for it in &track.items {
        let (start, stop) = (it.start, end(it)?);
        if start < s && stop > e {
            return Err(splits(&it.id, start, stop, s, e));
        }
        if stop <= s || start >= e {
            continue;
        }
        let remains = if start < s {
            s.checked_sub(start)?
        } else if stop > e {
            stop.checked_sub(e)?
        } else {
            continue;
        };
        if let Some(frame) = frame
            && remains < frame
        {
            return Err(too_short(&it.id, start, stop, s, e, remains, frame));
        }
    }
    Ok(())
}

fn too_short(
    id: &str,
    start: Rational,
    stop: Rational,
    s: Rational,
    e: Rational,
    remains: Rational,
    frame: Rational,
) -> CoreError {
    invalid_range(
        format!(
            "overwriting [{s}, {e}) would leave item {id} {remains}s long, shorter than one \
             frame ({frame}s)"
        ),
        json!({
            "item": id, "start": start, "end": stop, "from": s, "to": e,
            "remains": remains, "min": frame,
        }),
        format!("cover item {id} whole ({start} to {stop}), or leave at least {frame}s of it"),
    )
}

fn splits(id: &str, start: Rational, stop: Rational, s: Rational, e: Rational) -> CoreError {
    CoreError::new(
        "OVERWRITE_SPLITS_ITEM",
        format!("[{s}, {e}) lies strictly inside item {id}: overwriting it would split the item"),
    )
    .with_details(json!({ "item": id, "start": start, "end": stop, "from": s, "to": e }))
    .with_fix(format!(
        "split item {id} at {s} first (item.split), or let fin do it (fin overwrites through \
         compose::overwrite)"
    ))
}
