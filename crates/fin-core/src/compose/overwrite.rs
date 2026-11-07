//! `compose::overwrite`: an overwrite insert that may land inside one item. An op allocates at
//! most one id (plan 4C Decision 17), so the overwrite never splits: this sends the split
//! first.

use serde_json::{Value, json};

use crate::compose::Composed;
use crate::error::CoreError;
use crate::model::{ItemBody, Project};
use crate::ops::util::{end, track_index};
use crate::ops::{InsertMode, ItemInsert, ItemSplit};
use crate::time::Rational;

/// The placeholder of the split's right part.
const SPLIT: &str = "$split";

/// Ops for an overwrite of `[at, at + duration)` on `track` with an item of `body` named `id`
/// (a `$name` placeholder): the `item.split` of the one item strictly containing the range at
/// `at` (its right part `$split`, or `$split_` when `id` is `$split`), then the `item.insert`
/// in `overwrite` mode. `echo` is `{ mode: "overwrite", split: { item, offset, id } | null }`.
/// `NOT_FOUND` for an unknown track; everything else is the ops' own validation.
pub fn overwrite(
    project: &Project,
    track: &str,
    at: Rational,
    duration: Rational,
    body: ItemBody,
    id: &str,
) -> Result<Composed, CoreError> {
    let right = if id == SPLIT { "$split_" } else { SPLIT };
    overwrite_named(project, track, at, duration, body, id, right)
}

/// [`overwrite`] with the split's right part named `right` (builders that overwrite on
/// several tracks name each split apart).
pub(crate) fn overwrite_named(
    project: &Project,
    track: &str,
    at: Rational,
    duration: Rational,
    body: ItemBody,
    id: &str,
    right: &str,
) -> Result<Composed, CoreError> {
    let t = track_index(project, track)?.get(project);
    let stop = at.checked_add(duration)?;
    let mut containing = None;
    for it in &t.items {
        if it.start < at && end(it)? > stop {
            containing = Some(it);
        }
    }
    let mut ops = Vec::new();
    let mut split = Value::Null;
    if let Some(it) = containing {
        let offset = at.checked_sub(it.start)?;
        split = json!({ "item": it.id, "offset": offset, "id": right });
        ops.push(
            ItemSplit {
                item: it.id.clone(),
                offset,
                id: right.to_string(),
            }
            .into(),
        );
    }
    ops.push(
        ItemInsert {
            id: id.to_string(),
            track: track.to_string(),
            at,
            duration,
            mode: InsertMode::Overwrite,
            body,
        }
        .into(),
    );
    Ok(Composed {
        ops,
        echo: json!({ "mode": "overwrite", "split": split }),
    })
}
