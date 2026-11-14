//! Audio codec backends and the audio encoder policy (6C plan Decisions 10 and 24), beside the
//! video table of [`crate::backend`].
//!
//! [`audio_encoder_allowed`] is **the one function** that decides which class may encode audio
//! for a purpose: `fin_codecs::open_audio_encoder` filters its candidates through it and no other
//! code opens an audio encoder (`scripts/check-encoder-call-sites.mjs`). The rule mirrors owner
//! decision 9: exports, QA renders and mattes take `os`, `software` and `plugin` encoders, never
//! `gpu` (there is no GPU audio encoder today, so one appearing later is refused for them);
//! proxies take every class.

use fin_core::CoreError;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::backend::{EncodePurpose, PolicyReason, VendorClass};
use crate::format::AudioCodec;

/// One audio backend: its name (a `MEDIA_BACKENDS` entry, Decision 24), its class and what it
/// can decode and encode at most on the platforms it is built for.
#[derive(Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioBackendEntry {
    pub name: &'static str,
    pub class: VendorClass,
    pub decode: &'static [AudioCodec],
    pub encode: &'static [AudioCodec],
}

#[cfg(target_os = "linux")]
use AudioCodec::Aac;
use AudioCodec::{Alac, Flac, Mp3, Opus, Pcm, Vorbis};

/// What symphonia decodes on this target: AAC-LC only on Linux (Decision 9: Finisher's own AAC
/// code is compiled for Linux only; macOS and Windows decode AAC with the OS codec).
#[cfg(target_os = "linux")]
const SYMPHONIA_DECODES: &[AudioCodec] = &[Pcm, Flac, Alac, Mp3, Vorbis, Aac];
#[cfg(not(target_os = "linux"))]
const SYMPHONIA_DECODES: &[AudioCodec] = &[Pcm, Flac, Alac, Mp3, Vorbis];

/// The audio backends built for this target. Tasks 15–16 add `audiotoolbox` (macOS) and
/// `mediafoundation` (Windows).
pub static AUDIO_BACKENDS: &[AudioBackendEntry] = &[
    AudioBackendEntry {
        name: "symphonia",
        class: VendorClass::Software,
        decode: SYMPHONIA_DECODES,
        encode: &[],
    },
    // vo-aacenc, AAC-LC encode on Linux only (Task 14, Decision 12).
    #[cfg(target_os = "linux")]
    AudioBackendEntry {
        name: "vo-aacenc",
        class: VendorClass::Software,
        decode: &[],
        encode: &[Aac],
    },
    // libopus 1.6.1 built from source on every target (Task 13).
    AudioBackendEntry {
        name: "libopus",
        class: VendorClass::Software,
        decode: &[Opus],
        encode: &[Opus],
    },
];

/// The audio backend called `name`.
pub fn audio_backend(name: &str) -> Option<&'static AudioBackendEntry> {
    AUDIO_BACKENDS.iter().find(|b| b.name == name)
}

/// Why [`audio_encoder_allowed`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPolicyRefusal {
    pub reason: PolicyReason,
    pub class: VendorClass,
    pub codec: AudioCodec,
    pub purpose: EncodePurpose,
}

impl AudioPolicyRefusal {
    /// `CODEC_UNAVAILABLE` for an audio encode the policy refuses on `backend`.
    pub fn to_error(&self, backend: &str) -> CoreError {
        let name = |v: serde_json::Value| v.as_str().unwrap_or_default().to_string();
        let (codec, purpose, reason, class) = (
            name(json!(self.codec)),
            name(json!(self.purpose)),
            name(json!(self.reason)),
            name(json!(self.class)),
        );
        CoreError::new(
            "CODEC_UNAVAILABLE",
            format!("{backend} may not encode {codec} for {purpose} ({reason})"),
        )
        .with_details(json!({
            "codec": codec, "direction": "encode", "purpose": purpose,
            "backend": backend, "class": class, "reason": reason,
        }))
        .with_fix("exports use the OS's own or a software encoder; the codec plugin adds more")
    }
}

/// Whether an audio encoder of `class` may encode `codec` for `purpose` (Decision 10):
/// `Export`, `QaRender` and `Matte` refuse `gpu`; `Proxy` (and a test fixture) take every class.
pub fn audio_encoder_allowed(
    class: VendorClass,
    codec: AudioCodec,
    purpose: EncodePurpose,
) -> Result<(), AudioPolicyRefusal> {
    if class == VendorClass::Gpu && purpose.distributed() {
        return Err(AudioPolicyRefusal {
            reason: PolicyReason::GpuForDistributed,
            class,
            codec,
            purpose,
        });
    }
    Ok(())
}
