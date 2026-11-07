//! Track, timeline and marker settings (`track.set`, `timeline.set`, `marker.set`): each path's
//! value shape (structural, `PARAM_TYPE`) and range (policy, `INVALID_RANGE`), and reading and
//! writing them on the model.
//!
//! A stand-in for the settings registry's track, timeline and marker scopes (plan 4C Task 11,
//! `fin_core::params`, built beside this): the same paths, types, ranges and reset rules. When
//! both meet, [`check`] becomes `params::find` + `check_shape` + `check_range`, and the
//! getters and setters are `params::{get,set}_{track,timeline,marker}`.

use serde_json::json;

use crate::error::CoreError;
use crate::model::{Marker, ParamValue, Resolution, Timeline, Track};
use crate::time::Rational;

/// Whose settings a path names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Track,
    Timeline,
    Marker,
}

impl Scope {
    fn name(self) -> &'static str {
        match self {
            Scope::Track => "track",
            Scope::Timeline => "timeline",
            Scope::Marker => "marker",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Ty {
    Text {
        max_len: usize,
    },
    Bool,
    Number {
        min: f64,
        max: f64,
    },
    /// Whole pixels, `[width, height]`.
    Size {
        min: u32,
        max: u32,
    },
    /// A canonical rational as text.
    Rational {
        min: i64,
        max: i64,
    },
    /// `#rrggbb` or `#rrggbbaa`, lowercase.
    Color,
    /// Seconds as a canonical rational, at least 0.
    Time,
}

struct Spec {
    scope: Scope,
    path: &'static str,
    ty: Ty,
    unit: Option<&'static str>,
    fix: &'static str,
}

const NAME: Ty = Ty::Text { max_len: 64 };

const SPECS: &[Spec] = &[
    Spec {
        scope: Scope::Track,
        path: "name",
        ty: NAME,
        unit: None,
        fix: "use a name of at most 64 characters",
    },
    Spec {
        scope: Scope::Track,
        path: "mute",
        ty: Ty::Bool,
        unit: None,
        fix: "use true or false",
    },
    Spec {
        scope: Scope::Track,
        path: "solo",
        ty: Ty::Bool,
        unit: None,
        fix: "use true or false",
    },
    Spec {
        scope: Scope::Track,
        path: "lock",
        ty: Ty::Bool,
        unit: None,
        fix: "use true or false",
    },
    Spec {
        scope: Scope::Track,
        path: "gain",
        ty: Ty::Number {
            min: -96.0,
            max: 24.0,
        },
        unit: Some("dB"),
        fix: "use a gain from -96 to 24 dB",
    },
    Spec {
        scope: Scope::Timeline,
        path: "name",
        ty: NAME,
        unit: None,
        fix: "use a name of at most 64 characters",
    },
    Spec {
        scope: Scope::Timeline,
        path: "resolution",
        ty: Ty::Size {
            min: 16,
            max: 16384,
        },
        unit: Some("px"),
        fix: "use a width,height from 16 to 16384 px",
    },
    Spec {
        scope: Scope::Timeline,
        path: "rate",
        ty: Ty::Rational { min: 1, max: 240 },
        unit: Some("fps"),
        fix: "use a frame rate from 1 to 240 fps (n or n/d, such as 30000/1001)",
    },
    Spec {
        scope: Scope::Marker,
        path: "label",
        ty: Ty::Text { max_len: 256 },
        unit: None,
        fix: "use a label of at most 256 characters",
    },
    Spec {
        scope: Scope::Marker,
        path: "color",
        ty: Ty::Color,
        unit: None,
        fix: "use a colour #rrggbb or #rrggbbaa (lowercase hex)",
    },
    Spec {
        scope: Scope::Marker,
        path: "at",
        ty: Ty::Time,
        unit: Some("s"),
        fix: "use a time of at least 0 s",
    },
    Spec {
        scope: Scope::Marker,
        path: "duration",
        ty: Ty::Time,
        unit: Some("s"),
        fix: "use a duration of at least 0 s",
    },
];

fn spec(scope: Scope, path: &str) -> Result<&'static Spec, CoreError> {
    SPECS
        .iter()
        .find(|s| s.scope == scope && s.path == path)
        .ok_or_else(|| {
            let paths: Vec<&str> = SPECS
                .iter()
                .filter(|s| s.scope == scope)
                .map(|s| s.path)
                .collect();
            CoreError::new(
                "PARAM_UNKNOWN",
                format!("a {} has no parameter {path}", scope.name()),
            )
            .with_details(json!({ "path": path, "scope": scope.name(), "alternatives": paths }))
            .with_fix(format!("use one of: {}", paths.join(", ")))
        })
}

fn is_colour(s: &str) -> bool {
    let hex = s.strip_prefix('#').unwrap_or("");
    s.starts_with('#')
        && (hex.len() == 6 || hex.len() == 8)
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn wrong(path: &str, expected: &str, fix: &str) -> CoreError {
    CoreError::new("PARAM_TYPE", format!("{path} takes {expected}"))
        .with_details(json!({ "path": path, "expected": expected }))
        .with_fix(fix)
}

fn out_of_range(
    spec: &Spec,
    value: serde_json::Value,
    min: serde_json::Value,
    max: serde_json::Value,
) -> CoreError {
    CoreError::new(
        "INVALID_RANGE",
        format!("{} {value} is out of range", spec.path),
    )
    .with_details(
        json!({ "path": spec.path, "value": value, "min": min, "max": max,
                          "unit": spec.unit }),
    )
    .with_fix(spec.fix)
}

/// Checks `value` for `path` in `scope`: the path exists and the value has its shape
/// (structural), numbers are finite (`-0.0` becomes `0.0`), and, when `policy`, the value is in
/// range. No value (a reset) has nothing to check.
pub(crate) fn check(
    scope: Scope,
    path: &str,
    value: Option<&mut ParamValue>,
    policy: bool,
) -> Result<(), CoreError> {
    let spec = spec(scope, path)?;
    let Some(value) = value else {
        return Ok(());
    };
    match (spec.ty, &mut *value) {
        (Ty::Text { max_len }, ParamValue::Text(s)) => {
            if policy && s.chars().count() > max_len {
                return Err(out_of_range(spec, json!(s), json!(0), json!(max_len)));
            }
        }
        (Ty::Bool, ParamValue::Bool(_)) => {}
        (Ty::Number { min, max }, ParamValue::Number(x)) => {
            if !x.is_finite() {
                return Err(out_of_range(spec, json!(null), json!(min), json!(max)));
            }
            if *x == 0.0 {
                *x = 0.0;
            }
            if policy && (*x < min || *x > max) {
                return Err(out_of_range(spec, json!(*x), json!(min), json!(max)));
            }
        }
        (Ty::Size { min, max }, ParamValue::Vec2(v)) => {
            let whole = |x: f64| x.is_finite() && x >= 1.0 && x == x.floor() && x <= f64::from(max);
            if !v.iter().all(|x| whole(*x)) {
                return Err(wrong(path, "whole pixels [width, height]", spec.fix));
            }
            if policy && v.iter().any(|x| *x < f64::from(min)) {
                return Err(out_of_range(spec, json!(v), json!(min), json!(max)));
            }
        }
        (Ty::Rational { min, max }, ParamValue::Text(s)) => {
            let r: Rational = s
                .parse()
                .map_err(|_| wrong(path, "a rational (n or n/d)", spec.fix))?;
            if !r.is_positive() {
                return Err(out_of_range(spec, json!(s), json!(min), json!(max)));
            }
            if policy && (r < Rational::from_int(min) || r > Rational::from_int(max)) {
                return Err(out_of_range(spec, json!(s), json!(min), json!(max)));
            }
        }
        (Ty::Color, ParamValue::Text(s)) if is_colour(s) => {}
        (Ty::Time, ParamValue::Text(s)) => {
            let t: Rational = s
                .parse()
                .map_err(|_| wrong(path, "a time in seconds (n or n/d)", spec.fix))?;
            if t < Rational::zero() {
                return Err(out_of_range(spec, json!(s), json!("0"), json!(null)));
            }
        }
        (ty, _) => return Err(wrong(path, expected(ty), spec.fix)),
    }
    Ok(())
}

fn expected(ty: Ty) -> &'static str {
    match ty {
        Ty::Text { .. } => "text",
        Ty::Bool => "a bool",
        Ty::Number { .. } => "a number",
        Ty::Size { .. } => "whole pixels [width, height]",
        Ty::Rational { .. } => "a rational (n or n/d)",
        Ty::Color => "a colour #rrggbb or #rrggbbaa",
        Ty::Time => "a time in seconds (n or n/d)",
    }
}

fn required(path: &str) -> CoreError {
    CoreError::new("INVALID_OP", format!("{path} cannot be reset"))
        .with_details(json!({ "path": path }))
        .with_fix(format!("give {path} a value"))
}

fn text(v: Option<&ParamValue>) -> Option<String> {
    match v {
        Some(ParamValue::Text(s)) => Some(s.clone()),
        _ => None,
    }
}

fn boolean(v: Option<&ParamValue>) -> bool {
    matches!(v, Some(ParamValue::Bool(true)))
}

fn rational(path: &str, v: Option<&ParamValue>) -> Result<Rational, CoreError> {
    text(v).ok_or_else(|| required(path))?.parse()
}

fn time(r: Rational) -> ParamValue {
    ParamValue::Text(r.to_string())
}

/// The value of `path` of `track`.
pub(crate) fn get_track(track: &Track, path: &str) -> Result<Option<ParamValue>, CoreError> {
    spec(Scope::Track, path)?;
    Ok(Some(match path {
        "name" => ParamValue::Text(track.name.clone()),
        "mute" => ParamValue::Bool(track.mute),
        "solo" => ParamValue::Bool(track.solo),
        "lock" => ParamValue::Bool(track.lock),
        _ => ParamValue::Number(track.gain),
    }))
}

/// Sets `path` of `track` to a value [`check`] passed (`None` resets; a name cannot be reset),
/// returning the old value.
pub(crate) fn set_track(
    track: &mut Track,
    path: &str,
    value: Option<ParamValue>,
) -> Result<Option<ParamValue>, CoreError> {
    let old = get_track(track, path)?;
    let v = value.as_ref();
    match path {
        "name" => track.name = text(v).ok_or_else(|| required(path))?,
        "mute" => track.mute = boolean(v),
        "solo" => track.solo = boolean(v),
        "lock" => track.lock = boolean(v),
        _ => {
            track.gain = match v {
                Some(ParamValue::Number(x)) => *x,
                _ => 0.0,
            }
        }
    }
    Ok(old)
}

/// The value of `path` of `timeline`.
pub(crate) fn get_timeline(
    timeline: &Timeline,
    path: &str,
) -> Result<Option<ParamValue>, CoreError> {
    spec(Scope::Timeline, path)?;
    let r = timeline.resolution;
    Ok(Some(match path {
        "name" => ParamValue::Text(timeline.name.clone()),
        "resolution" => ParamValue::Vec2([f64::from(r.width), f64::from(r.height)]),
        _ => time(timeline.rate),
    }))
}

/// Sets `path` of `timeline` to a value [`check`] passed (`None` resets the resolution to
/// 1080×1920; a name or rate cannot be reset), returning the old value.
pub(crate) fn set_timeline(
    timeline: &mut Timeline,
    path: &str,
    value: Option<ParamValue>,
) -> Result<Option<ParamValue>, CoreError> {
    let old = get_timeline(timeline, path)?;
    let v = value.as_ref();
    match path {
        "name" => timeline.name = text(v).ok_or_else(|| required(path))?,
        "resolution" => {
            timeline.resolution = match v {
                // Whole and in range ([`check`]): the conversions are exact.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some(ParamValue::Vec2([w, h])) => Resolution {
                    width: *w as u32,
                    height: *h as u32,
                },
                _ => crate::model::DEFAULT_RESOLUTION,
            }
        }
        _ => timeline.rate = rational(path, v)?,
    }
    Ok(old)
}

/// The value of `path` of `marker` (`None`: no colour or duration).
pub(crate) fn get_marker(marker: &Marker, path: &str) -> Result<Option<ParamValue>, CoreError> {
    spec(Scope::Marker, path)?;
    Ok(match path {
        "label" => Some(ParamValue::Text(marker.label.clone())),
        "color" => marker.color.clone().map(ParamValue::Text),
        "at" => Some(time(marker.at)),
        _ => marker.duration.map(time),
    })
}

/// Sets `path` of `marker` to a value [`check`] passed (`None` clears a colour or duration; a
/// label or position cannot be reset), returning the old value.
pub(crate) fn set_marker(
    marker: &mut Marker,
    path: &str,
    value: Option<ParamValue>,
) -> Result<Option<ParamValue>, CoreError> {
    let old = get_marker(marker, path)?;
    let v = value.as_ref();
    match path {
        "label" => marker.label = text(v).ok_or_else(|| required(path))?,
        "color" => marker.color = text(v),
        "at" => marker.at = rational(path, v)?,
        _ => {
            marker.duration = match v {
                None => None,
                Some(_) => Some(rational(path, v)?),
            }
        }
    }
    Ok(old)
}
