//! Value checks against a spec, and the registry's errors.

use serde_json::{Value, json};

use crate::error::CoreError;
use crate::model::ParamValue;
use crate::params::{ParamSpec, ParamType};
use crate::time::Rational;

/// [`check_shape`] then [`check_range`], at the spec's own path.
pub fn validate(spec: &ParamSpec, value: &ParamValue) -> Result<(), CoreError> {
    check_shape(spec, spec.path, value)?;
    check_range(spec, spec.path, value)
}

/// The value's shape (structural): its variant, an enum name, a colour's form, a canonical
/// rational, whole pixels. `PARAM_TYPE` `{path, expected, got}` with the spec's fix text.
pub fn check_shape(spec: &ParamSpec, path: &str, value: &ParamValue) -> Result<(), CoreError> {
    let ok = match (&spec.ty, value) {
        (ParamType::Number { .. }, ParamValue::Number(_))
        | (ParamType::Bool, ParamValue::Bool(_))
        | (ParamType::Vec2 { .. }, ParamValue::Vec2(_))
        | (ParamType::Text { .. }, ParamValue::Text(_))
        | (ParamType::AssetRef { .. }, ParamValue::Asset { .. }) => true,
        (ParamType::Enum { values }, ParamValue::Text(s)) => values.contains(&s.as_str()),
        (ParamType::Color, ParamValue::Text(s)) => is_colour(s),
        (ParamType::Time { .. } | ParamType::Rational { .. }, ParamValue::Text(s)) => {
            s.parse::<Rational>().is_ok()
        }
        (ParamType::Size { .. }, ParamValue::Vec2(v)) => v
            .iter()
            .all(|x| *x >= 0.0 && *x <= f64::from(u32::MAX) && *x == x.floor()),
        _ => false,
    };
    if ok {
        return Ok(());
    }
    let (expected, phrase) = expected(&spec.ty);
    let got = match (&spec.ty, value) {
        (
            ParamType::Enum { .. }
            | ParamType::Color
            | ParamType::Time { .. }
            | ParamType::Rational { .. },
            ParamValue::Text(s),
        ) => format!("{s:?}"),
        (ParamType::Size { .. }, ParamValue::Vec2(v)) => vec2(*v),
        _ => value_phrase(value).to_string(),
    };
    Err(
        CoreError::new("PARAM_TYPE", format!("{path} takes {phrase}, not {got}"))
            .with_details(json!({ "path": path, "expected": expected, "got": value_kind(value) }))
            .with_fix(fill(spec, path)),
    )
}

/// The value's range (policy): numbers, each `vec2` component, times, rationals and sizes
/// inside the spec's bounds; text no longer than its limit. `INVALID_RANGE`
/// `{path, value, min, max, unit}` with the spec's fix text. Assumes [`check_shape`] passed.
pub fn check_range(spec: &ParamSpec, path: &str, value: &ParamValue) -> Result<(), CoreError> {
    let unit = spec.unit.map_or(String::new(), |u| format!(" {u}"));
    let out = |shown: String, v: Value, min: Value, max: Value, bounds: String| {
        CoreError::new("INVALID_RANGE", format!("{path} {shown} is {bounds}{unit}"))
            .with_details(json!({ "path": path, "value": v, "min": min, "max": max,
                                  "unit": spec.unit }))
            .with_fix(fill(spec, path))
    };
    match (&spec.ty, value) {
        (ParamType::Number { min, max }, ParamValue::Number(x)) if x < min || x > max => Err(out(
            x.to_string(),
            json!(x),
            json!(min),
            json!(max),
            format!("outside {min} to {max}"),
        )),
        (ParamType::Vec2 { min, max }, ParamValue::Vec2(v))
            if (0..2).any(|i| v[i] < min[i] || v[i] > max[i]) =>
        {
            Err(out(
                vec2(*v),
                json!(v),
                json!(min),
                json!(max),
                format!("outside {} to {}", vec2(*min), vec2(*max)),
            ))
        }
        (ParamType::Size { min, max }, ParamValue::Vec2(v))
            if v.iter()
                .any(|x| *x < f64::from(*min) || *x > f64::from(*max)) =>
        {
            Err(out(
                vec2(*v),
                json!(v),
                json!(min),
                json!(max),
                format!("outside {min} to {max}"),
            ))
        }
        (ParamType::Time { min: Some(min) }, ParamValue::Text(s)) => {
            let (v, lo) = (rational(s)?, rational(min)?);
            if v < lo {
                return Err(out(
                    s.clone(),
                    json!(s),
                    json!(min),
                    Value::Null,
                    format!("below {min}"),
                ));
            }
            Ok(())
        }
        (ParamType::Rational { min, max }, ParamValue::Text(s)) => {
            let (v, lo, hi) = (rational(s)?, rational(min)?, rational(max)?);
            if v < lo || v > hi {
                return Err(out(
                    s.clone(),
                    json!(s),
                    json!(min),
                    json!(max),
                    format!("outside {min} to {max}"),
                ));
            }
            Ok(())
        }
        (ParamType::Text { max_len }, ParamValue::Text(s))
            if s.chars().count() > *max_len as usize =>
        {
            let n = s.chars().count();
            Err(CoreError::new(
                "INVALID_RANGE",
                format!("{path} is {n} characters long; the limit is {max_len}"),
            )
            .with_details(json!({ "path": path, "value": n, "min": 0, "max": max_len,
                                  "unit": "characters" }))
            .with_fix(fill(spec, path)))
        }
        _ => Ok(()),
    }
}

fn rational(s: &str) -> Result<Rational, CoreError> {
    s.parse()
}

/// `#rrggbb` or `#rrggbbaa`, lowercase hex.
fn is_colour(s: &str) -> bool {
    let Some(hex) = s.strip_prefix('#') else {
        return false;
    };
    (hex.len() == 6 || hex.len() == 8)
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `x,y`, as `fin` takes a `vec2`.
fn vec2(v: [f64; 2]) -> String {
    format!("{},{}", v[0], v[1])
}

/// A type's name in `details.expected`, and how a message says it.
fn expected(ty: &ParamType) -> (&'static str, String) {
    match ty {
        ParamType::Number { .. } => ("number", "a number".into()),
        ParamType::Bool => ("bool", "true or false".into()),
        ParamType::Enum { values } => ("enum", format!("one of {}", values.join(", "))),
        ParamType::Color => ("color", "a colour #rrggbb or #rrggbbaa".into()),
        ParamType::Vec2 { .. } => ("vec2", "two numbers x,y".into()),
        ParamType::Time { .. } => ("time", "a time in seconds (n or n/d)".into()),
        ParamType::Rational { .. } => ("rational", "a rational (n or n/d)".into()),
        ParamType::Size { .. } => ("size", "a size in whole pixels w,h".into()),
        ParamType::Text { .. } => ("text", "text".into()),
        ParamType::AssetRef { kinds } => ("assetRef", format!("an asset ({})", kinds.join(", "))),
    }
}

/// A value's variant name in `details.got`.
pub(crate) fn value_kind(v: &ParamValue) -> &'static str {
    match v {
        ParamValue::Bool(_) => "bool",
        ParamValue::Number(_) => "number",
        ParamValue::Vec2(_) => "vec2",
        ParamValue::Text(_) => "text",
        ParamValue::Asset { .. } => "asset",
    }
}

fn value_phrase(v: &ParamValue) -> &'static str {
    match v {
        ParamValue::Bool(_) => "a bool",
        ParamValue::Number(_) => "a number",
        ParamValue::Vec2(_) => "a vec2",
        ParamValue::Text(_) => "text",
        ParamValue::Asset { .. } => "an asset",
    }
}

/// The spec's fix text with `{min}`, `{max}`, `{unit}` and `{path}` filled.
pub(crate) fn fill(spec: &ParamSpec, path: &str) -> String {
    let (min, max) = match &spec.ty {
        ParamType::Number { min, max } => (min.to_string(), max.to_string()),
        ParamType::Vec2 { min, max } => (vec2(*min), vec2(*max)),
        ParamType::Time { min } => (min.unwrap_or("").to_string(), String::new()),
        ParamType::Rational { min, max } => ((*min).to_string(), (*max).to_string()),
        ParamType::Size { min, max } => (min.to_string(), max.to_string()),
        ParamType::Text { max_len } => ("0".into(), max_len.to_string()),
        _ => (String::new(), String::new()),
    };
    spec.fix
        .replace("{min}", &min)
        .replace("{max}", &max)
        .replace("{unit}", spec.unit.unwrap_or(""))
        .replace("{path}", path)
}

/// `PARAM_UNKNOWN`: `message`, with `details` plus `path` and the closest of `candidates`
/// (at most three, by edit distance, ties in their order).
pub(crate) fn unknown(
    message: String,
    path: &str,
    mut details: Value,
    candidates: &[String],
) -> CoreError {
    let alternatives = closest(path, candidates);
    if let Value::Object(d) = &mut details {
        d.insert("path".into(), json!(path));
        d.insert("alternatives".into(), json!(alternatives));
    }
    let fix = if alternatives.is_empty() {
        "fin schema param lists every path".to_string()
    } else {
        format!(
            "use one of: {} (fin schema param lists every path)",
            alternatives.join(", ")
        )
    };
    CoreError::new("PARAM_UNKNOWN", message)
        .with_details(details)
        .with_fix(fix)
}

fn closest(path: &str, candidates: &[String]) -> Vec<String> {
    let mut ranked: Vec<(usize, usize, &String)> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (distance(path, c), i, c))
        .collect();
    ranked.sort();
    ranked
        .into_iter()
        .take(3)
        .map(|(_, _, c)| c.clone())
        .collect()
}

/// Levenshtein distance over chars.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = if ca == *cb {
                prev
            } else {
                1 + prev.min(here).min(row[j])
            };
            prev = here;
        }
    }
    row[b.len()]
}
