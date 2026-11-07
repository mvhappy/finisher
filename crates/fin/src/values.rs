//! Parameter values as agents type them, parsed by the registry's [`ParamType`] (plan 4C
//! Task 23 for track, timeline, marker and slot values; Task 24 extends it to item, effect and
//! keyframe values):
//! - numbers take an optional unit (`-6dB`, `45deg`, `120px`; `50%` is the fraction `0.5`);
//! - `x,y` for `vec2`, `WxH` or `W,H` for sizes, `#rgb`/`#rrggbb`/`#rrggbbaa` for colours;
//! - `true|false` (also `on|off`, `yes|no`, `1|0`), enum names, times as in `addr`, rationals
//!   as `n`, `n/d` or a decimal.
//!
//! Ranges are the core's to check (`INVALID_RANGE` with the spec's fix text); a value of the
//! wrong shape is `PARAM_TYPE` here, naming the expected form.

use fin_core::params::{ParamSpec, ParamType};
use fin_core::{CoreError, ParamValue, Rational, parse_time};
use serde_json::json;

fn wrong(path: &str, input: &str, expected: &str, example: &str) -> CoreError {
    CoreError::new(
        "PARAM_TYPE",
        format!("{path} takes {expected}, not {input:?}"),
    )
    .with_details(json!({ "path": path, "input": input, "expected": expected }))
    .with_fix(format!("write {path} as {expected}, e.g. {example}"))
}

/// A finite number, with the spec's unit or `%` allowed after it.
fn number(path: &str, input: &str, unit: Option<&str>) -> Result<f64, CoreError> {
    let bad = || {
        wrong(
            path,
            input,
            "a number",
            unit.map_or("0.5".into(), |u| format!("3{u}")).as_str(),
        )
    };
    let text = input.trim();
    let (body, scale) = match text.strip_suffix('%') {
        Some(b) => (b, 100.0),
        None => (unit.and_then(|u| text.strip_suffix(u)).unwrap_or(text), 1.0),
    };
    let v: f64 = body.trim().parse().map_err(|_| bad())?;
    if !v.is_finite() {
        return Err(bad());
    }
    let v = v / scale;
    // `-0` is stored as `0` (the core refuses nothing here, but keeps values canonical).
    Ok(if v == 0.0 { 0.0 } else { v })
}

fn pair(
    path: &str,
    input: &str,
    seps: &[char],
    expected: &str,
    ex: &str,
) -> Result<[f64; 2], CoreError> {
    let bad = || wrong(path, input, expected, ex);
    let parts: Vec<&str> = input.split(|c| seps.contains(&c)).collect();
    let [a, b] = parts.as_slice() else {
        return Err(bad());
    };
    let x: f64 = a.trim().parse().map_err(|_| bad())?;
    let y: f64 = b.trim().parse().map_err(|_| bad())?;
    if !x.is_finite() || !y.is_finite() {
        return Err(bad());
    }
    Ok([x, y])
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` → lowercase `#rrggbb` / `#rrggbbaa`.
fn color(path: &str, input: &str) -> Result<String, CoreError> {
    let bad = || wrong(path, input, "a colour #rrggbb or #rrggbbaa", "#ff8800");
    let hex = input.trim().strip_prefix('#').ok_or_else(bad)?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let hex = hex.to_ascii_lowercase();
    match hex.len() {
        3 => Ok(format!(
            "#{}",
            hex.chars().flat_map(|c| [c, c]).collect::<String>()
        )),
        6 | 8 => Ok(format!("#{hex}")),
        _ => Err(bad()),
    }
}

fn boolean(path: &str, input: &str) -> Result<bool, CoreError> {
    match input.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err(wrong(path, input, "true or false", "true")),
    }
}

/// `n`, `n/d` or a decimal, exactly.
fn rational(path: &str, input: &str) -> Result<Rational, CoreError> {
    let bad = || wrong(path, input, "a number n, n/d or a decimal", "30000/1001");
    let text = input.trim();
    if let Ok(r) = text.parse::<Rational>() {
        return Ok(r);
    }
    // A decimal ("29.97", "12.5"): read as seconds at rate 1, which is exact.
    let r = parse_time(&format!("{text}s"), Rational::from_int(1)).map_err(|_| bad())?;
    Ok(r)
}

/// Parses `input` as a value of `spec` (named `path` in errors). `rate`: the frame rate that
/// `f<n>` times count in.
pub fn parse(
    spec: &ParamSpec,
    path: &str,
    input: &str,
    rate: Rational,
) -> Result<ParamValue, CoreError> {
    Ok(match spec.ty {
        ParamType::Number { .. } => ParamValue::Number(number(path, input, spec.unit)?),
        ParamType::Bool => ParamValue::Bool(boolean(path, input)?),
        ParamType::Enum { values } => {
            let v = input.trim();
            if !values.contains(&v) {
                return Err(wrong(
                    path,
                    input,
                    &format!("one of {}", values.join(", ")),
                    values[0],
                )
                .with_details(json!({ "path": path, "input": input, "alternatives": values })));
            }
            ParamValue::Text(v.to_string())
        }
        ParamType::Color => ParamValue::Text(color(path, input)?),
        ParamType::Vec2 { .. } => ParamValue::Vec2(pair(path, input, &[','], "x,y", "0.5,0.5")?),
        ParamType::Size { .. } => {
            ParamValue::Vec2(pair(path, input, &['x', 'X', ','], "WxH", "1080x1920")?)
        }
        ParamType::Time { .. } => {
            let t = parse_time(input.trim(), rate).map_err(|e| {
                let mut e = e;
                if let Some(d) = e.details.as_object_mut() {
                    d.insert("path".into(), json!(path));
                }
                e
            })?;
            ParamValue::Text(t.to_string())
        }
        ParamType::Rational { .. } => ParamValue::Text(rational(path, input)?.to_string()),
        ParamType::Text { .. } => ParamValue::Text(input.to_string()),
        ParamType::AssetRef { .. } => ParamValue::Asset {
            asset: input.trim().to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_core::params::{ScopeOf, find};

    fn p(scope: ScopeOf<'_>, path: &str, input: &str) -> Result<ParamValue, CoreError> {
        let spec = find(&scope, path).unwrap();
        parse(spec, path, input, Rational::from_int(30))
    }

    #[test]
    fn values_parse_by_type() {
        assert_eq!(
            p(ScopeOf::Track, "gain", "-6dB").unwrap(),
            ParamValue::Number(-6.0)
        );
        assert_eq!(
            p(ScopeOf::Track, "gain", "-0").unwrap(),
            ParamValue::Number(0.0)
        );
        assert_eq!(
            p(ScopeOf::Track, "mute", "on").unwrap(),
            ParamValue::Bool(true)
        );
        assert_eq!(
            p(ScopeOf::Track, "name", "Dialog").unwrap(),
            ParamValue::Text("Dialog".into())
        );
        assert_eq!(
            p(ScopeOf::Timeline, "resolution", "1920x1080").unwrap(),
            ParamValue::Vec2([1920.0, 1080.0])
        );
        assert_eq!(
            p(ScopeOf::Timeline, "rate", "29.97").unwrap(),
            ParamValue::Text("2997/100".into())
        );
        assert_eq!(
            p(ScopeOf::Timeline, "rate", "30000/1001").unwrap(),
            ParamValue::Text("30000/1001".into())
        );
        assert_eq!(
            p(ScopeOf::Marker, "color", "#F80").unwrap(),
            ParamValue::Text("#ff8800".into())
        );
        assert_eq!(
            p(ScopeOf::Marker, "at", "f45").unwrap(),
            ParamValue::Text("3/2".into())
        );
        assert_eq!(
            p(ScopeOf::Marker, "duration", "2.5s").unwrap(),
            ParamValue::Text("5/2".into())
        );
    }

    #[test]
    fn wrong_shapes_are_param_type_with_the_expected_form() {
        for (scope, path, input) in [
            (ScopeOf::Track, "gain", "loud"),
            (ScopeOf::Track, "mute", "maybe"),
            (ScopeOf::Timeline, "resolution", "1080"),
            (ScopeOf::Marker, "color", "red"),
            (ScopeOf::Marker, "color", "#12345"),
        ] {
            let e = p(scope, path, input).unwrap_err();
            assert_eq!(e.code, "PARAM_TYPE", "{path} {input}");
            assert_eq!(e.details["path"], path);
            assert!(e.fix.unwrap().contains("e.g."));
        }
        assert_eq!(
            p(ScopeOf::Marker, "at", "soon").unwrap_err().details["path"],
            "at"
        );
    }
}
