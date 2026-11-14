//! `keyframe.remove`: removes the key `key` of `path` of an item (stable ids, spec §5.12).
//! Exactly one of `key` and `at` (`INVALID_OP` otherwise); the time form (`at`) is a client
//! form only, rewritten by the applier to the key at exactly that time
//! ([`crate::ops::util::keys`]). `NOT_FOUND` "keyframe" when the item has no such key on
//! `path` (structural). Removing a path's last keyframe drops the path's list, so the path is
//! static again (and the change names `path` too, as the first `keyframe.set` of it does). No
//! range check: a keyframe a trim carried outside the item can still be removed. The change
//! and footprint are `keyframe.set`'s (`"<path>#<key>"`). The inverse is `keyframe.restore`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::keyframe_set::keyframe_footprint;
use crate::ops::param_set::entity;
use crate::ops::util::keys::{key_param, time_form};
use crate::ops::util::paths::resolve_path;
use crate::ops::util::{OpDiff, check_unlocked, item_index, not_found, wrong_op};
use crate::ops::{Bindings, KeyframeRestore, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::Diff;

/// `keyframe.remove`: the key `key` (or, in the client time form, the key at `at`) of `path`
/// of `item`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeRemove {
    pub item: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Rational>,
}

impl KeyframeRemove {
    /// The time form: the key at `at`.
    pub fn timed(item: impl Into<String>, path: impl Into<String>, at: Rational) -> Self {
        KeyframeRemove {
            item: item.into(),
            path: path.into(),
            key: None,
            at: Some(at),
        }
    }

    /// The key form: the key `key`.
    pub fn keyed(item: impl Into<String>, path: impl Into<String>, key: impl Into<String>) -> Self {
        KeyframeRemove {
            item: item.into(),
            path: path.into(),
            key: Some(key.into()),
            at: None,
        }
    }
}

impl From<KeyframeRemove> for Op {
    fn from(op: KeyframeRemove) -> Op {
        Op::KeyframeRemove(op)
    }
}

/// The inverse is one `keyframe.restore`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
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
    let (Some(key), None) = (&k.key, k.at) else {
        return Err(time_form(op));
    };
    let key = bindings.resolve(key)?;
    let id = bindings.resolve(&k.item)?;
    let (ti, ci) = item_index(project, &id)?;
    let path = resolve_path(bindings, &k.path)?;
    let missing = || not_found("keyframe", &format!("{id} {path} {key}"));
    let i = ti.get(project).items[ci]
        .common
        .keyframes
        .get(&path)
        .and_then(|list| list.iter().position(|x| x.id == key))
        .ok_or_else(missing)?;
    check_unlocked(ti.get(project), Some(&id), bindings)?;
    let track = ti.get_mut(project);
    let track_id = track.id.clone();
    let keyframes = &mut track.items[ci].common.keyframes;
    let list = keyframes.get_mut(&path).ok_or_else(missing)?;
    let old = list.remove(i);
    let last = list.is_empty();
    if last {
        keyframes.remove(&path);
    }
    let (kind, entity) = entity(&id, &path);
    diff.record_path_of(kind, &entity, Some(&track_id), &key_param(&path, &key));
    if last {
        // The path is static again: as with its first keyframe, `path` itself changes.
        diff.record_path_of(kind, &entity, Some(&track_id), &path);
    }
    Ok(vec![
        KeyframeRestore {
            item: id,
            path,
            key: old.id,
            at: old.at,
            value: old.value,
            interp: old.interp,
        }
        .into(),
    ])
}

/// `keyframe.set`'s: `Param("<path>#<key>")` of the item or of `<item>/<fx>`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    match op {
        Op::KeyframeRemove(KeyframeRemove {
            item,
            path,
            key: Some(key),
            ..
        }) => keyframe_footprint(item, path, key, op_diff),
        _ => Footprint::on(None, op_diff),
    }
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// A time form's `at`, relative to the item (Decision 16).
pub(crate) fn offsets_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::KeyframeRemove(k) => {
            k.at.as_mut()
                .map(|at| (k.item.clone(), at))
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

/// The wire name.
pub(crate) const NAME: &str = "keyframe.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the item and the key.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::KeyframeRemove(k) => {
            let mut out = vec![&mut k.item];
            out.extend(k.key.as_mut());
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::KeyframeRemove(k) => {
            let mut out = vec![k.item.as_str()];
            out.extend(k.key.as_deref());
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
