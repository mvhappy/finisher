//! `fin-text` over the real catalog fonts (4C Decision 39): Arabic joining, bidi, kerning,
//! CJK and emoji, by SHA-256 from a warmed cache.
//!
//! The cache is the directory `FIN_FONT_CACHE` names (files `<sha256>`, as filled by
//! `cargo run -p fin-fonts --example warm_cache`). Locally the test skips with a notice when
//! the cache, a font, a pin or a golden is absent; with `FIN_REQUIRE_FONTS=1` (CI and the
//! validation hosts) each of those fails instead. `FIN_TEXT_BLESS=1` rewrites the goldens.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use common::{Case, MapBytes, layout_hash, sha256_hex};
use fin_core::layout::{FaceRef, Measured, TextLayout, TextMeasurer};
use fin_text::Measurer;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RealFont {
    /// `null` until the catalog pins the file (Task 5).
    sha256: Option<String>,
    face: Option<u32>,
    family: String,
    weight: u16,
    italic: bool,
    /// A variable file: `weight` is the instance asked for, not `font_info`'s default.
    #[serde(default)]
    variable: bool,
}

#[derive(Deserialize)]
struct Real {
    fonts: BTreeMap<String, RealFont>,
    cases: Vec<Case>,
}

fn required() -> bool {
    std::env::var("FIN_REQUIRE_FONTS").is_ok_and(|v| v == "1")
}

/// Fails under `FIN_REQUIRE_FONTS=1`; otherwise prints the notice and returns `false`.
fn absent(why: String) -> bool {
    assert!(!required(), "FIN_REQUIRE_FONTS=1: {why}");
    eprintln!("corpus_real: skipped: {why}");
    false
}

fn load(real: &Real) -> Option<(MapBytes, BTreeMap<String, FaceRef>)> {
    let Some(dir) = std::env::var_os("FIN_FONT_CACHE").map(PathBuf::from) else {
        absent("FIN_FONT_CACHE is not set (warm it with `cargo run -p fin-fonts --example warm_cache`)".into());
        return None;
    };
    let unpinned: Vec<&str> = real
        .fonts
        .iter()
        .filter(|(_, f)| f.sha256.is_none() || f.face.is_none())
        .map(|(k, _)| k.as_str())
        .collect();
    if !unpinned.is_empty() {
        absent(format!("real.json does not pin {unpinned:?} yet"));
        return None;
    }
    let mut bytes = MapBytes::default();
    let mut refs = BTreeMap::new();
    for (key, font) in &real.fonts {
        let (sha, face) = (font.sha256.clone().unwrap(), font.face.unwrap());
        let path = dir.join(&sha);
        let Ok(data) = std::fs::read(&path) else {
            absent(format!("{key}: {} is not in the cache", path.display()));
            return None;
        };
        assert_eq!(
            sha256_hex(&data),
            sha,
            "{key}: the cache file's SHA-256 differs"
        );
        let info = fin_text::font_info(&data).unwrap().remove(face as usize);
        let weight = if font.variable {
            font.weight
        } else {
            info.weight
        };
        assert_eq!(
            (info.family.as_str(), weight, info.italic),
            (font.family.as_str(), font.weight, font.italic),
            "{key}: font_info differs from real.json"
        );
        refs.insert(
            key.clone(),
            FaceRef {
                sha256: sha.clone(),
                face,
                family: info.family,
                weight: font.weight,
                italic: info.italic,
            },
        );
        bytes.0.insert(sha, data.into());
    }
    Some((bytes, refs))
}

/// Facts each layout must show beyond its golden hash.
fn check_case(id: &str, l: &TextLayout, refs: &BTreeMap<String, FaceRef>) {
    let uses = |key: &str| l.faces.contains(&refs[key].sha256);
    match id {
        "arabic-joining" | "mixed-bidi" => assert!(uses("noto-sans-arabic:regular"), "{id}"),
        "hebrew" => assert!(uses("noto-sans-hebrew:regular") && l.lines[0].rtl, "{id}"),
        "chinese" | "japanese-kana" | "korean" => assert!(uses("noto-sans-cjk:regular"), "{id}"),
        "emoji" => assert!(uses("noto-color-emoji:regular"), "{id}"),
        "unassigned" => assert_eq!(l.missing, [0x378], "{id}"),
        "max-lines" => assert!(l.overflow, "{id}"),
        "wrap" => assert!(l.lines.len() > 1 && !l.overflow, "{id}"),
        _ => {}
    }
    if id != "unassigned" {
        assert!(l.missing.is_empty(), "{id}: missing {:?}", l.missing);
    }
}

#[test]
fn real_corpus_matches_expected() {
    let real: Real = serde_json::from_str(include_str!("corpus/real.json")).unwrap();
    let Some((bytes, refs)) = load(&real) else {
        return;
    };
    let m = Measurer::new(Arc::new(bytes));
    let mut layouts = Vec::new();
    for case in &real.cases {
        let faces = case.fonts.iter().map(|k| refs[k].clone()).collect();
        let Measured::Layout(l) = m.measure(&case.request(faces)).unwrap() else {
            panic!("{}: every face is in the cache", case.id)
        };
        check_case(&case.id, &l, &refs);
        layouts.push((case.id.clone(), l));
    }
    // The variable Inter at wght 700 is wider than at 400 (the axis is applied).
    let width = |id: &str| layouts.iter().find(|(c, _)| c == id).map(|(_, l)| l.width);
    assert!(
        width("inter-bold") > width("latin-kerning"),
        "bold is wider"
    );
    let hashes: BTreeMap<String, String> = layouts
        .iter()
        .map(|(id, l)| (id.clone(), layout_hash(l)))
        .collect();
    if common::blessing() {
        common::bless("real", hashes, &layouts);
        return;
    }
    let expected = common::expected().real;
    for (id, hash) in &hashes {
        match expected.get(id) {
            Some(want) => assert_eq!(hash, want, "{id}: layout hash"),
            None => {
                absent(format!(
                    "{id} has no golden yet (FIN_TEXT_BLESS=1 after review)"
                ));
            }
        }
    }
}
