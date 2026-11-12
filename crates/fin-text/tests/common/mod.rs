//! Shared by `measure.rs` and `corpus_real.rs`: the corpus case format, an in-memory
//! `FontBytes`, and the layout hash (BLAKE3 of compact JSON).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use fin_core::layout::{FaceRef, TextLayout, TextRequest, TextStyle};
use fin_text::FontBytes;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// One corpus case: `fonts` are keys into the corpus' font table, primary first.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub what: String,
    pub fonts: Vec<String>,
    pub words: Vec<String>,
    pub style: TextStyle,
    #[serde(default)]
    pub max_width: Option<f64>,
    #[serde(default)]
    pub max_lines: Option<u32>,
}

impl Case {
    pub fn request(&self, faces: Vec<FaceRef>) -> TextRequest {
        TextRequest {
            words: self.words.clone(),
            style: self.style.clone(),
            faces,
            max_width: self.max_width,
            max_lines: self.max_lines,
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The layout hash compared across machines.
pub fn layout_hash(layout: &TextLayout) -> String {
    blake3::hash(&serde_json::to_vec(layout).unwrap())
        .to_hex()
        .to_string()
}

/// Bytes by SHA-256, held in memory.
#[derive(Default)]
pub struct MapBytes(pub BTreeMap<String, Arc<[u8]>>);

impl MapBytes {
    /// Adds `bytes` and returns their SHA-256.
    pub fn add(&mut self, bytes: Vec<u8>) -> String {
        let sha = sha256_hex(&bytes);
        self.0.insert(sha.clone(), bytes.into());
        sha
    }
}

impl FontBytes for MapBytes {
    fn get(&self, sha256: &str) -> Option<Arc<[u8]>> {
        self.0.get(sha256).cloned()
    }
}

/// A face ref for face `face` of `bytes`, described by `font_info`.
pub fn face_ref(sha256: &str, bytes: &[u8], face: u32) -> FaceRef {
    let info = fin_text::font_info(bytes).unwrap().remove(face as usize);
    FaceRef {
        sha256: sha256.to_string(),
        face,
        family: info.family,
        weight: info.weight,
        italic: info.italic,
    }
}

/// The generated fonts by corpus name (`sans`, `rtl`, `han`, `latinBold`, `var`, `overhang`,
/// `ttc#0`, `ttc#1`) and a provider holding them.
pub fn generated_fonts() -> (MapBytes, BTreeMap<String, FaceRef>) {
    use fin_testfonts as tf;
    let mut bytes = MapBytes::default();
    let mut refs = BTreeMap::new();
    for (name, data) in [
        ("sans", tf::test_sans()),
        ("rtl", tf::test_rtl()),
        ("han", tf::test_han()),
        ("latinBold", tf::build(&tf::LATIN_BOLD)),
        ("var", tf::test_variable()),
        ("overhang", tf::test_overhang()),
    ] {
        let sha = bytes.add(data.clone());
        refs.insert(name.to_string(), face_ref(&sha, &data, 0));
    }
    let ttc = tf::collection(&[tf::test_sans(), tf::build(&tf::LATIN_BOLD)]);
    let sha = bytes.add(ttc.clone());
    for face in 0..2 {
        refs.insert(format!("ttc#{face}"), face_ref(&sha, &ttc, face));
    }
    (bytes, refs)
}

/// `expected.json`: `{ "generated": {id: hash}, "real": {id: hash} }`.
#[derive(Debug, Default, Deserialize, serde::Serialize)]
pub struct Expected {
    pub generated: BTreeMap<String, String>,
    pub real: BTreeMap<String, String>,
}

pub const EXPECTED: &str = include_str!("../corpus/expected.json");

pub fn expected() -> Expected {
    serde_json::from_str(EXPECTED).unwrap()
}

/// `FIN_TEXT_BLESS=1` rewrites the goldens instead of comparing them (native only).
pub fn blessing() -> bool {
    std::env::var("FIN_TEXT_BLESS").is_ok_and(|v| v == "1")
}

pub fn corpus_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

/// Writes one section of `expected.json` and the reviewed layouts under `layouts/`.
pub fn bless(section: &str, hashes: BTreeMap<String, String>, layouts: &[(String, TextLayout)]) {
    let dir = corpus_dir();
    let path = dir.join("expected.json");
    let mut all: Expected = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    match section {
        "generated" => all.generated = hashes,
        _ => all.real = hashes,
    }
    std::fs::write(&path, serde_json::to_string_pretty(&all).unwrap() + "\n").unwrap();
    for (id, layout) in layouts {
        let file = dir.join("layouts").join(format!("{section}-{id}.json"));
        std::fs::write(file, serde_json::to_string_pretty(layout).unwrap() + "\n").unwrap();
    }
}
