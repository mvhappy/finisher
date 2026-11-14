//! The media cache's pure parts (sub-project 6 spec §7.2; 6B-1 plan Decisions 10–12): recipes,
//! the cache key, an entry's `meta.json` and the wire types of the cache's status and `gc`.
//! The index, the moves into place and eviction are `fin-node`'s (`src/media/cache/`); the
//! worker and the host compute the key here, so they agree (`PLAN_MISMATCH {reason: cacheKey}`
//! otherwise).
//!
//! - **Recipes** are `{name, version}`. A name is `[a-z0-9-]{1,32}`; a new recipe (a phone-sized
//!   proxy tier for offline downloads, `docs/specs/2026-11-08-mobile-offline-editing-decisions.md`)
//!   is one more constant here and needs nothing else from the cache: the key, the index and
//!   eviction take any valid name. Bumping a version invalidates that recipe's entries by key.
//! - **The key** ([`CacheKey::compute`]) is BLAKE3 over the canonical JSON ([`canonical_json`])
//!   of `{fingerprint, recipe, recipeVersion, params, renderVersion, decoderClass}`: object keys
//!   sorted by their UTF-8 bytes at every level, no whitespace, strings and numbers as
//!   `serde_json` writes them. Params should hold strings, integers and booleans: a float is
//!   hashed in `serde_json`'s shortest form, so `1.0` and `1` are different keys.
//! - **`meta.json`** ([`EntryMeta`]) carries the key input in full, so the host recomputes the
//!   key of any directory it is handed or finds ([`EntryMeta::check`]), and the list of the
//!   entry's files with their sizes and BLAKE3.

use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::RENDER_VERSION;
use crate::backend::VendorClass;
use crate::info::MediaInfo;
use crate::jobs::ToneMapped;

/// A cache recipe: what an entry holds and how it was made (spec §7.2's table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Recipe {
    pub name: &'static str,
    pub version: u32,
}

impl Recipe {
    /// `[a-z0-9-]{1,32}`, starting with a letter.
    pub fn valid_name(name: &str) -> bool {
        (1..=32).contains(&name.len())
            && name.as_bytes()[0].is_ascii_lowercase()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }
}

/// The scrub tier (Decision 6: fits 960 × 540). v1 is video only (Decision 4; 6C's Opus audio
/// is v2).
pub const PROXY_540: Recipe = Recipe {
    name: "proxy-540",
    version: 1,
};
/// The default preview tier (fits 1920 × 1080). v1 is video only, as `proxy-540`.
pub const PROXY_1080: Recipe = Recipe {
    name: "proxy-1080",
    version: 1,
};
/// Thumbnails at requested times (Decision 17).
pub const THUMBS: Recipe = Recipe {
    name: "thumbs",
    version: 1,
};
/// Filmstrip sprites with their `strip.json` index (Decision 17).
pub const STRIP: Recipe = Recipe {
    name: "strip",
    version: 1,
};

/// 48 kHz `f32` in the source's canonical layout (6C plan Decision 7; `jobs::pcm`).
pub const PCM48: Recipe = Recipe {
    name: "pcm48",
    version: 1,
};
/// 16 kHz `f32` mono, the analysis feed (6C plan Decision 7; `jobs::pcm`).
pub const PCM16M: Recipe = Recipe {
    name: "pcm16m",
    version: 1,
};

/// Every recipe this build makes.
pub const RECIPES: &[Recipe] = &[PROXY_540, PROXY_1080, THUMBS, STRIP, PCM48, PCM16M];

/// The recipe of this build named `name`.
pub fn recipe(name: &str) -> Option<Recipe> {
    RECIPES.iter().copied().find(|r| r.name == name)
}

/// What a cache key is computed from (Decision 11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyInput {
    /// The source's fingerprint (the asset's `fingerprint`).
    pub fingerprint: String,
    pub recipe: String,
    pub recipe_version: u32,
    /// The recipe's parameters (a tier, a codec, thumbnail times, …).
    pub params: Value,
    pub render_version: u32,
    /// The vendor class of the backend that decoded the source.
    pub decoder_class: VendorClass,
}

impl KeyInput {
    /// The input for `recipe` of this build, at this build's [`RENDER_VERSION`].
    pub fn new(
        fingerprint: impl Into<String>,
        recipe: Recipe,
        params: Value,
        decoder_class: VendorClass,
    ) -> KeyInput {
        KeyInput {
            fingerprint: fingerprint.into(),
            recipe: recipe.name.to_string(),
            recipe_version: recipe.version,
            params,
            render_version: RENDER_VERSION,
            decoder_class,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "fingerprint": self.fingerprint,
            "recipe": self.recipe,
            "recipeVersion": self.recipe_version,
            "params": self.params,
            "renderVersion": self.render_version,
            "decoderClass": self.decoder_class,
        })
    }
}

/// A cache key: 64 lower-case hex digits (BLAKE3). An entry lives at `<key[0..2]>/<key>/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey(String);

impl CacheKey {
    /// BLAKE3 over the canonical JSON of `input` (module docs).
    pub fn compute(input: &KeyInput) -> CacheKey {
        let text = canonical_json(&input.to_json());
        CacheKey(blake3::hash(text.as_bytes()).to_hex().to_string())
    }

    /// `s` when it is 64 lower-case hex digits (so it is also a safe directory name).
    pub fn parse(s: &str) -> Result<CacheKey, CoreError> {
        if s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            Ok(CacheKey(s.to_string()))
        } else {
            Err(
                CoreError::new("PLAN_MISMATCH", "a cache key is 64 lower-case hex digits")
                    .with_details(json!({ "reason": "cacheKey" })),
            )
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first two digits: the entry's shard directory.
    pub fn shard(&self) -> &str {
        &self.0[..2]
    }
}

impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `value` as canonical JSON (module docs).
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(v, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let sorted: BTreeMap<&str, &Value> = map.iter().map(|(k, v)| (k.as_str(), v)).collect();
            out.push('{');
            for (i, (k, v)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.to_string()).to_string());
                out.push(':');
                write_canonical(v, out);
            }
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// The encoder that made an entry (proxies), for diagnostics and the export rule that a
/// `gpu`-encoded file never leaves the cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntryEncoder {
    pub backend: String,
    pub class: VendorClass,
}

/// One payload file of an entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntryFile {
    /// `[A-Za-z0-9_][A-Za-z0-9._-]{0,63}`, not `meta.json`.
    pub name: String,
    pub bytes: u64,
    /// BLAKE3 of the file, 64 lower-case hex digits.
    pub blake3: String,
}

/// The most files one entry may list (a filmstrip of a long source has one sprite per 10 s).
pub const MAX_ENTRY_FILES: usize = 10_000;

/// An entry's `meta.json` (spec §7.2): the key and its input, the files and, for proxies, the
/// original's `MediaInfo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntryMeta {
    pub key: String,
    pub fingerprint: String,
    pub recipe: String,
    pub recipe_version: u32,
    pub params: Value,
    pub render_version: u32,
    pub decoder_class: VendorClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<EntryEncoder>,
    pub files: Vec<EntryFile>,
    /// The original's `MediaInfo` (proxies: rotation and pixel aspect are applied from it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaInfo>,
    /// How an HDR source was brought to SDR (proxies, plan 6B-1 Decision 9); absent for SDR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ToneMapped")]
    pub tone_mapped: Option<ToneMapped>,
    /// Unix milliseconds.
    pub created_at: i64,
}

fn entry_invalid(message: impl Into<String>, file: Option<&str>) -> CoreError {
    let mut details = json!({ "reason": "cacheEntry" });
    if let Some(f) = file {
        details["file"] = json!(f);
    }
    CoreError::new("PLAN_MISMATCH", message).with_details(details)
}

/// A payload file name an entry may list (module docs of [`EntryFile`]).
pub fn valid_file_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=64).contains(&b.len())
        && (b[0].is_ascii_alphanumeric() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        && name != META_FILE
}

/// The name of an entry's metadata file.
pub const META_FILE: &str = "meta.json";

impl EntryMeta {
    /// The input the key is computed from.
    pub fn key_input(&self) -> KeyInput {
        KeyInput {
            fingerprint: self.fingerprint.clone(),
            recipe: self.recipe.clone(),
            recipe_version: self.recipe_version,
            params: self.params.clone(),
            render_version: self.render_version,
            decoder_class: self.decoder_class,
        }
    }

    /// The payload's size (the files listed; `meta.json` not included);
    /// `PLAN_MISMATCH {reason: cacheEntry}` when the sizes overflow a `u64`.
    pub fn bytes(&self) -> Result<u64, CoreError> {
        self.files
            .iter()
            .try_fold(0u64, |n, f| n.checked_add(f.bytes))
            .ok_or_else(|| entry_invalid("the files' sizes overflow", None))
    }

    /// The checks a commit and an adoption make before anything else: the key parses and
    /// recomputes from the input (`PLAN_MISMATCH {reason: cacheKey}`), the recipe name and the
    /// fingerprint are well formed, and the files are a list of distinct valid names with
    /// BLAKE3 digests (`PLAN_MISMATCH {reason: cacheEntry, file?}`).
    pub fn check(&self) -> Result<CacheKey, CoreError> {
        let key = CacheKey::parse(&self.key)?;
        if CacheKey::compute(&self.key_input()) != key {
            return Err(CoreError::new(
                "PLAN_MISMATCH",
                "the entry's key does not recompute from its meta.json",
            )
            .with_details(json!({ "reason": "cacheKey", "key": self.key })));
        }
        if !Recipe::valid_name(&self.recipe) {
            return Err(entry_invalid(
                "the recipe name is not [a-z0-9-]{1,32}",
                None,
            ));
        }
        if self.fingerprint.is_empty() || self.fingerprint.len() > 256 {
            return Err(entry_invalid("the fingerprint is empty or too long", None));
        }
        self.bytes()?;
        if self.files.len() > MAX_ENTRY_FILES {
            return Err(entry_invalid(
                format!("an entry lists at most {MAX_ENTRY_FILES} files"),
                None,
            ));
        }
        let mut seen = BTreeSet::new();
        for f in &self.files {
            if !valid_file_name(&f.name) {
                return Err(entry_invalid(
                    "a file name the cache refuses",
                    Some(&f.name),
                ));
            }
            if !seen.insert(f.name.as_str()) {
                return Err(entry_invalid("a file is listed twice", Some(&f.name)));
            }
            let hex = f.blake3.len() == 64
                && f.blake3
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            if !hex {
                return Err(entry_invalid(
                    "a file's blake3 is not 64 hex digits",
                    Some(&f.name),
                ));
            }
        }
        Ok(key)
    }
}

/// One recipe's share of the cache.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecipeUsage {
    pub bytes: u64,
    pub entries: u64,
}

/// `media.cache.status` (`fin cache status`): what the cache holds against its cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CacheStatus {
    /// Every entry's files and `meta.json`.
    pub bytes: u64,
    pub entries: u64,
    /// Eviction keeps `bytes` at or below this (Decision 12).
    pub cap_bytes: u64,
    /// Entries with at least one pin (never evicted).
    pub pinned: u64,
    pub by_recipe: BTreeMap<String, RecipeUsage>,
}

/// What one `gc` (or eviction pass) did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CacheGcResult {
    pub evicted: u64,
    pub freed_bytes: u64,
    /// What the cache holds afterwards (above the target only when the rest is pinned).
    pub bytes: u64,
}
