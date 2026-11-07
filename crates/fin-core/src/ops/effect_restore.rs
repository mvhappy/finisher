//! `effect.restore` (internal: the inverse of `effect.remove`): puts `effect` back on `item` at
//! `index`, with the keyframes that animated its params. The effect's id is literal (it keeps
//! the id it had); its keyframe times are item-relative, so a rebase maps them through content
//! offsets like a `keyframe.set`'s `at` (Decision 16). Structural: the index fits, the item has
//! no effect with that id, each keyframe path is the effect's (`effects.<fx>.`) and not already
//! animated, each list is non-empty and strictly in time order, and the values are finite. An
//! index past the end (an undo re-run after other effects were removed) is the end, in every
//! mode, so the replay of what the undo committed puts it where the undo did. The change is
//! the effect's `added`; the footprint also touches the item's effect order.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Effect, Keyframe, ParamValue, Project};
use crate::ops::effect_remove::prefix;
use crate::ops::util::{
    OpDiff, check_unlocked, item_index, normalize_interp, normalize_value, wrong_op,
};
use crate::ops::{Bindings, EffectRemove, Op, check_literal};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `effect.restore` (internal): `effect` back on `item` at `index`, with its `keyframes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectRestore {
    pub item: String,
    pub effect: Effect,
    pub index: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keyframes: BTreeMap<String, Vec<Keyframe>>,
}

impl From<EffectRestore> for Op {
    fn from(op: EffectRestore) -> Op {
        Op::EffectRestore(Box::new(op))
    }
}

fn payload(op: &Op) -> Result<&EffectRestore, CoreError> {
    match op {
        Op::EffectRestore(r) => Ok(r),
        _ => Err(wrong_op(op)),
    }
}

/// Canonical values (structural, Decision 25).
fn canonical(r: &EffectRestore) -> Result<(Effect, BTreeMap<String, Vec<Keyframe>>), CoreError> {
    let mut effect = r.effect.clone();
    for (k, v) in &mut effect.params {
        normalize_value(&format!("effects.{}.{k}", effect.id), v)?;
    }
    let mut keyframes = r.keyframes.clone();
    for (path, keys) in &mut keyframes {
        if keys.is_empty() || keys.windows(2).any(|w| w[0].at >= w[1].at) {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("the keyframes of {path} are not a non-empty list in time order"),
            )
            .with_details(json!({ "path": path })));
        }
        for k in keys {
            normalize_value(path, &mut k.value)?;
            normalize_interp(path, &mut k.interp)?;
        }
    }
    Ok((effect, keyframes))
}

/// The inverse is one `effect.remove`.
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
    let r = payload(op)?;
    let item = bindings.resolve(&r.item)?;
    let (ti, ci) = item_index(project, &item)?;
    let (effect, keyframes) = canonical(r)?;
    if !bindings.policy() {
        check_literal(&effect.id, IdKind::Effect)?;
    }
    {
        let it = &ti.get(project).items[ci];
        if it.common.effects.iter().any(|e| e.id == effect.id) {
            return Err(CoreError::new(
                "DUPLICATE_ID",
                format!("item {item} already has effect {}", effect.id),
            )
            .with_details(json!({ "id": effect.id, "item": item })));
        }
        let own = prefix(&effect.id);
        if let Some(p) = keyframes
            .keys()
            .find(|p| !p.starts_with(&own) || it.common.keyframes.contains_key(*p))
        {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("keyframes at {p} are not effect {}'s to restore", effect.id),
            )
            .with_details(json!({ "item": item, "effect": effect.id, "path": p })));
        }
        check_unlocked(ti.get(project), Some(&item), bindings)?;
    }
    let track = ti.get_mut(project);
    let track_id = track.id.clone();
    let common = &mut track.items[ci].common;
    let fx = effect.id.clone();
    // An undo re-run after other effects were removed may name an index past the end: it is
    // the end (and so in the replay of what it committed).
    let index = (r.index as usize).min(common.effects.len());
    common.effects.insert(index, effect);
    common.keyframes.extend(keyframes);
    diff.record_on(
        &track_id,
        EntityKind::Effect,
        &format!("{item}/{fx}"),
        ChangeKind::Added,
    );
    Ok(vec![EffectRemove { item, effect: fx }.into()])
}

/// Creates `<item>/<fx>`, needs the item and touches its effect order.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::EffectRestore(r) = op {
        let fx = format!("{}/{}", r.item, r.effect.id);
        // Its params and keyframes as they were: a later edit of the effect is a conflict.
        f.puts_back(&fx);
        f.creates.push(fx);
        // Its index is carried to the head by its neighbours (`rebase::places`).
        f.uses.push(crate::ops::util::lists::effects(&r.item));
        f.uses.push(r.item.clone());
        f.uses.extend(
            r.effect
                .params
                .values()
                .chain(r.keyframes.values().flatten().map(|k| &k.value))
                .filter_map(ParamValue::asset)
                .map(str::to_string),
        );
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// Every restored keyframe's `at`, relative to the item (Decision 16).
pub(crate) fn offsets_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::EffectRestore(r) => {
            let item = r.item.clone();
            r.keyframes
                .values_mut()
                .flatten()
                .map(|k| (item.clone(), &mut k.at))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// The wire name.
pub(crate) const NAME: &str = "effect.restore";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;

/// Every id reference the op carries, mutably: the item, the effect's id, the assets its
/// params name and the assets its keyframe values name (the effect ids in the keyframe paths
/// are reached through [`Op::try_for_each_ref`]).
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::EffectRestore(r) => {
            let EffectRestore {
                item,
                effect,
                keyframes,
                ..
            } = &mut **r;
            let mut out = vec![item];
            out.extend(effect.refs_mut());
            out.extend(
                keyframes
                    .values_mut()
                    .flatten()
                    .filter_map(|k| k.value.asset_mut()),
            );
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::EffectRestore(r) => {
            let mut out = vec![r.item.as_str()];
            out.extend(r.effect.ids());
            out.extend(
                r.keyframes
                    .values()
                    .flatten()
                    .filter_map(|k| k.value.asset()),
            );
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
