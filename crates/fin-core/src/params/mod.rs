//! The settings registry (plan 4C Task 11, spec §5.2): every parameter of every item kind,
//! built-in effect, track, timeline and marker, as a [`ParamSpec`] with its type, range, unit,
//! default and fix text. `param.set`, `keyframe.*` and `effect.*` validate through it, and
//! `fin schema param` lists it.
//!
//! - [`find`] resolves a path in a scope (`PARAM_UNKNOWN` with the closest paths, or
//!   `SLOT_UNKNOWN` for a graphic's template);
//! - [`check_shape`] is the value's shape (structural: `PARAM_TYPE`), [`check_range`] its range
//!   (policy: `INVALID_RANGE` with `{path, value, min, max, unit}`), [`validate`] both;
//! - [`get`]/[`set`] read and write an item's value by path (and [`get_track`],
//!   [`get_timeline`], [`get_marker`], [`get_style`] and their setters the other scopes'). `None` is "nothing
//!   stored": a sparse effect param or slot, or audio params an item does not have; setting
//!   `None` resets to the default, so every set has an exact inverse.
//!
//! Item paths: `transform.position|scale|rotation|anchor`, `crop.left|right|top|bottom`,
//! `opacity`, `blend`, `audio.gain|pan|fadeIn|fadeOut`, `mask.feather|choke|invert` (only on an
//! item with a mask), `speed` (clips), `slots.<id>` (graphics, typed by the template's slot
//! kind) and `effects.<fx>.<param>` (an effect's `enabled`, or a param of its kind).
//!
//! Evaluation-pure (Global Constraints, Determinism): comparisons only, no float functions.

use std::sync::LazyLock;

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::CoreError;
use crate::model::{Item, LottieInfo};

mod access;
mod check;
mod effects;
mod item;
mod scopes;
mod style;

pub use access::{get, set};
pub(crate) use check::fill as fill_fix;
pub use check::{check_range, check_shape, validate};
pub use effects::effect_kinds;
pub(crate) use effects::of as effect_specs;
pub use item::slot_spec;
pub use scopes::{get_marker, get_timeline, get_track, set_marker, set_timeline, set_track};
#[cfg(test)]
pub(crate) use style::SAFE_ZONES;
pub use style::{get_style, set_style};

/// One parameter: its path in its scope, type, whether keyframes may animate it, default, unit
/// and fix text (`{min}`, `{max}`, `{unit}` and `{path}` are filled in errors).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    pub path: &'static str,
    pub scope: Scope,
    #[serde(rename = "type")]
    pub ty: ParamType,
    pub animatable: bool,
    pub default: DefaultValue,
    pub unit: Option<&'static str>,
    pub fix: &'static str,
}

/// Where a parameter lives. An effect's paths are `<kind>.<param>`; on an item they are
/// addressed as `effects.<fx>.<param>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Scope {
    Item { kinds: &'static [&'static str] },
    Effect { kind: &'static str },
    Track,
    Timeline,
    Marker,
    Style,
}

/// A parameter's value type. Times and rationals are canonical rational text (`"1/2"`),
/// colours `#rrggbb` or `#rrggbbaa` (lowercase), enums their names; a size is whole pixels as
/// a `vec2`; an asset reference is `{"asset": id}` of one of `kinds`. Ranges are inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ParamType {
    Number {
        min: f64,
        max: f64,
    },
    Bool,
    Enum {
        values: &'static [&'static str],
    },
    Color,
    Vec2 {
        min: [f64; 2],
        max: [f64; 2],
    },
    Time {
        min: Option<&'static str>,
    },
    Rational {
        min: &'static str,
        max: &'static str,
    },
    Size {
        min: u32,
        max: u32,
    },
    Text {
        max_len: u32,
    },
    AssetRef {
        kinds: &'static [&'static str],
    },
}

/// A spec's default, as its value would be written (`null`: none, the value is per entity or
/// the parameter is optional).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum DefaultValue {
    None,
    Bool(bool),
    Number(f64),
    Vec2([f64; 2]),
    Text(&'static str),
}

/// What a path is looked up on: an item (with its graphic template, for slots), an effect kind,
/// or another scope.
#[derive(Debug, Clone, Copy)]
pub enum ScopeOf<'a> {
    Item {
        item: &'a Item,
        template: Option<&'a LottieInfo>,
    },
    Effect {
        kind: &'a str,
    },
    Track,
    Timeline,
    Marker,
    Style,
}

static ALL: LazyLock<Vec<ParamSpec>> = LazyLock::new(|| {
    [
        item::SPECS,
        item::SLOTS,
        effects::SPECS,
        scopes::TRACK,
        scopes::TIMELINE,
        scopes::MARKER,
        style::STYLE,
    ]
    .concat()
});

/// Every spec, in a fixed order: item paths, slot kinds, effects, tracks, timelines, markers,
/// styles.
pub fn specs() -> &'static [ParamSpec] {
    &ALL
}

/// The effect and param an item path `effects.<fx>.<param>` names.
pub fn effect_path(path: &str) -> Option<(&str, &str)> {
    path.strip_prefix("effects.")?.split_once('.')
}

/// The spec `path` names in `scope`: `PARAM_UNKNOWN` (with `details.alternatives`, the closest
/// paths) when there is none, `SLOT_UNKNOWN` for a slot the template lacks.
pub fn find(scope: &ScopeOf<'_>, path: &str) -> Result<&'static ParamSpec, CoreError> {
    match scope {
        ScopeOf::Item { item, template } => item::find(item, *template, path),
        ScopeOf::Effect { kind } => effects::find(kind, path, |p| p.to_string()),
        ScopeOf::Track => scopes::find(scopes::TRACK, "track", path),
        ScopeOf::Timeline => scopes::find(scopes::TIMELINE, "timeline", path),
        ScopeOf::Marker => scopes::find(scopes::MARKER, "marker", path),
        ScopeOf::Style => scopes::find(style::STYLE, "style", path),
    }
}
