//! `item.trim` in `roll` mode (plan 4C Decision 18): the cut between two touching items
//! moves; and the trim that moves an edge back, every trim mode's inverse.

use serde_json::json;

use crate::error::CoreError;
use crate::model::Track;
use crate::ops::util::{OpDiff, end, move_start_edge};
use crate::ops::{Edge, ItemTrim, Op, TrimMode};
use crate::tx::EntityKind;

/// The item touching `edge` of item `index` (`NOT_ADJACENT` when none does).
pub(crate) fn neighbour(track: &Track, index: usize, edge: Edge) -> Result<usize, CoreError> {
    let it = &track.items[index];
    let found = match edge {
        Edge::End => track
            .items
            .get(index + 1)
            .filter(|n| end(it).is_ok_and(|e| n.start == e))
            .map(|_| index + 1),
        Edge::Start => index
            .checked_sub(1)
            .filter(|&i| end(&track.items[i]).is_ok_and(|e| e == it.start)),
    };
    found.ok_or_else(|| {
        CoreError::new(
            "NOT_ADJACENT",
            format!(
                "item {} has no neighbour touching its {} edge to roll with",
                it.id,
                match edge {
                    Edge::Start => "start",
                    Edge::End => "end",
                }
            ),
        )
        .with_details(json!({ "item": it.id, "edge": edge, "track": track.id }))
        .with_fix("roll a cut between two touching items, or trim with mode ripple or overwrite")
    })
}

/// The cut between item `index` and its neighbour `n` moves by `t.delta`: the left one's end
/// and the right one's start (with its source and content). The named item's change is
/// recorded first.
pub(crate) fn roll(
    track: &mut Track,
    index: usize,
    n: usize,
    t: &ItemTrim,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let (left, right) = match t.edge {
        Edge::End => (index, n),
        Edge::Start => (n, index),
    };
    let l = &mut track.items[left];
    l.duration = l.duration.checked_add(t.delta)?;
    let l_id = l.id.clone();
    let r = &mut track.items[right];
    let by = move_start_edge(r, t.delta)?;
    r.start = r.start.checked_add(t.delta)?;
    let r_id = r.id.clone();
    let left_change = |d: &mut OpDiff<'_>| d.record_edge(EntityKind::Item, &l_id, Edge::End, None);
    let right_change =
        |d: &mut OpDiff<'_>| d.record_edge(EntityKind::Item, &r_id, Edge::Start, Some(by));
    let named = if t.edge == Edge::End {
        left_change(diff);
        right_change(diff);
        &l_id
    } else {
        right_change(diff);
        left_change(diff);
        &r_id
    };
    Ok(vec![back(named, t, TrimMode::Roll)?])
}

/// The trim that moves the edge back, in `mode`.
pub(crate) fn back(id: &str, t: &ItemTrim, mode: TrimMode) -> Result<Op, CoreError> {
    Ok(ItemTrim {
        item: id.to_string(),
        edge: t.edge,
        delta: t.delta.checked_neg()?,
        mode,
    }
    .into())
}
