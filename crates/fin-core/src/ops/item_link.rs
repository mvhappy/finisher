//! `item.link`: makes items one link group (plan 4C Decision 19). At least two distinct items,
//! all on one timeline, none in a group already (`ALREADY_LINKED`); all structural. The group
//! id is the op's one allocation (`$name`; a literal id only in an internal apply, the inverse
//! of `item.unlink`). The diff records the group (`added`, kind `link`), then each member's
//! `link` parameter. The inverse is `item.unlink`.

use std::collections::BTreeSet;

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::Project;
use crate::ops::util::{OpDiff, TrackAt, check_unlocked, group_items, item_index, wrong_op};
use crate::ops::{Bindings, ItemLink, ItemUnlink, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// The parameter a link edit sets on each member (plan 4C Decision 20).
pub(crate) const LINK: &str = "link";

fn payload(op: &Op) -> Result<&ItemLink, CoreError> {
    match op {
        Op::ItemLink(l) => Ok(l),
        _ => Err(wrong_op(op)),
    }
}

fn invalid(message: &str, items: &[String], fix: &str) -> CoreError {
    CoreError::new("INVALID_OP", message)
        .with_details(json!({ "items": items }))
        .with_fix(fix)
}

/// The members' places, once checked.
fn validate(
    project: &Project,
    items: &[String],
    bindings: &Bindings,
) -> Result<Vec<(TrackAt, usize)>, CoreError> {
    // A one-member group comes back only as an inverse (an undo of `item.unlink` of a group
    // a removal left alone, or of the link pass dissolving one).
    if items.is_empty() || (items.len() < 2 && !bindings.internal()) {
        return Err(invalid(
            "a link group needs at least two items",
            items,
            "link two or more items",
        ));
    }
    if items.iter().collect::<BTreeSet<_>>().len() != items.len() {
        return Err(invalid(
            "a link group names an item twice",
            items,
            "name each item once",
        ));
    }
    let places = items
        .iter()
        .map(|id| item_index(project, id))
        .collect::<Result<Vec<_>, _>>()?;
    if places
        .iter()
        .any(|(t, _)| t.timeline != places[0].0.timeline)
    {
        return Err(invalid(
            "the items of a link group are on different timelines",
            items,
            "link items of one timeline",
        ));
    }
    for (id, (t, i)) in items.iter().zip(&places) {
        let track = t.get(project);
        if let Some(link) = &track.items[*i].common.link {
            return Err(CoreError::new(
                "ALREADY_LINKED",
                format!("item {id} is in link group {link} already"),
            )
            .with_details(json!({ "item": id, "link": link }))
            .with_fix(format!("unlink group {link} first (item.unlink)")));
        }
        check_unlocked(track, Some(id), bindings)?;
    }
    Ok(places)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let l = payload(op)?;
    let items = l
        .items
        .iter()
        .map(|i| bindings.resolve(i))
        .collect::<Result<Vec<_>, _>>()?;
    let places = validate(project, &items, bindings)?;
    // Internal: a literal id of an existing group is joined (an undo putting back a member a
    // removal took out of it, after the removal's own inverse restored the others).
    let joins =
        !l.id.starts_with('$') && bindings.internal() && !group_items(project, &l.id).is_empty();
    let id = if joins {
        l.id.clone()
    } else {
        bindings.create(project, &l.id, IdKind::Link, ids)?
    };
    let change = if joins {
        ChangeKind::Updated
    } else {
        ChangeKind::Added
    };
    diff.record(EntityKind::Link, &id, change);
    for (item, (t, i)) in items.iter().zip(places) {
        let track = t.get_mut(project);
        track.items[i].common.link = Some(id.clone());
        diff.record_path(&track.id, item, LINK);
    }
    Ok(vec![
        ItemUnlink {
            group: id,
            items: joins.then_some(items),
        }
        .into(),
    ])
}

/// Sets each item's `link` and creates the group (Decision 20), or needs it when the items
/// joined it.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ItemLink(l) = op {
        f.touches.extend(
            l.items
                .iter()
                .map(|i| (i.clone(), Touch::Param(LINK.to_string()))),
        );
        let joined = op_diff
            .changes
            .iter()
            .any(|c| c.kind == EntityKind::Link && c.change == ChangeKind::Updated);
        if joined {
            f.touches.push((l.id.clone(), Touch::Exists));
        } else {
            f.creates.push(l.id.clone());
        }
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The inverse is one `item.unlink`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "item.link";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the items, then the group's id.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemLink(l) => {
            let ItemLink { items, id } = l;
            let mut out: Vec<&mut String> = items.iter_mut().collect();
            out.push(id);
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemLink(l) => {
            let mut out: Vec<&str> = l.items.iter().map(String::as_str).collect();
            out.push(&l.id);
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::ItemLink(l) => Some(l.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
