//! Reading and writing an item's parameters by path, on the typed model. No range checks:
//! [`crate::params::validate`] is the caller's. A value of the wrong variant is `PARAM_TYPE`,
//! a path the item does not have `PARAM_UNKNOWN`.
//!
//! `None` means nothing is stored: an effect param or slot left at its default, or the audio
//! params of an item without any. Setting `None` resets: the field takes its default, a sparse
//! entry is removed, and audio params that are then all default are dropped. Setting a value
//! never drops anything, so the old value [`set`] returns is always an exact inverse.

use std::collections::BTreeMap;

use serde_json::json;

use crate::error::CoreError;
use crate::model::{AudioParams, BlendMode, Item, ItemBody, Mask, ParamValue};
use crate::params::effect_path;
use crate::time::Rational;

/// The value stored at `path` of `item` (see the module docs).
pub fn get(item: &Item, path: &str) -> Result<Option<ParamValue>, CoreError> {
    let c = &item.common;
    let t = &c.transform;
    let v = match path {
        "transform.position" => ParamValue::Vec2(t.position),
        "transform.scale" => ParamValue::Vec2(t.scale),
        "transform.rotation" => ParamValue::Number(t.rotation),
        "transform.anchor" => ParamValue::Vec2(t.anchor),
        "crop.left" => ParamValue::Number(c.crop.left),
        "crop.right" => ParamValue::Number(c.crop.right),
        "crop.top" => ParamValue::Number(c.crop.top),
        "crop.bottom" => ParamValue::Number(c.crop.bottom),
        "opacity" => ParamValue::Number(c.opacity),
        "blend" => ParamValue::Text(enum_name(&c.blend)),
        "speed" => match &item.body {
            ItemBody::Clip { speed, .. } => time(*speed),
            _ => return Err(unknown(&item.id, path)),
        },
        p if p.starts_with("audio.") => {
            if !AUDIO.contains(&p) {
                return Err(unknown(&item.id, path));
            }
            let Some(a) = &c.audio else {
                return Ok(None);
            };
            match p {
                "audio.gain" => ParamValue::Number(a.gain),
                "audio.pan" => ParamValue::Number(a.pan),
                "audio.fadeIn" => time(a.fade_in),
                "audio.fadeOut" => time(a.fade_out),
                "audio.fadeInOffset" => time(a.fade_in_offset),
                "audio.fadeOutOffset" => time(a.fade_out_offset),
                _ => return Err(unknown(&item.id, path)),
            }
        }
        p if p.starts_with("mask.") => {
            let m = c.mask.as_ref().ok_or_else(|| unknown(&item.id, path))?;
            match p {
                "mask.feather" => ParamValue::Number(m.feather),
                "mask.choke" => ParamValue::Number(m.choke),
                "mask.invert" => ParamValue::Bool(m.invert),
                _ => return Err(unknown(&item.id, path)),
            }
        }
        p => {
            if let Some((fx, param)) = effect_path(p) {
                let e = c
                    .effects
                    .iter()
                    .find(|e| e.id == fx)
                    .ok_or_else(|| unknown(&item.id, path))?;
                if param == "enabled" {
                    return Ok(Some(ParamValue::Bool(e.enabled)));
                }
                return Ok(e.params.get(param).cloned());
            }
            return slots(item, path).map(|s| s.get(path_slot(path)).cloned());
        }
    };
    Ok(Some(v))
}

/// Sets `path` of `item` to `value` (`None` resets), returning what was stored before.
pub fn set(
    item: &mut Item,
    path: &str,
    value: Option<ParamValue>,
) -> Result<Option<ParamValue>, CoreError> {
    let old = get(item, path)?;
    let id = item.id.clone();
    let v = value.as_ref();
    let c = &mut item.common;
    let t = &mut c.transform;
    match path {
        "transform.position" => t.position = vec2(path, v, [0.0, 0.0])?,
        "transform.scale" => t.scale = vec2(path, v, [1.0, 1.0])?,
        "transform.rotation" => t.rotation = number(path, v, 0.0)?,
        "transform.anchor" => t.anchor = vec2(path, v, [0.5, 0.5])?,
        "crop.left" => c.crop.left = number(path, v, 0.0)?,
        "crop.right" => c.crop.right = number(path, v, 0.0)?,
        "crop.top" => c.crop.top = number(path, v, 0.0)?,
        "crop.bottom" => c.crop.bottom = number(path, v, 0.0)?,
        "opacity" => c.opacity = number(path, v, 1.0)?,
        "blend" => c.blend = from_name(path, v, BlendMode::Normal)?,
        "speed" => match &mut item.body {
            ItemBody::Clip { speed, .. } => *speed = rational(path, v, Rational::from_int(1))?,
            _ => return Err(unknown(&id, path)),
        },
        p if p.starts_with("audio.") => {
            let mut a = c.audio.clone().unwrap_or_default();
            match p {
                "audio.gain" => a.gain = number(path, v, 0.0)?,
                "audio.pan" => a.pan = number(path, v, 0.0)?,
                "audio.fadeIn" => a.fade_in = rational(path, v, Rational::zero())?,
                "audio.fadeOut" => a.fade_out = rational(path, v, Rational::zero())?,
                "audio.fadeInOffset" => a.fade_in_offset = rational(path, v, Rational::zero())?,
                "audio.fadeOutOffset" => a.fade_out_offset = rational(path, v, Rational::zero())?,
                _ => return Err(unknown(&id, path)),
            }
            let drop = v.is_none() && a == AudioParams::default() && bits_default(&a);
            c.audio = (!drop).then_some(a);
        }
        p if p.starts_with("mask.") => {
            let m: &mut Mask = c.mask.as_mut().ok_or_else(|| unknown(&id, path))?;
            match p {
                "mask.feather" => m.feather = number(path, v, 0.0)?,
                "mask.choke" => m.choke = number(path, v, 0.0)?,
                "mask.invert" => m.invert = boolean(path, v, false)?,
                _ => return Err(unknown(&id, path)),
            }
        }
        p => {
            if let Some((fx, param)) = effect_path(p) {
                let Some(e) = c.effects.iter_mut().find(|e| e.id == fx) else {
                    return Err(unknown(&id, path));
                };
                if param == "enabled" {
                    e.enabled = boolean(path, v, true)?;
                } else {
                    put(&mut e.params, param, value);
                }
                return Ok(old);
            }
            let ItemBody::Graphic { slots, .. } = &mut item.body else {
                return Err(unknown(&id, path));
            };
            put(slots, path_slot(path), value);
        }
    }
    Ok(old)
}

/// Audio params at their default bit for bit (`-0.0` is not dropped).
fn bits_default(a: &AudioParams) -> bool {
    a.gain.to_bits() == 0 && a.pan.to_bits() == 0
}

const AUDIO: &[&str] = &[
    "audio.gain",
    "audio.pan",
    "audio.fadeIn",
    "audio.fadeOut",
    "audio.fadeInOffset",
    "audio.fadeOutOffset",
];

fn slots<'a>(item: &'a Item, path: &str) -> Result<&'a BTreeMap<String, ParamValue>, CoreError> {
    match (&item.body, path.strip_prefix("slots.")) {
        (ItemBody::Graphic { slots, .. }, Some(_)) => Ok(slots),
        _ => Err(unknown(&item.id, path)),
    }
}

fn path_slot(path: &str) -> &str {
    path.strip_prefix("slots.").unwrap_or(path)
}

fn put(map: &mut BTreeMap<String, ParamValue>, key: &str, value: Option<ParamValue>) {
    match value {
        Some(v) => {
            map.insert(key.to_string(), v);
        }
        None => {
            map.remove(key);
        }
    }
}

fn unknown(item: &str, path: &str) -> CoreError {
    CoreError::new(
        "PARAM_UNKNOWN",
        format!("item {item} has no parameter {path}"),
    )
    .with_details(json!({ "path": path, "item": item, "alternatives": [] }))
    .with_fix("fin schema param lists every path")
}

pub(super) fn wrong(path: &str, expected: &str, v: &ParamValue) -> CoreError {
    CoreError::new("PARAM_TYPE", format!("{path} takes {expected}, not {v:?}"))
        .with_details(json!({ "path": path, "expected": expected,
                          "got": crate::params::check::value_kind(v) }))
}

pub(super) fn time(r: Rational) -> ParamValue {
    ParamValue::Text(r.to_string())
}

pub(super) fn number(path: &str, v: Option<&ParamValue>, default: f64) -> Result<f64, CoreError> {
    match v {
        None => Ok(default),
        Some(ParamValue::Number(x)) => Ok(*x),
        Some(other) => Err(wrong(path, "number", other)),
    }
}

pub(super) fn boolean(
    path: &str,
    v: Option<&ParamValue>,
    default: bool,
) -> Result<bool, CoreError> {
    match v {
        None => Ok(default),
        Some(ParamValue::Bool(b)) => Ok(*b),
        Some(other) => Err(wrong(path, "bool", other)),
    }
}

pub(super) fn vec2(
    path: &str,
    v: Option<&ParamValue>,
    default: [f64; 2],
) -> Result<[f64; 2], CoreError> {
    match v {
        None => Ok(default),
        Some(ParamValue::Vec2(x)) => Ok(*x),
        Some(other) => Err(wrong(path, "vec2", other)),
    }
}

pub(super) fn text(path: &str, v: Option<&ParamValue>) -> Result<Option<String>, CoreError> {
    match v {
        None => Ok(None),
        Some(ParamValue::Text(s)) => Ok(Some(s.clone())),
        Some(other) => Err(wrong(path, "text", other)),
    }
}

pub(super) fn rational(
    path: &str,
    v: Option<&ParamValue>,
    default: Rational,
) -> Result<Rational, CoreError> {
    match text(path, v)? {
        None => Ok(default),
        Some(s) => s.parse(),
    }
}

/// An enum's wire name.
fn enum_name<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

fn from_name<T: serde::de::DeserializeOwned>(
    path: &str,
    v: Option<&ParamValue>,
    default: T,
) -> Result<T, CoreError> {
    match text(path, v)? {
        None => Ok(default),
        Some(s) => serde_json::from_value(json!(s))
            .map_err(|_| wrong(path, "enum", &ParamValue::Text(s.clone()))),
    }
}
