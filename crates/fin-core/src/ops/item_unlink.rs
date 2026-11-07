//! `item.unlink`: dissolves a link group, clearing `link` on every member. The diff records the
//! group (`removed`, kind `link`), then each member's `link` parameter. The inverse is an
//! internal `item.link` of the members with the group's literal id. Internal, with `items`:
//! only those members leave (the group `updated` while others stay); the inverse joins them
//! back.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::item_link::LINK;
use crate::ops::util::{OpDiff, check_unlocked, group_items, item_index, wrong_op};
use crate::ops::{Bindings, ItemLink, ItemUnlink, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::ItemUnlink(u) = op else {
        return Err(wrong_op(op));
    };
    let group = bindings.resolve(&u.group)?;
    let all = group_items(project, &group);
    if all.is_empty() {
        return Err(
            CoreError::new("NOT_FOUND", format!("link group {group} does not exist"))
                .with_details(json!({ "id": group, "kind": "link" }))
                .with_fix("use the id of an existing link group"),
        );
    }
    let leaving = match &u.items {
        None => all.clone(),
        Some(list) => {
            if !bindings.internal() {
                return Err(CoreError::new(
                    "INVALID_OP",
                    "members leaving a link group alone is internal",
                )
                .with_details(json!({ "group": group, "items": list }))
                .with_fix("dissolve the whole group (item.unlink without items)"));
            }
            let list = list
                .iter()
                .map(|i| bindings.resolve(i))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(stranger) = list.iter().find(|i| !all.contains(i)) {
                return Err(CoreError::new(
                    "INVALID_OP",
                    format!("item {stranger} is not in link group {group}"),
                )
                .with_details(json!({ "group": group, "item": stranger })));
            }
            list
        }
    };
    let places = leaving
        .iter()
        .map(|id| item_index(project, id))
        .collect::<Result<Vec<_>, _>>()?;
    for (id, (t, _)) in leaving.iter().zip(&places) {
        check_unlocked(t.get(project), Some(id), bindings)?;
    }
    let change = if leaving.len() < all.len() {
        ChangeKind::Updated
    } else {
        ChangeKind::Removed
    };
    diff.record(EntityKind::Link, &group, change);
    for (item, (t, i)) in leaving.iter().zip(places) {
        let track = t.get_mut(project);
        track.items[i].common.link = None;
        diff.record_path(&track.id, item, LINK);
    }
    Ok(vec![
        ItemLink {
            items: leaving,
            id: group,
        }
        .into(),
    ])
}

/// Needs the group and removes it (unless only some members leave it); its members' `link`
/// comes from the diff (Decision 20).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ItemUnlink(u) = op {
        f.touches.push((u.group.clone(), Touch::Exists));
        if u.items.is_none() {
            f.removes.push(u.group.clone());
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

/// The inverse is one `item.link`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "item.unlink";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemUnlink(u) => {
            let ItemUnlink { group, items } = u;
            let mut out = vec![group];
            out.extend(items.iter_mut().flatten());
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemUnlink(u) => {
            let mut out = vec![u.group.as_str()];
            out.extend(u.items.iter().flatten().map(String::as_str));
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
