//! Caption style parameters (plan 4C Task 18): the `style` scope of the registry, and reading
//! and writing a [`CaptionStyle`] by path (`style.add`'s `set`, `style.set`, a caption's
//! `overrides`). None is animatable.
//!
//! `None` means nothing is stored: no stroke, background, highlight or maximum duration (or,
//! inside an absent stroke or box, its field). Setting `None` resets a field to the default
//! style's ([`CaptionStyle::default`]); a stroke or box equal to its default is dropped, so it
//! exists exactly while it differs from it and every set has an exact inverse.

use serde_json::json;

use crate::error::CoreError;
use crate::layout::Align;
use crate::model::{Anchor, CaptionBox, CaptionStyle, ParamValue, Stroke};
use crate::params::access::{boolean, number, rational, text, time, wrong};
use crate::params::{DefaultValue, ParamSpec, ParamType, Scope};
use crate::time::Rational;

const fn spec(
    path: &'static str,
    ty: ParamType,
    default: DefaultValue,
    unit: Option<&'static str>,
    fix: &'static str,
) -> ParamSpec {
    ParamSpec {
        path,
        scope: Scope::Style,
        ty,
        animatable: false,
        default,
        unit,
        fix,
    }
}

const fn num(min: f64, max: f64) -> ParamType {
    ParamType::Number { min, max }
}

const COLOUR: &str = "use a colour #rrggbb or #rrggbbaa (lowercase hex)";
const RANGE: &str = "use a value from {min} to {max} {unit}";

/// The names of the safe zones (`captions::safe_zones`, in that order).
pub(crate) const SAFE_ZONES: &[&str] =
    &["tiktok", "reels", "shorts", "generic-9x16", "generic-16x9"];

pub(super) const STYLE: &[ParamSpec] = &[
    spec(
        "name",
        ParamType::Text { max_len: 64 },
        DefaultValue::None,
        None,
        "use a name of 1 to {max} characters",
    ),
    spec(
        "text.family",
        ParamType::Text { max_len: 128 },
        DefaultValue::Text("Inter"),
        None,
        "use a font family name (fin fonts ls --catalog)",
    ),
    spec(
        "text.weight",
        num(100.0, 900.0),
        DefaultValue::Number(700.0),
        None,
        "use a whole weight from {min} to {max}",
    ),
    spec(
        "text.italic",
        ParamType::Bool,
        DefaultValue::Bool(false),
        None,
        "use true or false",
    ),
    spec(
        "text.size",
        num(1.0, 1000.0),
        DefaultValue::Number(64.0),
        Some("px"),
        RANGE,
    ),
    spec(
        "text.lineHeight",
        num(0.5, 4.0),
        DefaultValue::Number(1.2),
        None,
        "use a line height from {min} to {max} times the size",
    ),
    spec(
        "text.letterSpacing",
        num(-0.5, 2.0),
        DefaultValue::Number(0.0),
        Some("em"),
        RANGE,
    ),
    spec(
        "text.align",
        ParamType::Enum {
            values: &["left", "center", "right"],
        },
        DefaultValue::Text("center"),
        None,
        "use left, center or right",
    ),
    spec(
        "color",
        ParamType::Color,
        DefaultValue::Text("#ffffff"),
        None,
        COLOUR,
    ),
    spec(
        "stroke.color",
        ParamType::Color,
        DefaultValue::None,
        None,
        COLOUR,
    ),
    spec(
        "stroke.width",
        num(0.0, 100.0),
        DefaultValue::None,
        Some("px"),
        RANGE,
    ),
    spec(
        "background.color",
        ParamType::Color,
        DefaultValue::None,
        None,
        COLOUR,
    ),
    spec(
        "background.padding",
        num(0.0, 500.0),
        DefaultValue::None,
        Some("px"),
        RANGE,
    ),
    spec(
        "background.radius",
        num(0.0, 500.0),
        DefaultValue::None,
        Some("px"),
        RANGE,
    ),
    spec(
        "highlight",
        ParamType::Color,
        DefaultValue::None,
        None,
        COLOUR,
    ),
    spec(
        "position.anchor",
        ParamType::Enum {
            values: &["top", "middle", "bottom"],
        },
        DefaultValue::Text("bottom"),
        None,
        "use top, middle or bottom",
    ),
    spec(
        "position.offset",
        num(0.0, 1.0),
        DefaultValue::Number(0.0),
        None,
        "use a fraction of the safe zone's height from {min} to {max}",
    ),
    spec(
        "safeZone",
        ParamType::Enum { values: SAFE_ZONES },
        DefaultValue::Text("generic-9x16"),
        None,
        "use tiktok, reels, shorts, generic-9x16 or generic-16x9",
    ),
    spec(
        "maxChars",
        num(1.0, 200.0),
        DefaultValue::Number(32.0),
        None,
        "use a whole number of characters per line from {min} to {max}",
    ),
    spec(
        "maxLines",
        num(1.0, 10.0),
        DefaultValue::Number(2.0),
        None,
        "use a whole number of lines from {min} to {max}",
    ),
    spec(
        "gap",
        ParamType::Time { min: Some("0") },
        DefaultValue::Text("1/2"),
        Some("s"),
        "use a pause of at least {min} {unit} (0: never split on pauses)",
    ),
    spec(
        "maxDuration",
        ParamType::Time {
            min: Some("1/1000"),
        },
        DefaultValue::None,
        Some("s"),
        "use a duration of at least {min} {unit}",
    ),
];

fn unknown(path: &str) -> CoreError {
    CoreError::new("PARAM_UNKNOWN", format!("a style has no parameter {path}"))
        .with_details(json!({ "path": path, "scope": "style", "alternatives": [] }))
        .with_fix("fin schema param --scope style lists every path")
}

fn align_name(a: Align) -> &'static str {
    match a {
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
    }
}

fn anchor_name(a: Anchor) -> &'static str {
    match a {
        Anchor::Top => "top",
        Anchor::Middle => "middle",
        Anchor::Bottom => "bottom",
    }
}

fn text_value(s: &str) -> ParamValue {
    ParamValue::Text(s.to_string())
}

/// A whole number of `path` in `[0, max]` (`PARAM_TYPE` otherwise).
fn whole(path: &str, v: Option<&ParamValue>, default: f64, max: f64) -> Result<f64, CoreError> {
    let x = number(path, v, default)?;
    if x < 0.0 || x > max || x != x.floor() {
        return Err(wrong(path, "whole number", &ParamValue::Number(x)));
    }
    Ok(x)
}

/// The value stored at `path` of `style` (see the module docs).
pub fn get_style(style: &CaptionStyle, path: &str) -> Result<Option<ParamValue>, CoreError> {
    let t = &style.text;
    let n = |x: f64| Some(ParamValue::Number(x));
    Ok(match path {
        "name" => Some(text_value(&style.name)),
        "text.family" => Some(text_value(&t.family)),
        "text.weight" => n(f64::from(t.weight)),
        "text.italic" => Some(ParamValue::Bool(t.italic)),
        "text.size" => n(t.size),
        "text.lineHeight" => n(t.line_height),
        "text.letterSpacing" => n(t.letter_spacing),
        "text.align" => Some(text_value(align_name(t.align))),
        "color" => Some(text_value(&style.color)),
        "stroke.color" => style.stroke.as_ref().map(|s| text_value(&s.color)),
        "stroke.width" => style.stroke.as_ref().and_then(|s| n(s.width)),
        "background.color" => style.background.as_ref().map(|b| text_value(&b.color)),
        "background.padding" => style.background.as_ref().and_then(|b| n(b.padding)),
        "background.radius" => style.background.as_ref().and_then(|b| n(b.radius)),
        "highlight" => style.highlight.as_deref().map(text_value),
        "position.anchor" => Some(text_value(anchor_name(style.position.anchor))),
        "position.offset" => n(style.position.offset),
        "safeZone" => Some(text_value(&style.safe_zone)),
        "maxChars" => n(f64::from(style.max_chars)),
        "maxLines" => n(f64::from(style.max_lines)),
        "gap" => Some(time(style.gap)),
        "maxDuration" => style.max_duration.map(time),
        _ => return Err(unknown(path)),
    })
}

/// Sets `path` of `style` to `value` (`None` resets), returning what was stored before.
/// Shapes are checked here only as far as the model needs (`PARAM_TYPE`); ranges are the
/// registry's ([`crate::params::validate`]).
pub fn set_style(
    style: &mut CaptionStyle,
    path: &str,
    value: Option<ParamValue>,
) -> Result<Option<ParamValue>, CoreError> {
    let old = get_style(style, path)?;
    let base = CaptionStyle::default();
    let v = value.as_ref();
    let t = &mut style.text;
    let required =
        |path: &str| -> Result<String, CoreError> { Ok(text(path, v)?.unwrap_or_default()) };
    match path {
        "name" => style.name = required(path)?,
        "text.family" => t.family = text(path, v)?.unwrap_or(base.text.family),
        "text.weight" => {
            // Whole and at most 1000: the conversion is exact.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let w = whole(path, v, f64::from(base.text.weight), 1000.0)? as u16;
            t.weight = w;
        }
        "text.italic" => t.italic = boolean(path, v, base.text.italic)?,
        "text.size" => t.size = number(path, v, base.text.size)?,
        "text.lineHeight" => t.line_height = number(path, v, base.text.line_height)?,
        "text.letterSpacing" => t.letter_spacing = number(path, v, base.text.letter_spacing)?,
        "text.align" => {
            t.align = match text(path, v)?.as_deref() {
                None => base.text.align,
                Some("left") => Align::Left,
                Some("center") => Align::Center,
                Some("right") => Align::Right,
                Some(other) => return Err(wrong(path, "alignment", &text_value(other))),
            }
        }
        "color" => style.color = text(path, v)?.unwrap_or(base.color),
        "stroke.color" | "stroke.width" => {
            let mut s = style.stroke.take().unwrap_or_default();
            let d = Stroke::default();
            match path {
                "stroke.color" => s.color = text(path, v)?.unwrap_or(d.color),
                _ => s.width = number(path, v, d.width)?,
            }
            style.stroke = (s != Stroke::default()).then_some(s);
        }
        "background.color" | "background.padding" | "background.radius" => {
            let mut b = style.background.take().unwrap_or_default();
            let d = CaptionBox::default();
            match path {
                "background.color" => b.color = text(path, v)?.unwrap_or(d.color),
                "background.padding" => b.padding = number(path, v, d.padding)?,
                _ => b.radius = number(path, v, d.radius)?,
            }
            style.background = (b != CaptionBox::default()).then_some(b);
        }
        "highlight" => style.highlight = text(path, v)?,
        "position.anchor" => {
            style.position.anchor = match text(path, v)?.as_deref() {
                None => base.position.anchor,
                Some("top") => Anchor::Top,
                Some("middle") => Anchor::Middle,
                Some("bottom") => Anchor::Bottom,
                Some(other) => return Err(wrong(path, "anchor", &text_value(other))),
            }
        }
        "position.offset" => style.position.offset = number(path, v, base.position.offset)?,
        "safeZone" => style.safe_zone = text(path, v)?.unwrap_or(base.safe_zone),
        "maxChars" | "maxLines" => {
            let d = if path == "maxChars" {
                base.max_chars
            } else {
                base.max_lines
            };
            // Whole and at most 1000: the conversion is exact.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = whole(path, v, f64::from(d), 1000.0)? as u32;
            if path == "maxChars" {
                style.max_chars = n;
            } else {
                style.max_lines = n;
            }
        }
        "gap" => style.gap = rational(path, v, base.gap)?,
        "maxDuration" => {
            style.max_duration = match v {
                None => None,
                Some(_) => Some(rational(path, v, Rational::zero())?),
            }
        }
        _ => return Err(unknown(path)),
    }
    Ok(old)
}
