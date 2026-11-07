//! `keyframe.remove`: removes the keyframe of `path` of an item at item-relative time `at`
//! (`NOT_FOUND` "keyframe" when there is none: structural). Removing a path's last keyframe
//! drops the path's list, so the path is static again. No range check: a keyframe a trim
//! carried outside the item can still be removed. `at` is a content offset (Decision 16), and
//! the footprint is `keyframe.set`'s. The inverse is `keyframe.set` of the removed keyframe.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::keyframe_set::{key, keyframe_footprint};
use crate::ops::param_set::entity;
use crate::ops::util::{OpDiff, check_unlocked, item_index, not_found, wrong_op};
use crate::ops::{Bindings, KeyframeSet, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::Diff;

/// `keyframe.remove`: the keyframe of `path` of `item` at `at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeRemove {
    pub item: String,
    pub path: String,
    pub at: Rational,
}

impl From<KeyframeRemove> for Op {
    fn from(op: KeyframeRemove) -> Op {
        Op::KeyframeRemove(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::KeyframeRemove(k) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&k.item)?;
    let (ti, ci) = item_index(project, &id)?;
    let missing = || not_found("keyframe", &format!("{id} {}", key(&k.path, k.at)));
    let i = ti.get(project).items[ci]
        .common
        .keyframes
        .get(&k.path)
        .and_then(|list| list.iter().position(|x| x.at == k.at))
        .ok_or_else(missing)?;
    check_unlocked(ti.get(project), Some(&id), bindings)?;
    let track = ti.get_mut(project);
    let track_id = track.id.clone();
    let keyframes = &mut track.items[ci].common.keyframes;
    let list = keyframes.get_mut(&k.path).ok_or_else(missing)?;
    let old = list.remove(i);
    if list.is_empty() {
        keyframes.remove(&k.path);
    }
    let (kind, entity) = entity(&id, &k.path);
    diff.record_path_of(kind, &entity, Some(&track_id), &key(&k.path, k.at));
    Ok(vec![
        KeyframeSet {
            item: id,
            path: k.path.clone(),
            at: old.at,
            value: old.value,
            interp: old.interp,
        }
        .into(),
    ])
}

/// `keyframe.set`'s: `Param("<path>@<at>")` of the item or of `<item>/<fx>`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    match op {
        Op::KeyframeRemove(k) => keyframe_footprint(&k.item, &k.path, k.at, op_diff),
        _ => Footprint::on(None, op_diff),
    }
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// `at`, relative to the item (Decision 16).
pub(crate) fn offsets_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::KeyframeRemove(k) => vec![(k.item.clone(), &mut k.at)],
        _ => Vec::new(),
    }
}

/// The wire name.
pub(crate) const NAME: &str = "keyframe.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::KeyframeRemove(k) => vec![&mut k.item],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::KeyframeRemove(k) => vec![&k.item],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
