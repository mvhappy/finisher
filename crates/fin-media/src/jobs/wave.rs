//! `media.waveform` v1 (plan 6C Task 19, Decisions 7, 19 and 20; sub-project 6 spec §7.2): a
//! source's waveform peaks as a content-addressed `wave` entry (`peaks.bin`) in the media cache,
//! made by `fin-workers media` and committed by the host.
//!
//! - **Recipe** `wave` v1 (Decision 19): `peaks.bin` holds, per channel of the 48 kHz stereo
//!   signal, `(min, max)` pairs as `i16` at 256, 1,024, 4,096, 16,384 and 65,536 samples per
//!   peak, behind a 32-byte header (`fin_audio::peaks` owns the format).
//! - **Key** (Decision 7): `{fingerprint, recipe, recipeVersion, params {audioVersion, rate},
//!   renderVersion, decoderClass}`. The peaks of a mono source are those of its dual-mono
//!   stereo form, so the layout is not a key input; the decoder class is the one that decodes
//!   the source (the host knows it from `mediaPcmPlan`'s plan of the same source's `pcm48`).
//! - **Source**: the `pcm48` entry of the same source when the host found one (`pcmKey`: the
//!   worker reads its chunks and falls back to the decode when the entry is not usable), else the
//!   decode. Both give the same bytes. A failed attempt restarts from sample 0, as `media.pcm`
//!   (Decision 23): the entry is small and written once.

use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::invalid;
use crate::AUDIO_VERSION;
use crate::backend::VendorClass;
use crate::cache::{CacheKey, KeyInput, Recipe, WAVE};
use crate::jobs::PCM48_RATE;

/// The entry's one file.
pub const PEAKS_FILE: &str = "peaks.bin";

/// Samples per peak of each level in `peaks.bin`, finest first (`fin_audio::peaks::LEVELS`, which
/// a worker test holds equal: hosts never link `fin-audio`).
pub const WAVE_LEVELS: [u32; 5] = [256, 1_024, 4_096, 16_384, 65_536];

/// The key input of a waveform (module docs).
pub fn wave_key_input(fingerprint: &str, decoder_class: VendorClass) -> KeyInput {
    KeyInput::new(
        fingerprint,
        WAVE,
        json!({ "audioVersion": AUDIO_VERSION, "rate": PCM48_RATE }),
        decoder_class,
    )
}

/// The cache key of a waveform.
pub fn wave_key(fingerprint: &str, decoder_class: VendorClass) -> CacheKey {
    CacheKey::compute(&wave_key_input(fingerprint, decoder_class))
}

/// The recipe of a waveform entry.
pub fn wave_recipe() -> Recipe {
    WAVE
}

/// `media.waveform` v1: the peaks of the asset whose content is `fingerprint`, which the cache
/// knows as `key`. A batch kind of one item, the fingerprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaveParams {
    pub fingerprint: String,
    /// The vendor class the host expects the source's decoder to have (part of the key).
    pub decoder_class: VendorClass,
    /// [`wave_key`] of the fields above: the worker writes `out/<key>/`.
    pub key: String,
    /// A project asset, through the job's `in/manifest.json`.
    pub asset: String,
    /// The source's `pcm48` entry the host found (its directory is in `in/pcm.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pcm_key: Option<String>,
    /// The project that asked (host only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The branch whose head names the asset (`main` when absent). Host only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl WaveParams {
    /// A named asset, a well-formed key and `pcmKey`.
    pub fn check(&self) -> Result<(), CoreError> {
        if self.fingerprint.is_empty() || self.fingerprint.len() > 256 {
            return Err(invalid("fingerprint", "1 to 256 characters".into()));
        }
        if self.asset.is_empty() {
            return Err(invalid("asset", "name an asset".into()));
        }
        CacheKey::parse(&self.key)
            .map_err(|_| invalid("key", "64 lower-case hex digits".into()))?;
        if let Some(k) = &self.pcm_key {
            CacheKey::parse(k).map_err(|_| invalid("pcmKey", "64 lower-case hex digits".into()))?;
        }
        Ok(())
    }
}

/// Where the peaks were made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum WaveFrom {
    /// The source's `pcm48` entry.
    #[serde(rename = "pcm48")]
    Pcm48,
    /// The decoded original.
    #[serde(rename = "decode")]
    Decode,
}

/// What `media.waveform` v1 answers for its item (the fingerprint).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaveResult {
    pub key: String,
    /// Levels in `peaks.bin` (5).
    pub levels: u32,
    /// Frames of the 48 kHz signal the peaks cover.
    pub frames: u64,
    /// `peaks.bin`'s size.
    pub bytes: u64,
    pub from: WaveFrom,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> WaveParams {
        WaveParams {
            fingerprint: "fp".into(),
            decoder_class: VendorClass::Software,
            key: wave_key("fp", VendorClass::Software).to_string(),
            asset: "a1".into(),
            pcm_key: None,
            project_id: None,
            branch: None,
        }
    }

    #[test]
    fn key_has_audio_version_fingerprint_and_decoder_class() {
        let input = wave_key_input("fp", VendorClass::Software);
        assert_eq!(
            (input.recipe.as_str(), input.recipe_version),
            ("wave", WAVE.version)
        );
        assert_eq!(
            input.params,
            json!({ "audioVersion": AUDIO_VERSION, "rate": 48_000 })
        );
        let k = wave_key("fp", VendorClass::Software);
        assert_ne!(k, wave_key("fp", VendorClass::Os));
        assert_ne!(k, wave_key("fp2", VendorClass::Software));
        // Another recipe over the same source is another key.
        let pcm = crate::jobs::pcm_key(
            "fp",
            crate::jobs::PcmRecipe::Pcm48,
            crate::jobs::PcmLayout::Stereo,
            VendorClass::Software,
        );
        assert_ne!(k, pcm);
    }

    #[test]
    fn params_check() {
        assert!(params().check().is_ok());
        let bad = |f: fn(&mut WaveParams)| {
            let mut p = params();
            f(&mut p);
            p.check().unwrap_err().code
        };
        assert_eq!(bad(|p| p.asset.clear()), "JOB_PARAMS_INVALID");
        assert_eq!(bad(|p| p.key = "x".into()), "JOB_PARAMS_INVALID");
        assert_eq!(bad(|p| p.pcm_key = Some("y".into())), "JOB_PARAMS_INVALID");
        assert_eq!(bad(|p| p.fingerprint.clear()), "JOB_PARAMS_INVALID");
        let mut ok = params();
        ok.pcm_key = Some(wave_key("fp", VendorClass::Os).to_string());
        assert!(ok.check().is_ok());
    }
}
