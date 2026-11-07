//! dotLottie templates (`.lottie`, a zip): `manifest.json` names the animations; the first
//! one, `animations/<id>.json`, gives the canvas (`w`, `h`), the rate (`fr`), the frames
//! (`ip`..`op`), the slots (`slots`, by value kind) and the fonts (`fonts.list`). Malformed
//! JSON, slots or `chars` (glyph shapes thorvg crashes on, arch §27.10) are
//! `UNSUPPORTED_CONTAINER` with `details.reason`.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use fin_core::{AssetKind, CoreError, LottieInfo, Rational, SlotSpec};
use serde_json::{Map, Value};

use crate::{Probe, READ_LIMIT, unsupported};

/// A member's JSON, read whole up to [`READ_LIMIT`] (decompressed).
fn member(archive: &mut zip::ZipArchive<&mut File>, name: &str) -> Result<Value, String> {
    let entry = archive
        .by_name(name)
        .map_err(|e| format!("dotLottie member {name}: {e}"))?;
    let mut bytes = Vec::new();
    entry
        .take(READ_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("dotLottie member {name}: {e}"))?;
    if bytes.len() as u64 > READ_LIMIT {
        return Err(format!("dotLottie member {name} is too large"));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("malformed JSON in {name}: {e}"))
}

/// The first animation's id: a plain file name (no separators, not `.`/`..`).
fn first_animation(manifest: &Value) -> Result<String, String> {
    let id = manifest
        .get("animations")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|a| a.get("id"))
        .and_then(Value::as_str)
        .ok_or("manifest.json lists no animation id")?;
    let plain = !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\', '\0']);
    if !plain {
        return Err(format!("animation id {id:?} is not a file name"));
    }
    Ok(id.to_string())
}

fn number(anim: &Map<String, Value>, key: &str) -> Result<f64, String> {
    anim.get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("animation has no numeric {key}"))
}

/// An exact rate for `fr`: whole numbers as they are, NTSC rates as `n×1000/1001`, others to
/// a thousandth.
pub(crate) fn rate(fr: f64) -> Option<Rational> {
    if !(fr > 0.0 && fr <= 1000.0) {
        return None;
    }
    if (fr - fr.round()).abs() < 1e-6 {
        return Rational::new(fr.round() as i64, 1).ok();
    }
    let k = (fr * 1.001).round();
    if (k * 1000.0 / 1001.0 - fr).abs() < 1e-3 {
        return Rational::new(k as i64 * 1000, 1001).ok();
    }
    Rational::new((fr * 1000.0).round() as i64, 1000).ok()
}

/// A frame count to a thousandth (whole counts exactly).
fn frames(n: f64) -> Option<Rational> {
    Rational::new((n * 1000.0).round() as i64, 1000).ok()
}

fn dim(anim: &Map<String, Value>, key: &str) -> Result<u32, String> {
    let v = number(anim, key)?;
    let ok = v >= 1.0 && v.fract() == 0.0 && v <= f64::from(u32::MAX);
    ok.then_some(v as u32)
        .ok_or_else(|| format!("animation {key} must be a positive whole number"))
}

fn is_num(v: &Value) -> bool {
    v.as_f64().is_some_and(f64::is_finite)
}

/// The kind of a static value or keyframe value `v`: a text document (`{t}`), a color (3–4
/// numbers) or a number (one number, alone or in an array).
fn value_kind(v: &Value) -> Option<&'static str> {
    if v.get("t").is_some_and(Value::is_string) {
        return Some("text");
    }
    match v.as_array() {
        Some(a) if (3..=4).contains(&a.len()) && a.iter().all(is_num) => Some("color"),
        Some(a) if a.len() == 1 && is_num(&a[0]) => Some("number"),
        Some(_) => None,
        None => is_num(v).then_some("number"),
    }
}

/// A slot's kind from its value `p`: an image asset (`w`, `h`, `p`), else the one kind of its
/// static value `k` or of every keyframe's `s` (text documents are always keyframes).
fn slot_kind(p: &Value) -> Option<&'static str> {
    let obj = p.as_object()?;
    if obj.get("p").is_some_and(Value::is_string) && is_num(obj.get("w")?) && is_num(obj.get("h")?)
    {
        return Some("image");
    }
    let k = obj.get("k")?;
    let keyframes = k
        .as_array()
        .filter(|a| !a.is_empty() && a.iter().all(Value::is_object));
    let Some(keyframes) = keyframes else {
        return value_kind(k).filter(|kind| *kind != "text");
    };
    let kinds: Option<Vec<&str>> = keyframes.iter().map(|f| value_kind(f.get("s")?)).collect();
    let kinds = kinds?;
    kinds.iter().all(|k| *k == kinds[0]).then_some(kinds[0])
}

fn slots(anim: &Map<String, Value>) -> Result<Vec<SlotSpec>, String> {
    let Some(slots) = anim.get("slots") else {
        return Ok(Vec::new());
    };
    let slots = slots.as_object().ok_or("slots is not an object")?;
    let mut out: Vec<SlotSpec> = slots
        .iter()
        .map(|(id, slot)| {
            let kind = slot.get("p").and_then(slot_kind).ok_or_else(|| {
                format!("slot {id:?} is not a text, color, image or number value")
            })?;
            Ok(SlotSpec {
                id: id.clone(),
                kind: kind.to_string(),
            })
        })
        .collect::<Result<_, String>>()?;
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// The `fFamily` of each `fonts.list` entry, in order, each once.
fn fonts(anim: &Map<String, Value>) -> Result<Vec<String>, String> {
    let Some(fonts) = anim.get("fonts") else {
        return Ok(Vec::new());
    };
    let list = match fonts.get("list") {
        None => return Ok(Vec::new()),
        Some(l) => l.as_array().ok_or("fonts.list is not an array")?,
    };
    let mut out: Vec<String> = Vec::new();
    for (i, f) in list.iter().enumerate() {
        let family = f
            .get("fFamily")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("fonts.list[{i}] has no fFamily"))?;
        if !out.iter().any(|o| o == family) {
            out.push(family.to_string());
        }
    }
    Ok(out)
}

/// Every `chars` entry is a glyph: `ch`, `fFamily` and `style` strings, finite `size` and
/// `w`, and `data` (when present) an object whose `shapes` (when present) are objects.
fn check_chars(anim: &Map<String, Value>) -> Result<(), String> {
    let Some(chars) = anim.get("chars") else {
        return Ok(());
    };
    let chars = chars.as_array().ok_or("malformed chars: not an array")?;
    for (i, c) in chars.iter().enumerate() {
        let text = |k: &str| c.get(k).is_some_and(Value::is_string);
        let num = |k: &str| c.get(k).is_some_and(is_num);
        let data_ok = match c.get("data") {
            None => true,
            Some(d) => d.as_object().is_some_and(|d| match d.get("shapes") {
                None => true,
                Some(s) => s.as_array().is_some_and(|s| s.iter().all(Value::is_object)),
            }),
        };
        let ch_ok = c
            .get("ch")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        if !(ch_ok && text("fFamily") && text("style") && num("size") && num("w") && data_ok) {
            return Err(format!("malformed chars[{i}]"));
        }
    }
    Ok(())
}

fn info(anim: &Value) -> Result<LottieInfo, String> {
    let anim = anim
        .as_object()
        .ok_or("the animation is not a JSON object")?;
    let (width, height) = (dim(anim, "w")?, dim(anim, "h")?);
    let rate = rate(number(anim, "fr")?).ok_or("animation fr must be between 0 and 1000")?;
    let (ip, op) = (number(anim, "ip")?, number(anim, "op")?);
    let duration = frames(op - ip)
        .filter(|n| n.is_positive())
        .and_then(|n| n.checked_div(rate).ok())
        .ok_or("animation op must be after ip")?;
    check_chars(anim)?;
    Ok(LottieInfo {
        width,
        height,
        rate,
        duration,
        slots: slots(anim)?,
        fonts: fonts(anim)?,
    })
}

pub(crate) fn probe(file: &mut File, path: &Path) -> Result<Probe, CoreError> {
    let parsed = (|| {
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("not a dotLottie archive: {e}"))?;
        let manifest = member(&mut archive, "manifest.json")?;
        let id = first_animation(&manifest)?;
        let anim = member(&mut archive, &format!("animations/{id}.json"))?;
        info(&anim)
    })();
    let lottie = parsed.map_err(|why| unsupported(path, &why))?;
    Ok(Probe {
        duration: Some(lottie.duration),
        lottie: Some(lottie),
        ..Probe::of(AssetKind::LottieTemplate)
    })
}
