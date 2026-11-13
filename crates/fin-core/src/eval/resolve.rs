//! An item's parameters at a local time: the stored value, or its keyframes (registry paths,
//! plan 4C Task 11) interpolated. A keyframed value of the wrong shape falls back to the
//! stored one, so a plan always has the field's type. A registry effect's params are typed by
//! the registry and its unset ones come at their defaults.

use std::collections::{BTreeMap, BTreeSet};

use crate::eval::interp::{ValueType, interpolate};
use crate::eval::plan::{EffectPlan, MaskPlan, PlanCrop, PlanTransform, PlanValue};
use crate::model::{Effect, Item, Keyframe, LottieInfo, ParamValue};
use crate::params::{DefaultValue, ParamSpec, effect_specs};
use crate::time::Rational;

pub(crate) struct Resolver<'a> {
    pub item: &'a Item,
    pub local: Rational,
}

/// The type of a stored value when the registry is not consulted: numbers and `vec2`s
/// interpolate, everything else steps.
fn shape_of(v: &ParamValue) -> ValueType {
    match v {
        ParamValue::Number(_) => ValueType::Number,
        ParamValue::Vec2(_) => ValueType::Vec2,
        ParamValue::Bool(_) | ParamValue::Text(_) | ParamValue::Asset { .. } => ValueType::Step,
    }
}

/// A registry default as a stored value (`None`: the param has none, such as a LUT's asset).
fn default_value(d: DefaultValue) -> Option<ParamValue> {
    match d {
        DefaultValue::None => None,
        DefaultValue::Bool(b) => Some(ParamValue::Bool(b)),
        DefaultValue::Number(n) => Some(ParamValue::Number(n)),
        DefaultValue::Vec2(v) => Some(ParamValue::Vec2(v)),
        DefaultValue::Text(s) => Some(ParamValue::Text(s.to_string())),
    }
}

/// A registry effect param's name (`exposure` of `colorAdjust.exposure`).
fn param_name(spec: &ParamSpec) -> &'static str {
    spec.path.split_once('.').map_or(spec.path, |(_, p)| p)
}

impl Resolver<'_> {
    fn keyframes(&self, path: &str) -> &[Keyframe] {
        self.item
            .common
            .keyframes
            .get(path)
            .map_or(&[], Vec::as_slice)
    }

    pub fn value(&self, path: &str, base: &ParamValue, ty: ValueType) -> PlanValue {
        interpolate(self.keyframes(path), self.local, base, ty)
    }

    pub fn number(&self, path: &str, base: f64) -> f64 {
        match self.value(path, &ParamValue::Number(base), ValueType::Number) {
            PlanValue::Number(n) => n,
            _ => base,
        }
    }

    pub fn vec2(&self, path: &str, base: [f64; 2]) -> [f64; 2] {
        match self.value(path, &ParamValue::Vec2(base), ValueType::Vec2) {
            PlanValue::Vec2(v) => v,
            _ => base,
        }
    }

    pub fn flag(&self, path: &str, base: bool) -> bool {
        match self.value(path, &ParamValue::Bool(base), ValueType::Step) {
            PlanValue::Bool(b) => b,
            _ => base,
        }
    }

    pub fn transform(&self) -> PlanTransform {
        let t = &self.item.common.transform;
        PlanTransform {
            position: self.vec2("transform.position", t.position),
            scale: self.vec2("transform.scale", t.scale),
            rotation: self.number("transform.rotation", t.rotation),
            anchor: self.vec2("transform.anchor", t.anchor),
        }
    }

    /// Each side clamped to `[0, 1]`.
    pub fn crop(&self) -> PlanCrop {
        let c = &self.item.common.crop;
        let side = |path, v| self.number(path, v).clamp(0.0, 1.0);
        PlanCrop {
            left: side("crop.left", c.left),
            right: side("crop.right", c.right),
            top: side("crop.top", c.top),
            bottom: side("crop.bottom", c.bottom),
        }
    }

    /// Clamped to `[0, 1]`.
    pub fn opacity(&self) -> f64 {
        self.number("opacity", self.item.common.opacity)
            .clamp(0.0, 1.0)
    }

    pub fn mask(&self) -> Option<MaskPlan> {
        let m = self.item.common.mask.as_ref()?;
        Some(MaskPlan {
            matte: m.matte.clone(),
            feather: self.number("mask.feather", m.feather).max(0.0),
            choke: self.number("mask.choke", m.choke).max(0.0),
            invert: self.flag("mask.invert", m.invert),
        })
    }

    /// The enabled effects, in order, with their stored and keyframed (`effects.<fx>.<param>`)
    /// parameters.
    pub fn effects(&self) -> Vec<EffectPlan> {
        let item = self.item;
        item.common
            .effects
            .iter()
            .filter(|e| e.enabled)
            .map(|e| EffectPlan {
                id: e.id.clone(),
                kind: e.kind.clone(),
                params: self.effect_params(e),
            })
            .collect()
    }

    /// A registry kind's params are all there: stored, else keyframed, else the default
    /// (`enabled` is the effect's own flag, not a param); an unknown kind's are the stored and
    /// keyframed ones, typed by their values.
    fn effect_params(&self, e: &Effect) -> BTreeMap<String, PlanValue> {
        let prefix = format!("effects.{}.", e.id);
        let specs: Vec<&ParamSpec> = effect_specs(&e.kind)
            .filter(|s| param_name(s) != "enabled")
            .collect();
        let mut names = self.names(&e.params, &prefix, false);
        names.extend(specs.iter().map(|s| param_name(s).to_string()));
        names.remove("enabled");
        names
            .into_iter()
            .filter_map(|name| {
                let path = format!("{prefix}{name}");
                let spec = specs.iter().find(|s| param_name(s) == name);
                let base = self
                    .base(&e.params, &name, &path)
                    .or_else(|| spec.and_then(|s| default_value(s.default)))?;
                let ty = spec.map_or_else(|| shape_of(&base), |s| ValueType::from(&s.ty));
                let v = self.value(&path, &base, ty);
                Some((name, v))
            })
            .collect()
    }

    /// A graphic's slots, typed by the template (`color` slots interpolate as colours).
    pub fn slots(
        &self,
        slots: &BTreeMap<String, ParamValue>,
        template: Option<&LottieInfo>,
    ) -> BTreeMap<String, PlanValue> {
        let kind = |id: &str| {
            template
                .and_then(|t| t.slots.iter().find(|s| s.id == id))
                .map(|s| s.kind.as_str())
        };
        self.names(slots, "slots.", true)
            .into_iter()
            .filter_map(|id| {
                let path = format!("slots.{id}");
                let base = self.base(slots, &id, &path)?;
                let ty = match kind(&id) {
                    Some("color" | "colour") => ValueType::Color,
                    _ => shape_of(&base),
                };
                let v = self.value(&path, &base, ty);
                Some((id, v))
            })
            .collect()
    }

    /// The stored names plus those only keyframed under `prefix`: one level deep, or, with
    /// `whole`, the entire rest of the path (a slot id is everything after `slots.`, dots
    /// included, as at import and mutation; review #51).
    fn names(
        &self,
        stored: &BTreeMap<String, ParamValue>,
        prefix: &str,
        whole: bool,
    ) -> BTreeSet<String> {
        let keyed = self.item.common.keyframes.keys().filter_map(|k| {
            k.strip_prefix(prefix)
                .filter(|rest| !rest.is_empty() && (whole || !rest.contains('.')))
        });
        stored
            .keys()
            .map(String::as_str)
            .chain(keyed)
            .map(str::to_string)
            .collect()
    }

    /// The stored value, else the first keyframe's (a value only keyframed).
    fn base(
        &self,
        stored: &BTreeMap<String, ParamValue>,
        name: &str,
        path: &str,
    ) -> Option<ParamValue> {
        stored
            .get(name)
            .or_else(|| self.keyframes(path).first().map(|k| &k.value))
            .cloned()
    }
}
