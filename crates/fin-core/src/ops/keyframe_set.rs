//! `keyframe.set`: sets a keyframe of `path` of an item (plan 4C Task 14; stable ids, spec
//! §5.12). Keys are named by id (`key`): a literal id edits that key, moving it when `at` is
//! another time (`NOT_FOUND` "keyframe" when the item has no such key on `path`); a placeholder
//! (`$k`) inserts a key at `at` under a new `kf<n>` id. A target time held by another key is
//! `KEYFRAME_EXISTS`. The *time form* (`at` without `key`) is a client form only: the applier
//! rewrites it to the key form of the key at `at`, or to an insert under `$kf<opIndex>`
//! ([`crate::ops::util::keys`]), so every stored op names its key.
//!
//! - The path must be a registry path of the item (structural, as `param.set`'s) that keyframes
//!   may animate (`PARAM_NOT_ANIMATABLE`, policy); the value has the spec's shape (structural)
//!   and range (policy); bézier `x1`/`x2` lie in `[0, 1]` and `y1`/`y2` in `[-100, 100]`
//!   (policy) and every number is finite (structural).
//! - `at` lies in `[0, duration]` when a client sends it (policy). Stored times outside are
//!   kept (a start trim moves keyframes past the item's start, Decision 13), so inverses and
//!   replays apply them as they are; evaluation clamps.
//! - `at` is a content offset (Decision 16): a rebase over a start-edge move maps it by the
//!   move's `by`.
//! - The change is the item's (or, on `effects.<fx>.<param>`, the effect's `<item>/<fx>`) with
//!   `path: "<path>#<key>"`: edits of one key conflict (`SAME_PARAM`) whatever trims did to its
//!   time; other keys merge. The first keyframe of a path also changes `path` itself (it is no
//!   longer static), so it conflicts with a concurrent `param.set` of it.
//! - The inverse of an insert is `keyframe.remove {key}`; of an edit or move, `keyframe.set`
//!   with the old time, value and interpolation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Interp, ItemBody, Keyframe, ParamValue, Project};
use crate::ops::param_set::entity;
use crate::ops::util::keys::{key_param, keyframe_exists, time_form};
use crate::ops::util::param_check::{about_item, check_value};
use crate::ops::util::paths::resolve_path;
use crate::ops::util::{
    OpDiff, TrackAt, asset, check_unlocked, invalid_range, item_index, normalize_interp, not_found,
    wrong_op,
};
use crate::ops::{Bindings, KeyframeRemove, Op};
use crate::params::{self, ScopeOf};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// `keyframe.set`: the key `key` (or, in the client time form, the key at `at`) of `path` of
/// `item`, at `at` (default: where it is) with `value` and `interp`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeSet {
    pub item: String,
    pub path: String,
    /// A key id (`kf<n>`), or a `$name` placeholder for a new key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Rational>,
    pub value: ParamValue,
    pub interp: Interp,
}

impl KeyframeSet {
    /// The time form: the key at `at`, or a new one there.
    pub fn timed(
        item: impl Into<String>,
        path: impl Into<String>,
        at: Rational,
        value: ParamValue,
        interp: Interp,
    ) -> KeyframeSet {
        KeyframeSet {
            item: item.into(),
            path: path.into(),
            key: None,
            at: Some(at),
            value,
            interp,
        }
    }
}

impl From<KeyframeSet> for Op {
    fn from(op: KeyframeSet) -> Op {
        Op::KeyframeSet(Box::new(op))
    }
}

fn payload(op: &Op) -> Result<&KeyframeSet, CoreError> {
    match op {
        Op::KeyframeSet(k) => Ok(k),
        _ => Err(wrong_op(op)),
    }
}

/// How far a bézier's `y1`/`y2` may overshoot (policy): an eased value stays within this many
/// times its keyframes' span of them, so it stays finite and renderable.
pub(crate) const BEZIER_Y_LIMIT: f64 = 100.0;

/// Bézier `x1`/`x2` in `[0, 1]` and `y1`/`y2` in `[-BEZIER_Y_LIMIT, BEZIER_Y_LIMIT]` (policy).
fn check_interp(path: &str, interp: &Interp) -> Result<(), CoreError> {
    let Interp::Bezier { x1, y1, x2, y2 } = interp else {
        return Ok(());
    };
    if [x1, x2].iter().any(|x| !(0.0..=1.0).contains(*x)) {
        return Err(invalid_range(
            format!("the bézier of {path} has x1 {x1} and x2 {x2}; both must lie in 0 to 1"),
            json!({ "path": path, "x1": x1, "x2": x2, "min": 0.0, "max": 1.0 }),
            "use x1 and x2 from 0 to 1 (CSS cubic-bezier)",
        ));
    }
    let lim = BEZIER_Y_LIMIT;
    if [y1, y2].iter().any(|y| !(-lim..=lim).contains(*y)) {
        return Err(invalid_range(
            format!(
                "the bézier of {path} has y1 {y1} and y2 {y2}; both must lie in -{lim} to {lim}"
            ),
            json!({ "path": path, "y1": y1, "y2": y2, "min": -lim, "max": lim }),
            format!("use y1 and y2 from -{lim} to {lim}"),
        ));
    }
    Ok(())
}

/// The checked value and interpolation of a key of `path` on item `id` (at `ti`, `ci`): the
/// path is the item's, the value fits it, and with policy the path is animatable, the bézier
/// within limits and the track unlocked (shared with `keyframe.restore`).
pub(crate) fn checked(
    project: &Project,
    bindings: &Bindings,
    (id, ti, ci): (&str, TrackAt, usize),
    path: &str,
    value: &ParamValue,
    interp: Interp,
) -> Result<(ParamValue, Interp), CoreError> {
    let mut value = value.clone();
    let mut interp = interp;
    normalize_interp(path, &mut interp)?;
    let track = ti.get(project);
    let it = &track.items[ci];
    let template = match &it.body {
        ItemBody::Graphic { asset: a, .. } => asset(project, a)?.lottie.as_ref(),
        _ => None,
    };
    let scope = ScopeOf::Item { item: it, template };
    let spec = params::find(&scope, path).map_err(|e| about_item(e, id))?;
    check_value(project, spec, path, &mut value, bindings).map_err(|e| about_item(e, id))?;
    if bindings.policy() {
        if !spec.animatable {
            return Err(CoreError::new(
                "PARAM_NOT_ANIMATABLE",
                format!("{path} cannot be animated"),
            )
            .with_details(json!({ "item": id, "path": path }))
            .with_fix("set it with param.set"));
        }
        check_interp(path, &interp)?;
        check_unlocked(track, Some(id), bindings)?;
    }
    Ok((value, interp))
}

/// The inverse is one `keyframe.set` (the old key back) or `keyframe.remove`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let k = payload(op)?;
    let Some(key) = &k.key else {
        return Err(time_form(op));
    };
    let id = bindings.resolve(&k.item)?;
    let (ti, ci) = item_index(project, &id)?;
    let path = resolve_path(bindings, &k.path)?;
    let (value, interp) = checked(project, bindings, (&id, ti, ci), &path, &k.value, k.interp)?;
    let duration = ti.get(project).items[ci].duration;
    if let Some(at) = k.at
        && !bindings.internal()
        && (at < Rational::zero() || at > duration)
    {
        return Err(invalid_range(
            format!("a keyframe at {at} is outside item {id} (0 to {duration})"),
            json!({ "item": id, "path": path, "at": at, "min": Rational::zero(),
                    "max": duration, "unit": "s" }),
            format!("set it at a time from 0 to {duration} (item-relative)"),
        ));
    }
    let list = ti.get(project).items[ci].common.keyframes.get(&path);
    let held = |at: Rational, other: &str| {
        list.and_then(|l| l.iter().find(|x| x.at == at && x.id != other))
    };
    // A placeholder an earlier op of the tx bound names that key; an unbound one is new.
    let (kid, old) = if key.starts_with('$') && !bindings.map.contains_key(key) {
        let Some(at) = k.at else {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("a new keyframe ({key}) needs a time"),
            )
            .with_details(json!({ "item": id, "path": path, "key": key }))
            .with_fix("give at"));
        };
        if let Some(x) = held(at, "") {
            return Err(keyframe_exists(&id, &path, key, at, &x.id));
        }
        (bindings.create(project, key, IdKind::Keyframe, ids)?, None)
    } else {
        let key = &bindings.resolve(key)?;
        let old = list
            .and_then(|l| l.iter().find(|x| x.id == *key))
            .ok_or_else(|| not_found("keyframe", &format!("{id} {path} {key}")))?;
        let at = k.at.unwrap_or(old.at);
        if let Some(x) = held(at, key) {
            return Err(keyframe_exists(&id, &path, key, at, &x.id));
        }
        (key.clone(), Some(old.clone()))
    };
    let at = k.at.or(old.as_ref().map(|o| o.at)).unwrap_or_default();
    let track = ti.get_mut(project);
    let track_id = track.id.clone();
    let list = track.items[ci]
        .common
        .keyframes
        .entry(path.clone())
        .or_default();
    let first = list.is_empty();
    list.retain(|x| x.id != kid);
    let i = list.partition_point(|x| x.at < at);
    list.insert(
        i,
        Keyframe {
            id: kid.clone(),
            at,
            value,
            interp,
        },
    );
    let (kind, entity) = entity(&id, &path);
    diff.record_path_of(kind, &entity, Some(&track_id), &key_param(&path, &kid));
    if first {
        // The path stops being static: a concurrent `param.set` of it is `SAME_PARAM`.
        diff.record_path_of(kind, &entity, Some(&track_id), &path);
    }
    Ok(vec![match old {
        Some(old) => KeyframeSet {
            item: id,
            path,
            key: Some(kid),
            at: Some(old.at),
            value: old.value,
            interp: old.interp,
        }
        .into(),
        None => KeyframeRemove {
            item: id,
            path,
            key: Some(kid),
            at: None,
        }
        .into(),
    }])
}

/// Sets `Param("<path>#<key>")` of the item, or of `<item>/<fx>` (which needs the item too).
pub(crate) fn keyframe_footprint(item: &str, path: &str, key: &str, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    let (kind, id) = entity(item, path);
    f.touches.push((id, Touch::Param(key_param(path, key))));
    if kind == EntityKind::Effect {
        f.uses.push(item.to_string());
    }
    f
}

/// See [`keyframe_footprint`]; an asset the value names is used. A time-form op (never
/// stored) names no key.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    match op {
        Op::KeyframeSet(k) => {
            let mut f = match &k.key {
                Some(key) => keyframe_footprint(&k.item, &k.path, key, op_diff),
                None => Footprint::on(None, op_diff),
            };
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
        Op::KeyframeSet(k) => {
            let KeyframeSet { item, at, .. } = &mut **k;
            at.as_mut()
                .map(|at| (item.clone(), at))
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

/// The wire name.
pub(crate) const NAME: &str = "keyframe.set";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the item, the key and an asset the value names.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::KeyframeSet(k) => {
            let KeyframeSet {
                item, key, value, ..
            } = &mut **k;
            let mut out = vec![item];
            out.extend(key.as_mut());
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
            out.extend(k.key.as_deref());
            out.extend(k.value.asset());
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a new key's id, if any: only the first op of a tx that
/// names it binds it, later ones name that key ([`crate::BoundIds`] skips them).
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::KeyframeSet(k) => k.key.as_deref().filter(|k| k.starts_with('$')),
        _ => None,
    }
}
