//! `media.proxy` v1 (plan 6B-1 Task 14; sub-project 6 spec §7.1): a preview proxy of one
//! source, made by `fin-workers media` and committed into the media cache by the host.
//!
//! - **Tiers** (Decision 6): `540` fits the coded size into 960 × 540 (540 × 960 when the coded
//!   picture is portrait), `1080` into 1920 × 1080, keeping the coded aspect ratio, with even
//!   dimensions and never upscaling ([`ProxyTier::fit`]). Rotation and pixel aspect are not
//!   baked: the entry's `meta.json` keeps the original's `MediaInfo`.
//! - **Frames** (Decision 7): exactly one proxy frame per presented frame of the original, in
//!   presentation order, stamped at the original's nominal rate (CFR). The entry's `table.bin`
//!   is the original's sample table ([`crate::table_bin`]); a reader maps a source time to an
//!   original frame index with `frame_at` on that table and reads proxy frame `i`.
//! - **GOP** (Decision 8, [`gop`]): `540` all-intra only when the encoder was measured at ≥ 8×
//!   real time intra-only (the capability probe of Task 8; until then never), else a keyframe at
//!   least every 0.25 s; `1080` a closed GOP with a keyframe at least every 0.5 s; no B-frames;
//!   a software encoder (SVT-AV1) also keeps spec §7.1's closed GOP of at most 15 frames.
//! - **Key** (Decision 11): the cache key of `{fingerprint, recipe (proxy-540 | proxy-1080),
//!   params {codec, audio}, renderVersion, decoderClass}` (`audio`: [`ProxySourceAudio`], 6C
//!   Task 20 review), computed by the host when it submits and
//!   again by the worker from the decoder it opened (`PLAN_MISMATCH {reason: cacheKey}` when
//!   they differ).
//! - **Audio** (recipe v2, 6C plan Task 20): the source's audio as Opus at 48 kHz in its
//!   canonical layout, 128 kb/s stereo or 64 kb/s for a mono source ([`ProxyAudio`]), beside the
//!   video in `proxy.mp4`; a source without audio, or whose audio cannot be read here, gives a
//!   video-only proxy (`ProxyResult.audioSkipped` says why).
//! - **Source**: a project asset (`asset`, through the job's `in/manifest.json`), or a library
//!   file (`file`, the `proxy540` library stage of 5D: opened through `fin-library`'s opener
//!   with the catalogue's raw chain and `Expect`). Exactly one of the two.

use fin_core::{CoreError, Rational};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::invalid;
use crate::backend::VendorClass;
use crate::backend_audio::{AUDIO_BACKENDS, AudioBackendEntry};
use crate::cache::{CacheKey, KeyInput, PROXY_540, PROXY_1080, Recipe};
use crate::format::{AudioCodec, ContainerKind, VideoCodec};
use crate::info::Size;

/// A proxy tier (spec §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ProxyTier {
    /// Scrub and remote clients: fits 960 × 540.
    #[serde(rename = "540")]
    Scrub,
    /// The default preview: fits 1920 × 1080.
    #[serde(rename = "1080")]
    Preview,
}

impl ProxyTier {
    pub const ALL: [ProxyTier; 2] = [ProxyTier::Scrub, ProxyTier::Preview];

    /// The tier's cache recipe.
    pub fn recipe(self) -> Recipe {
        match self {
            ProxyTier::Scrub => PROXY_540,
            ProxyTier::Preview => PROXY_1080,
        }
    }

    /// `"540"` or `"1080"`.
    pub fn name(self) -> &'static str {
        match self {
            ProxyTier::Scrub => "540",
            ProxyTier::Preview => "1080",
        }
    }

    /// The landscape box the tier fits into.
    pub fn bounds(self) -> Size {
        match self {
            ProxyTier::Scrub => Size::new(960, 540),
            ProxyTier::Preview => Size::new(1920, 1080),
        }
    }

    /// The proxy's coded size for a source coded at `coded` (Decision 6): the largest size
    /// with the coded aspect ratio inside the tier's box (its portrait form when the coded
    /// picture is taller than wide), never larger than the source, each side even (rounded to
    /// the nearest even number, halves up) and at least 2. Exact integer arithmetic.
    pub fn fit(self, coded: Size) -> Size {
        let b = self.bounds();
        let (bw, bh) = if coded.height > coded.width {
            (b.height as u64, b.width as u64)
        } else {
            (b.width as u64, b.height as u64)
        };
        let (w, h) = (coded.width.max(1) as u64, coded.height.max(1) as u64);
        let even_down = |v: u64| (v & !1).max(2);
        let (tw, th) = if w <= bw && h <= bh {
            (even_down(w), even_down(h))
        } else if w * bh >= h * bw {
            // Width-bound: w → bw, h → h·bw/w rounded to the nearest even.
            (bw, (2 * ((h * bw + w) / (2 * w))).clamp(2, even_down(h)))
        } else {
            ((2 * ((w * bh + h) / (2 * h))).clamp(2, even_down(w)), bh)
        };
        Size::new(tw as u32, th as u32)
    }
}

/// The codec of a proxy (Decision 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProxyCodec {
    /// H.264 from an OS or hardware encoder (any vendor class: proxies never leave the cache).
    H264,
    /// AV1 from SVT-AV1 (preset 10, CRF, closed GOP).
    Av1,
}

impl ProxyCodec {
    pub fn video(self) -> VideoCodec {
        match self {
            ProxyCodec::H264 => VideoCodec::H264,
            ProxyCodec::Av1 => VideoCodec::Av1,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ProxyCodec::H264 => "h264",
            ProxyCodec::Av1 => "av1",
        }
    }
}

/// How an HDR source was brought to SDR (Decision 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ToneMapped {
    /// By the OS (`VTPixelTransferSession`, macOS).
    Os,
    /// By the compositor's ingest shader (`fin_media::colour::tonemap`).
    Shader,
}

/// A library file the `proxy540` stage hands the worker (5D Decision 39): the library's
/// canonical root, the file's raw chain below it and the `Expect` the opener checks, in
/// `fin-library`'s wire forms (`RawRel`, `Expect`), which the worker decodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyLibraryFile {
    pub library: String,
    /// The library's canonical root (absolute).
    pub root: String,
    /// `fin-library`'s `RawRel`: `{components: [<base64>]}`.
    pub rel: Value,
    /// `fin-library`'s `Expect`: `{size, mtimeNs, fileId?, dev?}`.
    pub expect: Value,
    /// The library hydrates cloud-only files when read.
    #[serde(default)]
    pub hydrate: bool,
}

/// `media.proxy` v1: a proxy of the source whose content is `fingerprint`, at `tier`, in
/// `codec`, which the cache knows as `key`. A batch kind of one item, the fingerprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyParams {
    pub fingerprint: String,
    pub tier: ProxyTier,
    pub codec: ProxyCodec,
    /// The vendor class the host expects the original's decoder to have (part of the key).
    pub decoder_class: VendorClass,
    /// What the host expects of the original's audio ([`source_audio`], part of the key).
    pub audio: ProxySourceAudio,
    /// [`proxy_key`] of the fields above: where the host commits the entry. The worker writes
    /// `out/<key>/` and fails `PLAN_MISMATCH {reason: cacheKey}` when the decoder it opened
    /// gives another key.
    pub key: String,
    /// A project asset, through the job's `in/manifest.json` (the asset's fingerprint there
    /// must equal `fingerprint`, else `ASSET_CHANGED`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// A library file (the `proxy540` stage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ProxyLibraryFile")]
    pub file: Option<ProxyLibraryFile>,
    /// The project that asked (host only: `prepare` writes the manifest from its branch head,
    /// `onSucceeded` commits the entry for it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The branch whose head names the asset (`main` when absent). Host only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    // The source's display size, for the kind's memory rule (Decision 18). Host only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Size")]
    pub display: Option<Size>,
    /// The encoder may make a 540 proxy all-intra (Decision 8: measured at ≥ 8× real time by
    /// the capability probe). Absent: a keyframe every 0.25 s.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub intra_ok: bool,
}

impl ProxyParams {
    /// The key input for a source decoded by a backend of `decoder_class`, its audio `audio`.
    pub fn key_input(&self, decoder_class: VendorClass, audio: ProxySourceAudio) -> KeyInput {
        key_input(
            &self.fingerprint,
            self.tier,
            self.codec,
            decoder_class,
            audio,
        )
    }

    /// Exactly one source, a well-formed key (the checks serde cannot make).
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
        CacheKey::parse(&self.key)
            .map_err(|_| invalid("key", "64 lower-case hex digits".into()))?;
        Ok(())
    }
}

/// What a proxy's key says of the source's audio (6C Task 20 review): none, one this build
/// has no decoder for, or one a decoder of a class decodes. It is read from this build's static
/// tables ([`audio_decoder_for_proxy`] over [`AUDIO_BACKENDS`]), not from what the machine can do
/// at run time. So a **build** whose tables gain a decoder for the codec or container (AAC on
/// macOS and Windows with 6C Tasks 15–16; WebM audio) changes the key, and a video-only entry
/// made without it is made again. What does **not** change the key: a decoder the tables list
/// but the machine lacks or refuses at run time (Media Foundation's AAC on Windows N without the
/// Media Feature Pack; a profile such as HE-AAC that the listed decoder refuses), and a corrupt
/// track. Those key as decodable, their proxy is video only (`audioSkipped`), and a plugin or
/// feature pack installed later does not refresh it. Keying on the capability report's audio
/// decode rows instead is 6C Task 17's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProxySourceAudio {
    /// The source has no audio track.
    None,
    /// It has one this build reads no audio of (its codec, or its container).
    Unavailable,
    /// It has one a backend of this class decodes.
    Decoded(VendorClass),
}

/// The audio backend `fin-workers` opens for a proxy source's audio (`AudioReader` over
/// `fin-container`, so MP4/MOV only; WebM/Matroska audio is not read): libopus for Opus, the
/// first of [`AUDIO_BACKENDS`] for AAC, symphonia for MP3 (the codecs `open_audio_decoder` takes
/// from an MP4 sample entry); `None` for anything else. The worker checks the backend it opens
/// against it.
pub fn audio_decoder_for_proxy(
    container: ContainerKind,
    codec: AudioCodec,
) -> Option<&'static AudioBackendEntry> {
    if !matches!(container, ContainerKind::Mp4 | ContainerKind::Mov) {
        return None;
    }
    let by = |name: &str| {
        AUDIO_BACKENDS
            .iter()
            .find(|b| b.name == name && b.decode.contains(&codec))
    };
    match codec {
        AudioCodec::Opus => by("libopus"),
        AudioCodec::Mp3 => by("symphonia"),
        AudioCodec::Aac => AUDIO_BACKENDS.iter().find(|b| b.decode.contains(&codec)),
        _ => None,
    }
}

/// The `audioSkipped` code of a source whose audio the key names [`ProxySourceAudio::Unavailable`]:
/// `UNSUPPORTED_CONTAINER` when its container's audio is not read here (WebM, Matroska), else
/// `CODEC_UNAVAILABLE` (no decoder for the codec).
pub fn audio_unavailable_code(container: ContainerKind) -> &'static str {
    match container {
        ContainerKind::Mp4 | ContainerKind::Mov => "CODEC_UNAVAILABLE",
        ContainerKind::Webm | ContainerKind::Matroska => "UNSUPPORTED_CONTAINER",
    }
}

/// The key's [`ProxySourceAudio`] for a source in `container` whose audio track has `codec`
/// (`None`: no audio track).
pub fn source_audio(container: ContainerKind, codec: Option<AudioCodec>) -> ProxySourceAudio {
    match codec {
        None => ProxySourceAudio::None,
        Some(c) => audio_decoder_for_proxy(container, c)
            .map_or(ProxySourceAudio::Unavailable, |b| {
                ProxySourceAudio::Decoded(b.class)
            }),
    }
}

/// The key input of a proxy (Decision 11): the codec and the source's audio are the recipe's
/// parameters.
pub fn key_input(
    fingerprint: &str,
    tier: ProxyTier,
    codec: ProxyCodec,
    decoder_class: VendorClass,
    audio: ProxySourceAudio,
) -> KeyInput {
    KeyInput::new(
        fingerprint,
        tier.recipe(),
        json!({ "codec": codec, "audio": audio }),
        decoder_class,
    )
}

/// The cache key of a proxy.
pub fn proxy_key(
    fingerprint: &str,
    tier: ProxyTier,
    codec: ProxyCodec,
    decoder_class: VendorClass,
    audio: ProxySourceAudio,
) -> CacheKey {
    CacheKey::compute(&key_input(fingerprint, tier, codec, decoder_class, audio))
}

/// The decoder a capability report's list for `source` names first, by
/// [`crate::backend::BACKENDS`]: an encoder or an unknown name is skipped.
pub fn decoder_for<S: AsRef<str>>(
    source: VideoCodec,
    backends: &[S],
) -> Option<&'static crate::backend::BackendEntry> {
    backends
        .iter()
        .filter_map(|n| crate::backend::backend(n.as_ref()))
        .find(|b| b.decode.contains(&source))
}

/// `CODEC_UNAVAILABLE {direction: decode}`: no backend of the report decodes `source`.
pub(crate) fn no_decoder(source: VideoCodec) -> CoreError {
    let name = serde_json::to_value(source).unwrap_or_default();
    CoreError::new(
        "CODEC_UNAVAILABLE",
        format!(
            "no decoder on this machine reads {}",
            name.as_str().unwrap_or("this codec")
        ),
    )
    .with_details(json!({ "codec": name, "direction": "decode" }))
    .with_fix("see `fin media caps` for what this machine decodes; the codec plugin adds more")
}

/// The key a proxy of a `source`-coded original whose audio is `audio` ([`source_audio`]) will
/// have, with the decoder the report names first (the host's half of Decision 11);
/// `CODEC_UNAVAILABLE {direction: decode}` when none decodes it.
pub fn plan_key<S: AsRef<str>>(
    fingerprint: &str,
    tier: ProxyTier,
    codec: ProxyCodec,
    source: VideoCodec,
    backends: &[S],
    audio: ProxySourceAudio,
) -> Result<(CacheKey, &'static crate::backend::BackendEntry), CoreError> {
    let Some(b) = decoder_for(source, backends) else {
        return Err(no_decoder(source));
    };
    Ok((proxy_key(fingerprint, tier, codec, b.class, audio), b))
}

/// Keyframe placement (Decision 8): `(frames between keyframes, all-intra)`. `rate` is the
/// original's nominal rate; `intra_ok` says the encoder was measured fast enough for all-intra
/// 540; `software` adds spec §7.1's closed GOP of at most 15 frames (SVT-AV1).
pub fn gop(tier: ProxyTier, rate: Rational, intra_ok: bool, software: bool) -> (u32, bool) {
    if tier == ProxyTier::Scrub && intra_ok {
        return (1, true);
    }
    // ⌊rate × seconds⌋ frames, at least 1: a keyframe at least every `seconds`.
    let quarters: i128 = match tier {
        ProxyTier::Scrub => 1,
        ProxyTier::Preview => 2,
    };
    let (num, den) = (rate.num().max(1) as i128, rate.den().max(1) as i128);
    let frames = (num * quarters / (den * 4)).clamp(1, u32::MAX as i128) as u32;
    let frames = if software { frames.min(15) } else { frames };
    (frames, false)
}

/// One move of the encode to another encoder (`ENCODER_HUNG`'s fallback, an encoder that did
/// not open), as the result records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyFallback {
    pub from: String,
    pub to: String,
    /// `openFailed` or `hung`.
    pub reason: String,
}

/// The proxy's audio track (recipe v2, 6C plan Task 20): the source's audio at 48 kHz in its
/// canonical layout (a mono source stays one channel), as Opus at 128 kb/s stereo or 64 kb/s
/// mono; its edit list presents exactly the source's samples after `priming`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyAudio {
    pub codec: ProxyAudioCodec,
    /// Bits a second.
    pub bitrate: u32,
    /// The encoder's delay in samples (the edit list's media time, `dOps` pre-skip).
    pub priming: u32,
}

/// The codec of a proxy's audio (spec §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProxyAudioCodec {
    Opus,
}

/// Why a source's audio track is not in its proxy (6C Task 20 review): the decode's error code
/// (`CODEC_UNAVAILABLE`, `UNSUPPORTED_CONTAINER`, `MEDIA_LIMIT`, `MEDIA_CORRUPT`,
/// `DECODE_FAILED`). The mix reads `pcm48`, which reports the same failure as its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyAudioSkipped {
    pub code: String,
}

impl ProxyAudio {
    /// Spec §7.1: 128 kb/s for a stereo (or downmixed) source.
    pub const STEREO_BITRATE: u32 = 128_000;
    /// A mono source keeps one channel (6C plan Decision 3) at half the stereo rate.
    pub const MONO_BITRATE: u32 = 64_000;
}

/// What `media.proxy` v1 answers for its item (the fingerprint).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyResult {
    pub key: String,
    pub tier: ProxyTier,
    pub codec: ProxyCodec,
    /// The encoder's backend (`svt-av1`, `videotoolbox`, `mf-hw`, …) and its vendor class.
    pub backend: String,
    pub class: VendorClass,
    /// The decoder's backend.
    pub decoder: String,
    /// Proxy frames: the original's presented frames.
    pub frames: u64,
    /// The entry's bytes (its files, `meta.json` not included).
    pub bytes: u64,
    /// The proxy's coded size.
    pub size: Size,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ToneMapped")]
    pub tone_mapped: Option<ToneMapped>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fell_back: Vec<ProxyFallback>,
    /// The audio track; absent when the source has none or its audio could not be read (then
    /// `audioSkipped` says why): the proxy is then video only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ProxyAudio")]
    pub audio: Option<ProxyAudio>,
    /// The source has an audio track the proxy does not carry, and why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ProxyAudioSkipped")]
    pub audio_skipped: Option<ProxyAudioSkipped>,
}

/// One tier of `media.proxy.status`: its entry when the cache holds one, the job making it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyTierStatus {
    pub tier: ProxyTier,
    /// `ready` (in the cache), `making` (a job runs), `failed` (the last job failed) or
    /// `none`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ProxyCodec")]
    pub codec: Option<ProxyCodec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub job: Option<String>,
    /// The failed job's error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub code: Option<String>,
}

/// `media.proxy.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyStatus {
    pub asset: String,
    pub fingerprint: String,
    pub tiers: Vec<ProxyTierStatus>,
}
