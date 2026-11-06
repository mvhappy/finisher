//! The project's font pins (plan 4C Decision 29): every face a caption measures with names an
//! exact file by SHA-256, so every machine measures with identical bytes. Pins are plain data:
//! applying or replaying an edit never fetches and never reads font bytes.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A font file's container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FontFormat {
    Ttf,
    Otf,
    Ttc,
}

/// The pins, keyed by [`font_key`], and the fallback order (pin keys, first tried first).
/// Empty until a caption edit pins a face; omitted from the project JSON while empty.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Fonts {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pins: BTreeMap<String, FontPin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<String>,
}

impl Fonts {
    pub fn is_empty(&self) -> bool {
        self.pins.is_empty() && self.fallback.is_empty()
    }
}

/// One face of one exact file: `face` is the index inside a collection (0 for a single font).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontPin {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub face: u32,
    /// The whole file's SHA-256, 64 lowercase hex digits.
    pub sha256: String,
    /// The file's size in bytes.
    pub size: u64,
    pub format: FontFormat,
    /// ISO 15924 script codes the face covers (as `fin_text::font_info` reports them).
    pub scripts: Vec<String>,
    pub source: FontSource,
}

/// Where a pinned file comes from: a catalog entry's style (at the catalog version that
/// pinned it), or an imported font asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FontSource {
    Catalog {
        entry: String,
        style: String,
        catalog: String,
    },
    Asset {
        asset: String,
    },
}

/// The pin key of a face: `"<family>|<weight>|<normal|italic>"`, the family as written.
pub fn font_key(family: &str, weight: u16, italic: bool) -> String {
    let slope = if italic { "italic" } else { "normal" };
    format!("{family}|{weight}|{slope}")
}
