//! `keyframe.restore` (internal: the inverse of `keyframe.remove`, spec §5.12): puts key `key`
//! of `path` back on an item at `at`, under its own literal id. Structural, as
//! `keyframe.set`'s value checks: the path is the item's and the value fits it; the item holds
//! no key `key` on any path (`DUPLICATE_ID`); no other key of `path` is at `at`
//! (`KEYFRAME_EXISTS`, which an undo after another author's key at that time meets). `at` is a
//! content offset (Decision 16). The change and footprint are `keyframe.set`'s; the inverse is
//! `keyframe.remove {key}`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Interp, Keyframe, ParamValue, Project};
use crate::ops::keyframe_set::{checked, keyframe_footprint};
use crate::ops::param_set::entity;
use crate::ops::util::keys::{key_param, keyframe_exists};
use crate::ops::util::paths::resolve_path;
use crate::ops::util::{OpDiff, item_index, wrong_op};
use crate::ops::{Bindings, KeyframeRemove, Op, check_literal};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::Diff;

/// `keyframe.restore` (internal): key `key` of `path` of `item` back at `at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeRestore {
    pub item: String,
    pub path: String,
    pub key: String,
    pub at: Rational,
    pub value: ParamValue,
    pub interp: Interp,
}

impl From<KeyframeRestore> for Op {
    fn from(op: KeyframeRestore) -> Op {
        Op::KeyframeRestore(Box::new(op))
    }
}

fn payload(op: &Op) -> Result<&KeyframeRestore, CoreError> {
    match op {
        Op::KeyframeRestore(k) => Ok(k),
        _ => Err(wrong_op(op)),
    }
}

/// The inverse is one `keyframe.remove`.
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
    let k = payload(op)?;
    let id = bindings.resolve(&k.item)?;
    let (ti, ci) = item_index(project, &id)?;
    let path = resolve_path(bindings, &k.path)?;
    if !bindings.policy() {
        check_literal(&k.key, IdKind::Keyframe)?;
    }
    let (value, interp) = checked(project, bindings, (&id, ti, ci), &path, &k.value, k.interp)?;
    let keyframes = &ti.get(project).items[ci].common.keyframes;
    if keyframes.values().flatten().any(|x| x.id == k.key) {
        return Err(CoreError::new(
            "DUPLICATE_ID",
            format!("item {id} already has key {}", k.key),
        )
        .with_details(json!({ "id": k.key, "item": id })));
    }
    if let Some(x) = keyframes
        .get(&path)
        .and_then(|l| l.iter().find(|x| x.at == k.at))
    {
        return Err(keyframe_exists(&id, &path, &k.key, k.at, &x.id));
    }
    let track = ti.get_mut(project);
    let track_id = track.id.clone();
    let list = track.items[ci]
        .common
        .keyframes
        .entry(path.clone())
        .or_default();
    let first = list.is_empty();
    let i = list.partition_point(|x| x.at < k.at);
    list.insert(
        i,
        Keyframe {
            id: k.key.clone(),
            at: k.at,
            value,
            interp,
        },
    );
    let (kind, entity) = entity(&id, &path);
    diff.record_path_of(kind, &entity, Some(&track_id), &key_param(&path, &k.key));
    if first {
        diff.record_path_of(kind, &entity, Some(&track_id), &path);
    }
    Ok(vec![KeyframeRemove::keyed(id, path, k.key.clone()).into()])
}

/// `keyframe.set`'s: `Param("<path>#<key>")` of the item or of `<item>/<fx>`; an asset the
/// value names is used.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    match op {
        Op::KeyframeRestore(k) => {
            let mut f = keyframe_footprint(&k.item, &k.path, &k.key, op_diff);
            f.uses.extend(k.value.asset().map(str::to_string));
            f
        }
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
        Op::KeyframeRestore(k) => {
            let KeyframeRestore { item, at, .. } = &mut **k;
            vec![(item.clone(), at)]
        }
        _ => Vec::new(),
    }
}

/// The wire name.
pub(crate) const NAME: &str = "keyframe.restore";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;

/// Every id reference the op carries, mutably: the item, the key and an asset the value names.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::KeyframeRestore(k) => {
            let KeyframeRestore {
                item, key, value, ..
            } = &mut **k;
            let mut out = vec![item, key];
            out.extend(value.asset_mut());
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::KeyframeRestore(k) => {
            let mut out = vec![k.item.as_str(), k.key.as_str()];
            out.extend(k.value.asset());
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
