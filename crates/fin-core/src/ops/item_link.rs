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
use crate::ops::util::{OpDiff, TrackAt, check_unlocked, item_index, wrong_op};
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
    if items.len() < 2 {
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
    let id = bindings.create(project, &l.id, IdKind::Link, ids)?;
    diff.record(EntityKind::Link, &id, ChangeKind::Added);
    for (item, (t, i)) in items.iter().zip(places) {
        let track = t.get_mut(project);
        track.items[i].common.link = Some(id.clone());
        diff.record_path(&track.id, item, LINK);
    }
    Ok(vec![ItemUnlink { group: id }.into()])
}

/// Sets each item's `link` and creates the group (Decision 20).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ItemLink(l) = op {
        f.touches.extend(
            l.items
                .iter()
                .map(|i| (i.clone(), Touch::Param(LINK.to_string()))),
        );
        f.creates.push(l.id.clone());
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
