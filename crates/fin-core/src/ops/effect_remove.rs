//! `effect.remove`: takes an effect off an item, with the keyframes that animate its params
//! (`effects.<fx>.*`). The inverse is the internal `effect.restore` of the effect at its index,
//! with those keyframes (plan 4C Decision 24 lists the ids nested in it). The change is the
//! effect's (`<item>/<fx>`) `removed`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Item, Keyframe, Project};
use crate::ops::util::{OpDiff, check_unlocked, item_index, not_found, wrong_op};
use crate::ops::{Bindings, EffectRestore, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `effect.remove`: takes `effect` off `item`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectRemove {
    pub item: String,
    pub effect: String,
}

impl From<EffectRemove> for Op {
    fn from(op: EffectRemove) -> Op {
        Op::EffectRemove(op)
    }
}

/// The index of effect `fx` on `item` (`NOT_FOUND` "effect" `<item>/<fx>`).
pub(crate) fn effect_index(item: &Item, fx: &str) -> Result<usize, CoreError> {
    item.common
        .effects
        .iter()
        .position(|e| e.id == fx)
        .ok_or_else(|| not_found("effect", &format!("{}/{fx}", item.id)))
}

/// The keyframe paths of effect `fx` (`effects.<fx>.`).
pub(crate) fn prefix(fx: &str) -> String {
    format!("effects.{fx}.")
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::EffectRemove(r) = op else {
        return Err(wrong_op(op));
    };
    let item = bindings.resolve(&r.item)?;
    let (ti, ci) = item_index(project, &item)?;
    let index = effect_index(&ti.get(project).items[ci], &r.effect)?;
    check_unlocked(ti.get(project), Some(&item), bindings)?;
    let track = ti.get_mut(project);
    let track_id = track.id.clone();
    let common = &mut track.items[ci].common;
    let effect = common.effects.remove(index);
    let own = prefix(&effect.id);
    let paths: Vec<String> = common
        .keyframes
        .keys()
        .filter(|p| p.starts_with(&own))
        .cloned()
        .collect();
    let mut keyframes: BTreeMap<String, Vec<Keyframe>> = BTreeMap::new();
    for p in paths {
        if let Some(k) = common.keyframes.remove(&p) {
            keyframes.insert(p, k);
        }
    }
    diff.record_on(
        EntityKind::Effect,
        &format!("{item}/{}", effect.id),
        Some(&track_id),
        ChangeKind::Removed,
    );
    Ok(vec![
        EffectRestore {
            item,
            effect,
            index: u32::try_from(index).unwrap_or(u32::MAX),
            keyframes,
        }
        .into(),
    ])
}

/// Needs `<item>/<fx>` and the item, and removes the effect.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::EffectRemove(r) = op {
        f.touches
            .push((format!("{}/{}", r.item, r.effect), Touch::Exists));
        f.uses.push(r.item.clone());
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
pub(crate) const NAME: &str = "effect.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::EffectRemove(r) => vec![&mut r.item, &mut r.effect],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::EffectRemove(r) => vec![&r.item, &r.effect],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
