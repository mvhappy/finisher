//! The built-in effects (plan 4C Decision 27): `colorAdjust`, `lut` and `blur`. Each also has
//! `enabled`. Params are sparse: an absent one is its default. Plugins may declare more kinds
//! later (sub-project 8); everything here is looked up by kind name.

use serde_json::json;

use crate::error::CoreError;
use crate::params::check::unknown;
use crate::params::{DefaultValue, ParamSpec, ParamType, Scope};

const KINDS: &[&str] = &["colorAdjust", "lut", "blur"];

/// The effect kinds the registry knows, in a fixed order.
pub fn effect_kinds() -> &'static [&'static str] {
    KINDS
}

const fn spec(
    path: &'static str,
    kind: &'static str,
    ty: ParamType,
    animatable: bool,
    default: DefaultValue,
    unit: Option<&'static str>,
    fix: &'static str,
) -> ParamSpec {
    ParamSpec {
        path,
        scope: Scope::Effect { kind },
        ty,
        animatable,
        default,
        unit,
        fix,
    }
}

const fn number(
    path: &'static str,
    kind: &'static str,
    min: f64,
    max: f64,
    default: f64,
    unit: Option<&'static str>,
    fix: &'static str,
) -> ParamSpec {
    spec(
        path,
        kind,
        ParamType::Number { min, max },
        true,
        DefaultValue::Number(default),
        unit,
        fix,
    )
}

const fn enabled(path: &'static str, kind: &'static str) -> ParamSpec {
    spec(
        path,
        kind,
        ParamType::Bool,
        false,
        DefaultValue::Bool(true),
        None,
        "use true or false",
    )
}

pub(super) const SPECS: &[ParamSpec] = &[
    enabled("colorAdjust.enabled", "colorAdjust"),
    number(
        "colorAdjust.exposure",
        "colorAdjust",
        -4.0,
        4.0,
        0.0,
        Some("EV"),
        "use an exposure from {min} to {max} {unit}",
    ),
    number(
        "colorAdjust.contrast",
        "colorAdjust",
        -1.0,
        1.0,
        0.0,
        None,
        "use a contrast from {min} to {max}",
    ),
    number(
        "colorAdjust.saturation",
        "colorAdjust",
        0.0,
        2.0,
        1.0,
        None,
        "use a saturation from {min} to {max} (1 leaves it unchanged)",
    ),
    number(
        "colorAdjust.temperature",
        "colorAdjust",
        -1.0,
        1.0,
        0.0,
        None,
        "use a temperature from {min} (cool) to {max} (warm)",
    ),
    number(
        "colorAdjust.tint",
        "colorAdjust",
        -1.0,
        1.0,
        0.0,
        None,
        "use a tint from {min} (green) to {max} (magenta)",
    ),
    enabled("lut.enabled", "lut"),
    spec(
        "lut.asset",
        "lut",
        ParamType::AssetRef { kinds: &["lut"] },
        false,
        DefaultValue::None,
        None,
        "use a lut asset (a .cube file)",
    ),
    number(
        "lut.intensity",
        "lut",
        0.0,
        1.0,
        1.0,
        None,
        "use an intensity from {min} to {max}",
    ),
    enabled("blur.enabled", "blur"),
    number(
        "blur.radius",
        "blur",
        0.0,
        200.0,
        0.0,
        Some("px"),
        "use a blur radius from {min} to {max} {unit}",
    ),
];

/// The specs of effect kind `kind` (`enabled` included), in registry order.
pub(crate) fn of(kind: &str) -> impl Iterator<Item = &'static ParamSpec> + '_ {
    SPECS
        .iter()
        .filter(move |s| matches!(s.scope, Scope::Effect { kind: k } if k == kind))
}

/// The param names of effect kind `kind` (`enabled` included).
pub(super) fn params_of(kind: &str) -> impl Iterator<Item = &'static str> + '_ {
    of(kind).map(|s| s.path.split_once('.').map_or(s.path, |(_, p)| p))
}

/// Whether `kind` is a registry effect.
pub(crate) fn is_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// The spec of `param` (`radius` or `blur.radius`) of effect kind `kind`. `show` turns a param
/// name into the path the caller addresses it by (for alternatives).
pub(super) fn find(
    kind: &str,
    param: &str,
    show: impl Fn(&str) -> String,
) -> Result<&'static ParamSpec, CoreError> {
    if !is_kind(kind) {
        return Err(
            CoreError::new("PARAM_UNKNOWN", format!("no effect kind {kind}"))
                .with_details(json!({ "path": param, "kind": kind, "alternatives": KINDS }))
                .with_fix(format!("use an effect kind: {}", KINDS.join(", "))),
        );
    }
    let name = param
        .strip_prefix(kind)
        .and_then(|p| p.strip_prefix('.'))
        .unwrap_or(param);
    if let Some(s) = of(kind).find(|s| s.path.split_once('.').is_some_and(|(_, p)| p == name)) {
        return Ok(s);
    }
    let candidates: Vec<String> = params_of(kind).map(&show).collect();
    Err(unknown(
        format!("a {kind} effect has no parameter {name}"),
        &show(name),
        json!({ "kind": kind }),
        &candidates,
    ))
}
