//! `param.set`: sets one registry parameter of an item (plan 4C Task 11), or resets it with no
//! `value`. The inverse is `param.set` of what was stored before (`None` when nothing was: an
//! unset effect param or slot, absent audio params), so it is exact.
//!
//! - An unknown path, a value of the wrong shape and an asset reference to a missing asset or
//!   one of the wrong kind are structural (`PARAM_UNKNOWN`/`SLOT_UNKNOWN`, `PARAM_TYPE`,
//!   `NOT_FOUND`; replay fails on them too); a range (`INVALID_RANGE`), a path animated by
//!   keyframes (`PARAM_ANIMATED`), a locked track and a speed whose source extent leaves the
//!   asset are policy.
//! - The change is the item's with `path` set, or, for `effects.<fx>.<param>`, the effect's
//!   (`kind: effect`, id `<item>/<fx>`, Decision 20), so the footprint touches `Param(path)`
//!   of that entity: two sets of one parameter conflict (`SAME_PARAM`), others merge.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ItemBody, ParamValue, Project};
use crate::ops::util::param_check::{about_item, check_value};
use crate::ops::util::paths::resolve_path;
use crate::ops::util::{
    OpDiff, asset, check_source, check_speed, check_unlocked, frame_of, item_index, wrong_op,
};
use crate::ops::{Bindings, Op};
use crate::params::{self, ScopeOf, effect_path};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// `param.set`: sets `path` of `item` to `value`; no `value` resets it to its default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParamSet {
    pub item: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ParamValue")]
    pub value: Option<ParamValue>,
}

impl From<ParamSet> for Op {
    fn from(op: ParamSet) -> Op {
        Op::ParamSet(op)
    }
}

fn payload(op: &Op) -> Result<&ParamSet, CoreError> {
    match op {
        Op::ParamSet(s) => Ok(s),
        _ => Err(wrong_op(op)),
    }
}

/// The footprint entity of an item path: the item, or `<item>/<fx>` for an effect's.
pub(crate) fn entity(item: &str, path: &str) -> (EntityKind, String) {
    match effect_path(path) {
        Some((fx, _)) => (EntityKind::Effect, format!("{item}/{fx}")),
        None => (EntityKind::Item, item.to_string()),
    }
}

/// `PARAM_ANIMATED` when keyframes animate `path` of `item` (policy).
pub(crate) fn check_static(
    keyframes: usize,
    item: &str,
    path: &str,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    if keyframes == 0 || !bindings.policy() {
        return Ok(());
    }
    Err(CoreError::new(
        "PARAM_ANIMATED",
        format!("{path} of item {item} is animated by keyframes"),
    )
    .with_details(json!({ "item": item, "path": path, "keyframes": keyframes }))
    .with_fix("change its keyframes (keyframe.set, keyframe.remove), or remove them all first"))
}

/// The inverse is one `param.set`.
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
    let s = payload(op)?;
    let id = bindings.resolve(&s.item)?;
    let (ti, ci) = item_index(project, &id)?;
    let path = resolve_path(bindings, &s.path)?;
    let mut value = s.value.clone();
    {
        let track = ti.get(project);
        let it = &track.items[ci];
        let template = match &it.body {
            ItemBody::Graphic { asset: a, .. } => asset(project, a)?.lottie.as_ref(),
            _ => None,
        };
        let scope = ScopeOf::Item { item: it, template };
        let spec = params::find(&scope, &path).map_err(|e| about_item(e, &id))?;
        if let Some(v) = &mut value {
            check_value(project, spec, &path, v, bindings).map_err(|e| about_item(e, &id))?;
        }
        if path == "speed" {
            check_speed_value(project, ti, ci, value.as_ref(), bindings)?;
        }
        check_unlocked(track, Some(&id), bindings)?;
        let animated = it.common.keyframes.get(&path).map_or(0, Vec::len);
        check_static(animated, &id, &path, bindings)?;
    }
    let track = ti.get_mut(project);
    let track_id = track.id.clone();
    let old = params::set(&mut track.items[ci], &path, value)?;
    let (kind, entity) = entity(&id, &path);
    diff.record_path_of(kind, &entity, Some(&track_id), &path);
    Ok(vec![
        ParamSet {
            item: id,
            path,
            value: old,
        }
        .into(),
    ])
}

/// A new speed: positive (structural), and the source extent `duration × speed` inside the
/// asset (policy).
fn check_speed_value(
    project: &Project,
    ti: crate::ops::util::TrackAt,
    ci: usize,
    value: Option<&ParamValue>,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    let speed = match value {
        Some(ParamValue::Text(s)) => s.parse::<Rational>()?,
        _ => Rational::from_int(1),
    };
    check_speed(speed)?;
    let it = &ti.get(project).items[ci];
    if let (
        true,
        ItemBody::Clip {
            asset: a,
            source_in,
            ..
        },
    ) = (bindings.policy(), &it.body)
    {
        let frame = frame_of(project, ti)?;
        check_source(frame, asset(project, a)?, *source_in, it.duration, speed)?;
    }
    Ok(())
}

/// Sets `Param(path)` of the item, or of `<item>/<fx>` (which needs the item too).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ParamSet(s) = op {
        let (kind, id) = entity(&s.item, &s.path);
        f.touches.push((id, Touch::Param(s.path.clone())));
        if kind == EntityKind::Effect {
            f.uses.push(s.item.clone());
        }
        f.uses.extend(
            s.value
                .as_ref()
                .and_then(ParamValue::asset)
                .map(str::to_string),
        );
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

/// The wire name.
pub(crate) const NAME: &str = "param.set";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the item and an asset the value names.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ParamSet(s) => {
            let mut out = vec![&mut s.item];
            out.extend(s.value.as_mut().and_then(ParamValue::asset_mut));
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ParamSet(s) => {
            let mut out = vec![s.item.as_str()];
            out.extend(s.value.as_ref().and_then(ParamValue::asset));
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
