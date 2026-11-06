//! Carving a range out of a track (overwrite edits, plan 4C Decision 18).

use serde_json::json;

use crate::error::CoreError;
use crate::model::Track;
use crate::ops::util::{OpDiff, end, move_start_edge};
use crate::ops::{DeleteMode, Edge, ItemRestore, ItemTrim, Op, TrimMode};
use crate::time::Rational;
use crate::tx::{ChangeKind, EntityKind};

/// Clears `[s, e)` on `track`, moving nothing else. Items fully inside it are removed (inverse:
/// `item.restore` in `lift` mode); one overlapping `s` is trimmed at its end, one overlapping
/// `e` at its start (`updated` with `edge`, and `by` on the start edge: its content stays
/// where it was on the timeline). An item strictly containing `[s, e)` is
/// `OVERWRITE_SPLITS_ITEM` in every apply mode, and nothing changes: an op allocates at most
/// one id (Decision 17), so it is split first (`compose::overwrite`).
///
/// Returns the inverse ops: the `strict` trims back, then the restores. Neither carves, so an
/// undo that meets a later edit of the range is `CLIP_OVERLAP` (B4).
pub(crate) fn carve(
    track: &mut Track,
    s: Rational,
    e: Rational,
    d: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    if s >= e {
        return Ok(Vec::new());
    }
    for it in &track.items {
        if it.start < s && end(it)? > e {
            return Err(splits(&it.id, it.start, end(it)?, s, e));
        }
    }
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
            d.record(EntityKind::Item, &item.id, ChangeKind::Removed);
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
            d.record_edge(EntityKind::Item, &it.id, Edge::End, None);
            (Edge::End, delta)
        } else {
            let delta = e.checked_sub(start)?;
            let by = move_start_edge(it, delta)?;
            it.start = e;
            d.record_edge(EntityKind::Item, &it.id, Edge::Start, Some(by));
            (Edge::Start, delta)
        };
        trims.push(
            ItemTrim {
                item: it.id.clone(),
                edge,
                delta: delta.checked_neg()?,
                mode: TrimMode::Strict,
            }
            .into(),
        );
        i += 1;
    }
    trims.extend(restores);
    Ok(trims)
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
