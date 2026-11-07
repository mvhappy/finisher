//! `effect.add`: adds an effect of a registry kind to an item (plan 4C Decision 27), appended or
//! at index `at`, with sparse initial params. Its id (`$fx` → `fx<n>`) comes from the
//! project-wide effect counter; the effect is addressed as `(item, effect)` (`<item>/<fx>`,
//! Decision 10), since a split copies effects with their ids.
//!
//! An `at` past the end is structural (`INVALID_RANGE`); an unknown kind is policy
//! (`PARAM_UNKNOWN`); for a known kind each param must be one of its (structural) with a value
//! of its shape (structural) and range (policy). `enabled` is not a param: added effects are
//! enabled (`param.set effects.<fx>.enabled` turns one off). The change is the effect's
//! `added`, plus the item's `effects` order when `at` is given; the inverse is `effect.remove`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Effect, ParamValue, Project};
use crate::ops::util::param_check::{about_item, check_value};
use crate::ops::util::{
    OpDiff, check_unlocked, invalid_range, item_index, normalize_value, wrong_op,
};
use crate::ops::{Bindings, EffectRemove, Op};
use crate::params::{self, ScopeOf};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// The item path its effect order is recorded and judged under (Decision 20).
pub(crate) const EFFECTS: &str = "effects";

/// `effect.add`: an effect of `kind` on `item` at index `at` (appended when absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectAdd {
    pub item: String,
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub at: Option<u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ParamValue>,
}

impl From<EffectAdd> for Op {
    fn from(op: EffectAdd) -> Op {
        Op::EffectAdd(op)
    }
}

fn payload(op: &Op) -> Result<&EffectAdd, CoreError> {
    match op {
        Op::EffectAdd(a) => Ok(a),
        _ => Err(wrong_op(op)),
    }
}

/// The checked params of `a`, values canonical and asset placeholders resolved.
fn checked_params(
    project: &Project,
    a: &EffectAdd,
    bindings: &Bindings,
) -> Result<BTreeMap<String, ParamValue>, CoreError> {
    let known = params::effect_kinds().contains(&a.kind.as_str());
    if !known && bindings.policy() {
        params::find(&ScopeOf::Effect { kind: &a.kind }, "")?;
    }
    let mut out = BTreeMap::new();
    for (k, v) in &a.params {
        let path = format!("effects.{}.{k}", a.id);
        let mut v = v.clone();
        if k == "enabled" {
            return Err(CoreError::new(
                "PARAM_UNKNOWN",
                format!("{path}: enabled is not an effect param"),
            )
            .with_details(json!({ "path": path, "alternatives": [] }))
            .with_fix("an added effect is enabled; turn it off with param.set"));
        }
        if known {
            let spec = params::find(&ScopeOf::Effect { kind: &a.kind }, k)?;
            check_value(project, spec, &path, &mut v, bindings)?;
        } else {
            normalize_value(&path, &mut v)?;
            if let Some(x) = v.asset_mut() {
                *x = bindings.resolve(x)?;
            }
        }
        out.insert(k.clone(), v);
    }
    Ok(out)
}

/// The inverse is one `effect.remove`.
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
    let a = payload(op)?;
    let item = bindings.resolve(&a.item)?;
    let (ti, ci) = item_index(project, &item)?;
    let len = ti.get(project).items[ci].common.effects.len();
    let index = match a.at {
        Some(at) if at as usize > len => {
            return Err(invalid_range(
                format!("item {item} has {len} effects: there is no index {at}"),
                json!({ "item": item, "at": at, "min": 0, "max": len }),
                format!("use an index from 0 to {len}, or none to append"),
            ));
        }
        Some(at) => at as usize,
        None => len,
    };
    check_unlocked(ti.get(project), Some(&item), bindings)?;
    let params = checked_params(project, a, bindings).map_err(|e| about_item(e, &item))?;
    let fx = bindings.create(project, &a.id, IdKind::Effect, ids)?;
    let track = ti.get_mut(project);
    track.items[ci].common.effects.insert(
        index,
        Effect {
            id: fx.clone(),
            kind: a.kind.clone(),
            enabled: true,
            params,
        },
    );
    let track_id = track.id.clone();
    diff.record_on(
        &track_id,
        EntityKind::Effect,
        &format!("{item}/{fx}"),
        ChangeKind::Added,
    );
    if a.at.is_some() {
        diff.record_path(&track_id, &item, EFFECTS);
    }
    Ok(vec![EffectRemove { item, effect: fx }.into()])
}

/// Creates `<item>/<fx>` and needs the item; an index touches the item's effect order.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::EffectAdd(a) = op {
        f.creates.push(format!("{}/{}", a.item, a.id));
        f.uses.push(a.item.clone());
        if a.at.is_some() {
            f.touches
                .push((a.item.clone(), Touch::Param(EFFECTS.to_string())));
        }
        f.uses.extend(
            a.params
                .values()
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

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "effect.add";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the item, the new id, assets its params name.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::EffectAdd(a) => {
            let mut out = vec![&mut a.item, &mut a.id];
            out.extend(a.params.values_mut().filter_map(ParamValue::asset_mut));
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::EffectAdd(a) => {
            let mut out = vec![a.item.as_str(), a.id.as_str()];
            out.extend(a.params.values().filter_map(ParamValue::asset));
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::EffectAdd(a) => Some(a.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
