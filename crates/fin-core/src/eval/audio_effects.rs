//! An audio item's effects in its plan (plan 6C Decision 15; spec §8.4 says
//! `params: {path: segments}`, but enums are not segments): each enabled effect with every
//! number as piecewise segments over the item's span and every other value resolved.
//!
//! - A registry kind has all its params (`enabled` aside, the effect's own flag): stored, else
//!   keyframed, else the default. Numbers follow their keyframes (`effects.<fx>.<param>`, number
//!   keyframes only) as [`NumberCurve`]s cut at the keyframes and the span's ends; enums and
//!   bools are not animatable and resolve from the stored value or the default. A number param
//!   stored with another shape plays its default.
//! - A visual kind (on an audio clip only by import; `effect.add` refuses it) does not sound
//!   and is left out. A kind the registry does not know (a newer model's) is carried with its
//!   stored and keyframed params typed by their values, for the mixer to skip and report.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::eval::interp::{ValueType, resolve};
use crate::eval::plan::PlanValue;
use crate::eval::resolve::{default_value, param_name, shape_of};
use crate::eval::segments::{NumberCurve, breakpoints};
use crate::model::{Effect, Interp, Item, Keyframe, ParamValue};
use crate::params::{EffectDomain, ParamSpec, effect_domain, effect_specs};
use crate::time::Rational;

/// One effect of an audio item: `numbers` cover the item's span `[from, to)` without gaps, in
/// order; `values` are the params that do not move (enums, bools, text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioEffectPlan {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub numbers: BTreeMap<String, Vec<ParamSegment>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, PlanValue>,
}

/// A number over `[from, to)`, read like a gain segment: the value at progress `x` of the
/// segment is `lerp(v[0], v[1], ease(lerp(u[0], u[1], x)))`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParamSegment {
    pub from: Rational,
    pub to: Rational,
    pub v: [f64; 2],
    pub interp: Interp,
    pub u: [f64; 2],
}

/// The enabled audio (and unknown) effects of `item`, in its order, over `[from, to)` (the
/// part of the item's extent being planned).
pub(crate) fn effects_of(
    item: &Item,
    from: Rational,
    to: Rational,
) -> Result<Vec<AudioEffectPlan>, CoreError> {
    item.common
        .effects
        .iter()
        .filter(|e| e.enabled && effect_domain(&e.kind) != Some(EffectDomain::Visual))
        .map(|e| effect_plan(item, e, from, to))
        .collect()
}

fn effect_plan(
    item: &Item,
    e: &Effect,
    from: Rational,
    to: Rational,
) -> Result<AudioEffectPlan, CoreError> {
    let prefix = format!("effects.{}.", e.id);
    let specs: Vec<&ParamSpec> = effect_specs(&e.kind).collect();
    // Stored, keyframed (one level deep, review #51) and registry names.
    let keyed = item.common.keyframes.keys().filter_map(|k| {
        k.strip_prefix(&prefix)
            .filter(|rest| !rest.is_empty() && !rest.contains('.'))
    });
    let mut names: BTreeSet<&str> = e.params.keys().map(String::as_str).chain(keyed).collect();
    names.extend(specs.iter().map(|s| param_name(s)));
    names.remove("enabled");
    let mut plan = AudioEffectPlan {
        id: e.id.clone(),
        kind: e.kind.clone(),
        numbers: BTreeMap::new(),
        values: BTreeMap::new(),
    };
    for name in names {
        let path = format!("{prefix}{name}");
        let kfs: &[Keyframe] = item.common.keyframes.get(&path).map_or(&[], Vec::as_slice);
        let spec = specs.iter().find(|s| param_name(s) == name);
        let default = spec.and_then(|s| default_value(s.default));
        let Some(base) = e
            .params
            .get(name)
            .or_else(|| kfs.first().map(|k| &k.value))
            .or(default.as_ref())
        else {
            continue;
        };
        let ty = spec.map_or_else(|| shape_of(base), |s| ValueType::from(&s.ty));
        if ty != ValueType::Number {
            plan.values.insert(name.to_string(), resolve(base, ty));
            continue;
        }
        let n = match (base, &default) {
            (ParamValue::Number(n), _) | (_, Some(ParamValue::Number(n))) => *n,
            _ => continue,
        };
        let segments = segments(&NumberCurve::new(kfs, n, item.start)?, from, to);
        plan.numbers.insert(name.to_string(), segments);
    }
    Ok(plan)
}

/// `curve` over `[from, to)`, cut at its keyframes.
fn segments(curve: &NumberCurve, from: Rational, to: Rational) -> Vec<ParamSegment> {
    breakpoints(from, to, curve.cuts().iter().copied())
        .windows(2)
        .map(|w| {
            let piece = curve.piece(w[0], w[1]);
            ParamSegment {
                from: w[0],
                to: w[1],
                v: piece.v,
                interp: piece.interp,
                u: piece.u,
            }
        })
        .collect()
}
