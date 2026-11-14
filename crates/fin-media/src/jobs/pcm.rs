//! `media.pcm` v1 (plan 6C Task 18, Decisions 7, 8, 20 and 23; sub-project 6 spec §7.2): a
//! source's audio as a content-addressed PCM feed in the media cache, made by `fin-workers media`
//! and committed by the host.
//!
//! - **Recipes** (Decision 7): `pcm48` v1, 48 kHz `f32` in the source's canonical layout (one
//!   channel for a mono source, else two: a mono source keeps its pan law), the mixer's input;
//!   `pcm16m` v1, 16 kHz `f32` mono (the canonical form folded), sub-project 8's analysis feed.
//! - **Chunks**: `chunk-NNNNNN.f32`, interleaved little-endian, 1 MiB each ([`CHUNK_BYTES`]:
//!   131,072 stereo or 262,144 mono frames), the last one short; an entry lists them in order.
//!   The entry's frame count is the sum of its chunks ([`PcmEntry::of`]).
//! - **Key** (Decision 7): `{fingerprint, recipe, recipeVersion, params {audioVersion, rate,
//!   layout}, renderVersion, decoderClass}`. `audioVersion` ([`crate::AUDIO_VERSION`]) moves
//!   every audio key without touching video ones; `layout` is what the host can know before a
//!   decode (the probe's channel count: 1 is mono, else stereo; always mono for `pcm16m`), so
//!   both sides compute the same key. The frame count is not a key input (the host cannot know
//!   it exactly before the decode): it is derived from the listed chunks (**spec vs code**:
//!   Decision 7 lists `frames` among the params).
//! - **Decoder class** (Decision 8): of the backend that decodes the source; the host plans it
//!   from the source's audio codec with [`crate::backend_audio::AUDIO_BACKENDS`] ([`pcm_plan_key`]),
//!   and a source the probe gives no codec for (WAV, which `fin-probe` reads without a
//!   `MediaInfo`) is planned as symphonia's. The worker recomputes the key from the decoder it
//!   opened (`PLAN_MISMATCH {reason: cacheKey}` when they differ).
//! - **Retries restart from sample 0** (Decision 23): the kind is a batch of one item (the
//!   fingerprint, so the `pcm16m` library stage can run it), answered only once every chunk and
//!   `meta.json` are written; it reports progress and writes no checkpoint.

use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::invalid;
use super::proxy::ProxyLibraryFile;
use crate::AUDIO_VERSION;
use crate::backend::VendorClass;
use crate::backend_audio::{AUDIO_BACKENDS, AudioBackendEntry, audio_backend};
use crate::cache::{CacheKey, EntryMeta, KeyInput, PCM16M, PCM48, Recipe};
use crate::format::AudioCodec;

/// The size of every chunk but the last.
pub const CHUNK_BYTES: u64 = 1 << 20;
/// The mixer's rate.
pub const PCM48_RATE: u32 = 48_000;
/// The analysis feed's rate.
pub const PCM16M_RATE: u32 = 16_000;

/// `chunk-NNNNNN.f32`, the name of chunk `i`.
pub fn chunk_name(i: usize) -> String {
    format!("chunk-{i:06}.f32")
}

/// Which PCM feed (Decision 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum PcmRecipe {
    /// 48 kHz in the canonical layout: the mixer's input.
    #[serde(rename = "pcm48")]
    Pcm48,
    /// 16 kHz mono: the analysis feed.
    #[serde(rename = "pcm16m")]
    Pcm16m,
}

impl PcmRecipe {
    pub fn recipe(self) -> Recipe {
        match self {
            PcmRecipe::Pcm48 => PCM48,
            PcmRecipe::Pcm16m => PCM16M,
        }
    }

    pub fn rate(self) -> u32 {
        match self {
            PcmRecipe::Pcm48 => PCM48_RATE,
            PcmRecipe::Pcm16m => PCM16M_RATE,
        }
    }

    /// The feed's layout for a source of `channels` channels.
    pub fn layout(self, channels: u16) -> PcmLayout {
        match self {
            PcmRecipe::Pcm16m => PcmLayout::Mono,
            PcmRecipe::Pcm48 if channels == 1 => PcmLayout::Mono,
            PcmRecipe::Pcm48 => PcmLayout::Stereo,
        }
    }

    /// The recipe named `name` (`pcm48`, `pcm16m`).
    pub fn of_name(name: &str) -> Option<PcmRecipe> {
        [PcmRecipe::Pcm48, PcmRecipe::Pcm16m]
            .into_iter()
            .find(|r| r.recipe().name == name)
    }
}

/// A feed's channels: one, or two interleaved `L R`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PcmLayout {
    Mono,
    Stereo,
}

impl PcmLayout {
    pub fn channels(self) -> usize {
        match self {
            PcmLayout::Mono => 1,
            PcmLayout::Stereo => 2,
        }
    }

    /// Frames in a full chunk: 262,144 mono, 131,072 stereo.
    pub fn chunk_frames(self) -> u64 {
        CHUNK_BYTES / (4 * self.channels() as u64)
    }
}

/// The key input of a PCM feed (module docs).
pub fn pcm_key_input(
    fingerprint: &str,
    recipe: PcmRecipe,
    layout: PcmLayout,
    decoder_class: VendorClass,
) -> KeyInput {
    KeyInput::new(
        fingerprint,
        recipe.recipe(),
        json!({ "audioVersion": AUDIO_VERSION, "rate": recipe.rate(), "layout": layout }),
        decoder_class,
    )
}

/// The cache key of a PCM feed.
pub fn pcm_key(
    fingerprint: &str,
    recipe: PcmRecipe,
    layout: PcmLayout,
    decoder_class: VendorClass,
) -> CacheKey {
    CacheKey::compute(&pcm_key_input(fingerprint, recipe, layout, decoder_class))
}

/// The audio backend this build decodes `codec` with: the first of
/// [`AUDIO_BACKENDS`] that lists it; a source without a codec (module docs) is symphonia's.
pub fn audio_decoder_for(codec: Option<AudioCodec>) -> Option<&'static AudioBackendEntry> {
    match codec {
        None => audio_backend("symphonia"),
        Some(c) => AUDIO_BACKENDS.iter().find(|b| b.decode.contains(&c)),
    }
}

/// What the host plans before it submits (`mediaPcmPlan`): the key, the decoder and its class,
/// and the layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmPlan {
    pub key: CacheKey,
    pub decoder: &'static str,
    pub decoder_class: VendorClass,
    pub layout: PcmLayout,
}

/// The host's half of the key: `CODEC_UNAVAILABLE {direction: decode}` when no backend of this
/// build decodes `codec`, `ASSET_TYPE_UNSUPPORTED` for a source with no audio channel.
pub fn pcm_plan_key(
    fingerprint: &str,
    recipe: PcmRecipe,
    codec: Option<AudioCodec>,
    channels: u16,
) -> Result<PcmPlan, CoreError> {
    if channels == 0 {
        return Err(
            CoreError::new("ASSET_TYPE_UNSUPPORTED", "the source has no audio channel")
                .with_details(json!({ "reason": "noAudio" }))
                .with_fix("make PCM feeds of assets with an audio track"),
        );
    }
    let Some(b) = audio_decoder_for(codec) else {
        let name = serde_json::to_value(codec).unwrap_or_default();
        return Err(CoreError::new(
            "CODEC_UNAVAILABLE",
            format!(
                "no decoder on this machine reads {} audio",
                name.as_str().unwrap_or("this")
            ),
        )
        .with_details(json!({ "codec": name, "direction": "decode" }))
        .with_fix(
            "see `fin media caps` for what this machine decodes; the codec plugin adds more",
        ));
    };
    let layout = recipe.layout(channels);
    Ok(PcmPlan {
        key: pcm_key(fingerprint, recipe, layout, b.class),
        decoder: b.name,
        decoder_class: b.class,
        layout,
    })
}

/// `media.pcm` v1: the `recipe` feed of the source whose content is `fingerprint`, which the
/// cache knows as `key`. A batch kind of one item, the fingerprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PcmParams {
    pub fingerprint: String,
    pub recipe: PcmRecipe,
    /// The layout the host planned (part of the key).
    pub layout: PcmLayout,
    /// The vendor class the host expects the source's decoder to have (part of the key).
    pub decoder_class: VendorClass,
    /// [`pcm_key`] of the fields above: the worker writes `out/<key>/`.
    pub key: String,
    /// A project asset, through the job's `in/manifest.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// A library file (the `pcm16m` stage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ProxyLibraryFile")]
    pub file: Option<ProxyLibraryFile>,
    /// The project that asked (host only, as `ProxyParams`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The branch whose head names the asset (`main` when absent). Host only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl PcmParams {
    /// The key input for a source decoded by a backend of `decoder_class` into `layout`.
    pub fn key_input(&self, layout: PcmLayout, decoder_class: VendorClass) -> KeyInput {
        pcm_key_input(&self.fingerprint, self.recipe, layout, decoder_class)
    }

    /// Exactly one source, a well-formed key, a layout the recipe can have.
    pub fn check(&self) -> Result<(), CoreError> {
        match (&self.asset, &self.file) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(invalid(
                    "asset",
                    "name exactly one source: asset or file".into(),
                ));
            }
            _ => {}
        }
        if self.fingerprint.is_empty() || self.fingerprint.len() > 256 {
            return Err(invalid("fingerprint", "1 to 256 characters".into()));
        }
        if self.recipe == PcmRecipe::Pcm16m && self.layout != PcmLayout::Mono {
            return Err(invalid("layout", "pcm16m is mono".into()));
        }
        CacheKey::parse(&self.key)
            .map_err(|_| invalid("key", "64 lower-case hex digits".into()))?;
        Ok(())
    }
}

/// The backend that decoded the source, and its class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PcmDecoder {
    pub backend: String,
    pub class: VendorClass,
}

/// What `media.pcm` v1 answers for its item (the fingerprint).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PcmResult {
    pub key: String,
    pub recipe: PcmRecipe,
    pub layout: PcmLayout,
    /// Frames at the feed's rate.
    pub frames: u64,
    pub chunks: u32,
    pub decoder: PcmDecoder,
    pub source_rate: u32,
    pub source_channels: u16,
    /// The entry's bytes (its chunks, `meta.json` not included).
    pub bytes: u64,
}

/// A PCM entry's shape, read from its `meta.json` (after [`EntryMeta::check`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmEntry {
    pub recipe: PcmRecipe,
    pub layout: PcmLayout,
    pub rate: u32,
    pub frames: u64,
    /// Each chunk's bytes, in order.
    pub chunks: Vec<u64>,
}

fn not_pcm(message: &str) -> CoreError {
    CoreError::new("PLAN_MISMATCH", format!("not a PCM entry: {message}"))
        .with_details(json!({ "reason": "cacheEntry" }))
}

impl PcmEntry {
    /// The entry `meta` describes, when it is a PCM feed of this build: the recipe and its
    /// version, `params {audioVersion, rate, layout}`, and the files `chunk-000000.f32`, … in
    /// order, every one [`CHUNK_BYTES`] but the last, which is a non-empty whole number of
    /// frames. `PLAN_MISMATCH {reason: cacheEntry}` otherwise.
    pub fn of(meta: &EntryMeta) -> Result<PcmEntry, CoreError> {
        let recipe = PcmRecipe::of_name(&meta.recipe).ok_or_else(|| not_pcm("recipe"))?;
        if meta.recipe_version != recipe.recipe().version {
            return Err(not_pcm("recipe version"));
        }
        let layout: PcmLayout = meta
            .params
            .get("layout")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .ok_or_else(|| not_pcm("layout"))?;
        let expected = pcm_key_input(&meta.fingerprint, recipe, layout, meta.decoder_class).params;
        if meta.params != expected {
            return Err(not_pcm("params (another audio version or rate)"));
        }
        let frame = 4 * layout.channels() as u64;
        let mut chunks = Vec::with_capacity(meta.files.len());
        for (i, f) in meta.files.iter().enumerate() {
            let last = i + 1 == meta.files.len();
            let ok = f.name == chunk_name(i)
                && if last {
                    f.bytes > 0 && f.bytes <= CHUNK_BYTES && f.bytes.is_multiple_of(frame)
                } else {
                    f.bytes == CHUNK_BYTES
                };
            if !ok {
                return Err(not_pcm("chunk list"));
            }
            chunks.push(f.bytes);
        }
        let frames = chunks.iter().sum::<u64>() / frame;
        Ok(PcmEntry {
            recipe,
            layout,
            rate: recipe.rate(),
            frames,
            chunks,
        })
    }
}

#[cfg(test)]
#[path = "pcm_tests.rs"]
mod tests;
