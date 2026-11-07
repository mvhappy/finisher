//! `keyframe.set`: sets the keyframe of `path` of an item at item-relative time `at` (plan 4C
//! Task 14). An existing keyframe at `at` is replaced and the inverse puts it back; otherwise
//! the inverse is `keyframe.remove`.
//!
//! - The path must be a registry path of the item (structural, as `param.set`'s) that keyframes
//!   may animate (`PARAM_NOT_ANIMATABLE`, policy); the value has the spec's shape (structural)
//!   and range (policy); bézier `x1`/`x2` lie in `[0, 1]` (policy) and every number is finite
//!   (structural).
//! - `at` lies in `[0, duration]` when a client sends it (policy). Stored times outside are
//!   kept (a start trim moves keyframes past the item's start, Decision 13), so inverses and
//!   replays apply them as they are; evaluation clamps.
//! - `at` is a content offset (Decision 16): a rebase over a start-edge move maps it by the
//!   move's `by`, so a stale edit meets the keyframe the move carried along.
//! - The change is the item's (or, on `effects.<fx>.<param>`, the effect's `<item>/<fx>`) with
//!   `path: "<path>@<at>"`: edits of one keyframe conflict (`SAME_PARAM`), keyframes at other
//!   times merge.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Interp, ItemBody, Keyframe, ParamValue, Project};
use crate::ops::param_set::entity;
use crate::ops::util::param_check::{about_item, check_value};
use crate::ops::util::{
    OpDiff, asset, check_unlocked, invalid_range, item_index, normalize_interp, wrong_op,
};
use crate::ops::{Bindings, KeyframeRemove, Op};
use crate::params::{self, ScopeOf};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// `keyframe.set`: the keyframe of `path` of `item` at `at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeSet {
    pub item: String,
    pub path: String,
    pub at: Rational,
    pub value: ParamValue,
    pub interp: Interp,
}

impl From<KeyframeSet> for Op {
    fn from(op: KeyframeSet) -> Op {
        Op::KeyframeSet(Box::new(op))
    }
}

/// The parameter key of one keyframe (Decision 20): `<path>@<at>`.
pub(crate) fn key(path: &str, at: Rational) -> String {
    format!("{path}@{at}")
}

fn payload(op: &Op) -> Result<&KeyframeSet, CoreError> {
    match op {
        Op::KeyframeSet(k) => Ok(k),
        _ => Err(wrong_op(op)),
    }
}

/// Bézier `x1`/`x2` in `[0, 1]` (policy).
fn check_interp(path: &str, interp: &Interp) -> Result<(), CoreError> {
    if let Interp::Bezier { x1, x2, .. } = interp
        && [x1, x2].iter().any(|x| !(0.0..=1.0).contains(*x))
    {
        return Err(invalid_range(
            format!("the bézier of {path} has x1 {x1} and x2 {x2}; both must lie in 0 to 1"),
            json!({ "path": path, "x1": x1, "x2": x2, "min": 0.0, "max": 1.0 }),
            "use x1 and x2 from 0 to 1 (CSS cubic-bezier)",
        ));
    }
    Ok(())
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
    let mut value = k.value.clone();
    let mut interp = k.interp;
    normalize_interp(&k.path, &mut interp)?;
    {
        let track = ti.get(project);
        let it = &track.items[ci];
        let template = match &it.body {
            ItemBody::Graphic { asset: a, .. } => asset(project, a)?.lottie.as_ref(),
            _ => None,
        };
        let scope = ScopeOf::Item { item: it, template };
        let spec = params::find(&scope, &k.path).map_err(|e| about_item(e, &id))?;
        check_value(project, spec, &k.path, &mut value, bindings)
            .map_err(|e| about_item(e, &id))?;
        if bindings.policy() {
            if !spec.animatable {
                return Err(CoreError::new(
                    "PARAM_NOT_ANIMATABLE",
                    format!("{} cannot be animated", k.path),
                )
                .with_details(json!({ "item": id, "path": k.path }))
                .with_fix("set it with param.set"));
            }
            check_interp(&k.path, &interp)?;
            check_unlocked(track, Some(&id), bindings)?;
        }
        if !bindings.internal() && (k.at < Rational::zero() || k.at > it.duration) {
            return Err(invalid_range(
                format!(
                    "a keyframe at {} is outside item {id} (0 to {})",
                    k.at, it.duration
                ),
                json!({ "item": id, "path": k.path, "at": k.at, "min": Rational::zero(),
                        "max": it.duration, "unit": "s" }),
                format!("set it at a time from 0 to {} (item-relative)", it.duration),
            ));
        }
    }
    let track = ti.get_mut(project);
    let track_id = track.id.clone();
    let list = track.items[ci]
        .common
        .keyframes
        .entry(k.path.clone())
        .or_default();
    let new = Keyframe {
        at: k.at,
        value,
        interp,
    };
    let i = list.partition_point(|x| x.at < k.at);
    let old = if list.get(i).is_some_and(|x| x.at == k.at) {
        Some(std::mem::replace(&mut list[i], new))
    } else {
        list.insert(i, new);
        None
    };
    let (kind, entity) = entity(&id, &k.path);
    diff.record_path_of(kind, &entity, Some(&track_id), &key(&k.path, k.at));
    Ok(vec![match old {
        Some(old) => KeyframeSet {
            item: id,
            path: k.path.clone(),
            at: old.at,
            value: old.value,
            interp: old.interp,
        }
        .into(),
        None => KeyframeRemove {
            item: id,
            path: k.path.clone(),
            at: k.at,
        }
        .into(),
    }])
}

/// Sets `Param("<path>@<at>")` of the item, or of `<item>/<fx>` (which needs the item too).
pub(crate) fn keyframe_footprint(
    item: &str,
    path: &str,
    at: Rational,
    op_diff: &Diff,
) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    let (kind, id) = entity(item, path);
    f.touches.push((id, Touch::Param(key(path, at))));
    if kind == EntityKind::Effect {
        f.uses.push(item.to_string());
    }
    f
}

/// See [`keyframe_footprint`]; an asset the value names is used.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    match op {
        Op::KeyframeSet(k) => {
            let mut f = keyframe_footprint(&k.item, &k.path, k.at, op_diff);
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
        Op::KeyframeSet(k) => vec![(k.item.clone(), &mut k.at)],
        _ => Vec::new(),
    }
}

/// The wire name.
pub(crate) const NAME: &str = "keyframe.set";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the item and an asset the value names.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::KeyframeSet(k) => {
            let KeyframeSet { item, value, .. } = &mut **k;
            let mut out = vec![item];
            out.extend(value.asset_mut());
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::KeyframeSet(k) => {
            let mut out = vec![k.item.as_str()];
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
