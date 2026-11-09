//! dotLottie templates (`.lottie`, a zip): `manifest.json` names the animations; the first
//! one gives the canvas (`w`, `h`), the rate (`fr`), the frames
//! (`ip`..`op`), the slots (`slots`, by value kind) and the fonts (`fonts.list`). Malformed
//! JSON, slots or `chars` (glyph shapes thorvg crashes on, arch §27.10) are
//! `UNSUPPORTED_CONTAINER` with `details.reason`.
//!
//! Memory is bounded by the archive's bytes, not by what they expand to: a member is read up
//! to its own limit and at most [`RATIO_LIMIT`] times its compressed size, and the animation
//! is never parsed whole into a JSON tree (which takes ~16 times the text). Only the keys the
//! probe needs are kept, as raw text; the rest (`layers`, `assets`, …) are skipped as they are
//! read. `slots`, `fonts` and each `chars` glyph become trees only up to [`TREE_LIMIT`].

use std::io::{Read, Seek};
use std::path::Path;

use fin_core::{AssetKind, CoreError, LottieInfo, Rational, SlotSpec};
use serde::Deserialize;
use serde::de::{Deserializer as _, Error as _, SeqAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::{Probe, unsupported};

const MIB: u64 = 1024 * 1024;
/// `manifest.json`, decompressed, at most.
const MANIFEST_LIMIT: u64 = MIB;
/// `animations/<id>.json`, decompressed, at most.
const ANIMATION_LIMIT: u64 = 32 * MIB;
/// A member past [`MANIFEST_LIMIT`] expands at most this many times its compressed size: JSON
/// deflates ~5–20×, a bomb ~1000×.
const RATIO_LIMIT: u64 = 100;
/// The longest `slots`, `fonts` or `chars` glyph text parsed into a tree.
const TREE_LIMIT: usize = MIB as usize;

/// A member's bytes, refused past `limit` or [`RATIO_LIMIT`] times its compressed size
/// (`file_size` bounds a declared compressed size).
fn member<R: Read + Seek>(
    archive: &mut zip::ZipArchive<&mut R>,
    file_size: u64,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, String> {
    let entry = archive
        .by_name(name)
        .map_err(|e| format!("dotLottie member {name}: {e}"))?;
    let by_ratio = entry
        .compressed_size()
        .min(file_size)
        .saturating_mul(RATIO_LIMIT)
        .max(MANIFEST_LIMIT);
    let cap = limit.min(by_ratio);
    let mut bytes = Vec::new();
    entry
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("dotLottie member {name}: {e}"))?;
    if bytes.len() as u64 > cap {
        return Err(if cap == limit {
            format!("dotLottie member {name} is larger than {} MiB", limit / MIB)
        } else {
            format!("dotLottie member {name} expands more than {RATIO_LIMIT} times")
        });
    }
    Ok(bytes)
}

/// The archive directory of animations by manifest `version`: dotLottie 1 (`"1"`, `"1.0"`, or
/// no version, as early writers left it out) keeps them in `animations/`, dotLottie 2
/// (<https://dotlottie.io/spec/2.0/>, `"2"`) in `a/`. Its themes, state machines and other
/// members are not read. Any other version is refused rather than guessed at.
fn animation_dir(manifest: &Value) -> Result<&'static str, String> {
    match manifest.get("version") {
        None => Ok("animations"),
        Some(Value::String(v)) => match v.as_str() {
            "1" | "1.0" => Ok("animations"),
            "2" | "2.0" => Ok("a"),
            other => Err(format!(
                "unsupported dotLottie version {other:?} (versions 1 and 2 are read)"
            )),
        },
        Some(other) => Err(format!(
            "unsupported dotLottie version {other} (versions 1 and 2 are read)"
        )),
    }
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

/// The keys of an animation the probe reads, as raw JSON text; every other key is skipped
/// without being kept.
#[derive(Deserialize)]
struct Animation<'a> {
    #[serde(borrow, default)]
    w: Option<&'a RawValue>,
    #[serde(borrow, default)]
    h: Option<&'a RawValue>,
    #[serde(borrow, default)]
    fr: Option<&'a RawValue>,
    #[serde(borrow, default)]
    ip: Option<&'a RawValue>,
    #[serde(borrow, default)]
    op: Option<&'a RawValue>,
    #[serde(borrow, default)]
    layers: Option<&'a RawValue>,
    #[serde(borrow, default)]
    slots: Option<&'a RawValue>,
    #[serde(borrow, default)]
    fonts: Option<&'a RawValue>,
    #[serde(borrow, default)]
    chars: Option<&'a RawValue>,
}

fn number(raw: Option<&RawValue>, key: &str) -> Result<f64, String> {
    raw.and_then(|r| serde_json::from_str::<f64>(r.get()).ok())
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("animation has no numeric {key}"))
}

/// `raw` as a JSON tree, refused past [`TREE_LIMIT`].
fn tree(raw: Option<&RawValue>, key: &str) -> Result<Option<Value>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.get().len() > TREE_LIMIT {
        return Err(format!(
            "{key} is larger than {} MiB",
            TREE_LIMIT as u64 / MIB
        ));
    }
    serde_json::from_str(raw.get()).map_err(|e| format!("malformed {key}: {e}"))
}

/// An exact rate for `fr`: whole numbers as they are, NTSC rates as `n×1000/1001`
/// ([`fin_core::ntsc_rate`]), others to a thousandth.
pub(crate) fn rate(fr: f64) -> Option<Rational> {
    if !(fr > 0.0 && fr <= 1000.0) {
        return None;
    }
    if (fr - fr.round()).abs() < 1e-6 {
        return Rational::new(fr.round() as i64, 1).ok();
    }
    if let Some(ntsc) = fin_core::ntsc_rate(fr) {
        return Some(ntsc);
    }
    Rational::new((fr * 1000.0).round() as i64, 1000).ok()
}

/// A frame count to a thousandth (whole counts exactly).
fn frames(n: f64) -> Option<Rational> {
    Rational::new((n * 1000.0).round() as i64, 1000).ok()
}

fn dim(raw: Option<&RawValue>, key: &str) -> Result<u32, String> {
    let v = number(raw, key)?;
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

fn slots(raw: Option<&RawValue>) -> Result<Vec<SlotSpec>, String> {
    let Some(slots) = tree(raw, "slots")? else {
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
fn fonts(raw: Option<&RawValue>) -> Result<Vec<String>, String> {
    let Some(fonts) = tree(raw, "fonts")? else {
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

/// A glyph: `ch`, `fFamily` and `style` strings, finite `size` and `w`, and `data` (when
/// present) an object whose `shapes` (when present) are objects.
fn check_char(raw: &RawValue, i: usize) -> Result<(), String> {
    let bad = || format!("malformed chars[{i}]");
    if raw.get().len() > TREE_LIMIT {
        return Err(bad());
    }
    let c: Value = serde_json::from_str(raw.get()).map_err(|_| bad())?;
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
        return Err(bad());
    }
    Ok(())
}

/// Checks each `chars` glyph as it is read, keeping none; the first failure's reason.
struct Glyphs<'r>(&'r mut Option<String>);

impl<'de> Visitor<'de> for Glyphs<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an array of glyphs")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut i = 0;
        while let Some(raw) = seq.next_element::<&'de RawValue>()? {
            if let Err(why) = check_char(raw, i) {
                *self.0 = Some(why);
                return Err(A::Error::custom("malformed glyph"));
            }
            i += 1;
        }
        Ok(())
    }
}

/// Every `chars` entry is a glyph ([`check_char`]).
fn check_chars(raw: Option<&RawValue>) -> Result<(), String> {
    let Some(raw) = raw else {
        return Ok(());
    };
    let mut why = None;
    let mut de = serde_json::Deserializer::from_str(raw.get());
    match de.deserialize_seq(Glyphs(&mut why)) {
        Ok(()) => Ok(()),
        Err(_) => Err(why.unwrap_or_else(|| "malformed chars: not an array".into())),
    }
}

fn info(bytes: &[u8], name: &str) -> Result<LottieInfo, String> {
    // A derived `Deserialize` also reads a JSON array positionally, so an array such as
    // `[64,36,30,0,30,null,…]` would pass for an animation: require the object first.
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Err(format!(
            "malformed JSON in {name}: the animation is not a JSON object"
        ));
    }
    let anim: Animation<'_> =
        serde_json::from_slice(bytes).map_err(|e| format!("malformed JSON in {name}: {e}"))?;
    let (width, height) = (dim(anim.w, "w")?, dim(anim.h, "h")?);
    let rate = rate(number(anim.fr, "fr")?).ok_or("animation fr must be between 0 and 1000")?;
    let (ip, op) = (number(anim.ip, "ip")?, number(anim.op, "op")?);
    let duration = frames(op - ip)
        .filter(|n| n.is_positive())
        .and_then(|n| n.checked_div(rate).ok())
        .ok_or("animation op must be after ip")?;
    let layers = anim
        .layers
        .is_some_and(|l| l.get().trim_start().starts_with('['));
    if !layers {
        return Err("animation layers must be an array (a Lottie animation has `layers`)".into());
    }
    check_chars(anim.chars)?;
    Ok(LottieInfo {
        width,
        height,
        rate,
        duration,
        slots: slots(anim.slots)?,
        fonts: fonts(anim.fonts)?,
    })
}

pub(crate) fn probe<R: Read + Seek>(
    file: &mut R,
    size: u64,
    path: &Path,
) -> Result<Probe, CoreError> {
    let parsed = (|| {
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("not a dotLottie archive: {e}"))?;
        let manifest = member(&mut archive, size, "manifest.json", MANIFEST_LIMIT)?;
        let manifest: Value = serde_json::from_slice(&manifest)
            .map_err(|e| format!("malformed JSON in manifest.json: {e}"))?;
        let dir = animation_dir(&manifest)?;
        let id = first_animation(&manifest)?;
        let name = format!("{dir}/{id}.json");
        let anim = member(&mut archive, size, &name, ANIMATION_LIMIT)?;
        info(&anim, &name)
    })();
    let lottie = parsed.map_err(|why| unsupported(path, &why))?;
    Ok(Probe {
        duration: Some(lottie.duration),
        lottie: Some(lottie),
        ..Probe::of(AssetKind::LottieTemplate)
    })
}
