//! The media engine's diagnostics (sub-project 6 spec §15, 6A plan Task 14, 6B-1 plan Task 19):
//! the optional `media` object of `job.done` (`kv` 2), `media.capabilities` (`kv` 2) and
//! `render.stats`.
//!
//! Every field is `meta`: numbers, booleans, closed enums, or a [`Name`] from the tables in
//! [`names_media`](crate::events::names_media). No path, file or project name, no adapter or
//! driver *name string* (only the numeric PCI ids and the driver's major version, owner
//! decision 7), no decoder error text. `fin-workers` builds them; `finisherd` forwards them
//! to the journal, which is the only writer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::stats::Histogram;
use crate::events::approx::{approx_fixed, approx_struct};
use crate::events::class::Name;
use crate::events::names_media::{self, BackendClass};

/// A frame size as a bucket: the smallest standard size the frame fits within, by pixel count
/// (so a portrait clip buckets as its landscape twin): `sd` ≤ 960×540, `hd720` ≤ 1280×720,
/// `hd1080` ≤ 2048×1080, `qhd` ≤ 2560×1440, `uhd4k` ≤ 4096×2160, `uhd8k` above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ResolutionBucket {
    Sd,
    Hd720,
    Hd1080,
    Qhd,
    Uhd4k,
    Uhd8k,
}

impl ResolutionBucket {
    /// The bucket of a `width` × `height` frame.
    pub fn of(width: u32, height: u32) -> ResolutionBucket {
        let pixels = u64::from(width) * u64::from(height);
        match pixels {
            0..=518_400 => ResolutionBucket::Sd,
            518_401..=921_600 => ResolutionBucket::Hd720,
            921_601..=2_211_840 => ResolutionBucket::Hd1080,
            2_211_841..=3_686_400 => ResolutionBucket::Qhd,
            3_686_401..=8_847_360 => ResolutionBucket::Uhd4k,
            _ => ResolutionBucket::Uhd8k,
        }
    }
}

/// How frames travel between the decoder, the compositor and the encoder (spec §5.4): through
/// host memory (`copy`; all of 6A) or as shared GPU surfaces (`zeroCopy`, 6G).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CopyPath {
    Copy,
    ZeroCopy,
}

/// The worker sandbox in force (spec §3.5): `none` in 6A.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Sandbox {
    None,
    Landlock,
    Worker,
}

/// How a codec direction is served: hardware, software, or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SupportClass {
    Hw,
    Sw,
    None,
}

/// A GPU adapter's type (`wgpu::DeviceType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AdapterType {
    Integrated,
    Discrete,
    Virtual,
    Cpu,
    Other,
}

impl AdapterType {
    /// The type named by `wgpu`'s `DeviceType` debug form (`"DiscreteGpu"`, `"Cpu"`, …).
    pub fn from_wgpu(name: &str) -> AdapterType {
        match name {
            "IntegratedGpu" => AdapterType::Integrated,
            "DiscreteGpu" => AdapterType::Discrete,
            "VirtualGpu" => AdapterType::Virtual,
            "Cpu" => AdapterType::Cpu,
            _ => AdapterType::Other,
        }
    }
}

/// A stage of a render, for [`RenderStats`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RenderPhase {
    Decode,
    Composite,
    Readback,
    /// An encoder's send and receive calls per frame (6B-1: `media.proxy`).
    Encode,
}

/// The `media` object of `job.done` `kv` 2: what a media job did. `codec`, `backend` and `hw`
/// describe the decode that served the job (the first source it decoded; `none` when it
/// decoded nothing). Kinds (6B-1 Task 19): `media.look`; `media.proxy` (frames encoded, the
/// proxy's size; its encoder is in `render.stats`' `encode` rows); `media.thumbs` (thumbnails or
/// sprites made, from the proxy or the original it decoded); `media.probeDeep` (its first frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobMedia {
    // `MEDIA_CODECS`.
    #[serde(deserialize_with = "names_media::media_codec")]
    pub codec: Name,
    // `MEDIA_BACKENDS`.
    #[serde(deserialize_with = "names_media::media_backend")]
    pub backend: Name,
    pub hw: bool,
    /// The largest frame the job produced.
    pub resolution: ResolutionBucket,
    /// Frames the job produced.
    pub frames: u32,
    /// Media seconds produced per wall second of rendering, in thousandths (1000 = real
    /// time): the frames' durations at the timeline's rate over decode + composite +
    /// readback time (`media.proxy`: the frames' durations at the source's nominal rate over the
    /// decode, scale and encode time). `0` when the job took no measurable time, and for kinds
    /// that do not play media through (`media.thumbs`, `media.probeDeep`).
    pub rt_factor_milli: u32,
    /// Every source the job needed was already open in the warm worker (and it needed one).
    pub cache_hit: bool,
    pub copy_path: CopyPath,
}
approx_struct!(JobMedia {
    codec,
    backend,
    hw,
    resolution,
    frames,
    rt_factor_milli,
    cache_hit,
    copy_path
});

/// Which way a backend serves a codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodecDirection {
    Decode,
    Encode,
}

/// A backend, who provides it (spec §6.1's vendor class), which way it serves the row and
/// whether it runs on hardware.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackendEntry {
    // `MEDIA_BACKENDS`.
    #[serde(deserialize_with = "names_media::media_backend")]
    pub name: Name,
    pub class: BackendClass,
    pub direction: CodecDirection,
    pub hw: bool,
}
approx_struct!(BackendEntry {
    name,
    class,
    direction,
    hw
});

/// What a gap in the report is about (`fin-media`'s `GapCode`): the code only, never the
/// guidance text (the `fix` stays in the capability answer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GapCode {
    HevcExtensions,
    Av1Extension,
    MediaFeaturePack,
    LtscStoreCodecs,
    GpuDriver,
    CodecPlugin,
    LaterBatch,
}

/// The sub-project 6 batch that closes a `laterBatch` gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum GuidanceBatch {
    #[serde(rename = "6B-2")]
    B6b2,
    #[serde(rename = "6C")]
    B6c,
    #[serde(rename = "6D")]
    B6d,
    #[serde(rename = "6E")]
    B6e,
    #[serde(rename = "6F")]
    B6f,
    #[serde(rename = "6G")]
    B6g,
}

/// One gap of a row, by code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GuidanceEntry {
    pub code: GapCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "GuidanceBatch")]
    pub batch: Option<GuidanceBatch>,
}
approx_struct!(GuidanceEntry { code, batch });

/// One video codec (and profile and bit depth, when the row is about one) on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodecEntry {
    // `MEDIA_CODECS`.
    #[serde(deserialize_with = "names_media::media_codec")]
    pub codec: Name,
    // `MEDIA_PROFILES`; absent: the codec as a whole.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "names_media::opt_media_profile"
    )]
    #[schemars(with = "Name")]
    pub profile: Option<Name>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u8")]
    pub bit_depth: Option<u8>,
    pub decode: SupportClass,
    pub encode: SupportClass,
    /// The backends that would serve it, decode first, in the order they are tried.
    pub backends: Vec<BackendEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guidance: Vec<GuidanceEntry>,
}
approx_struct!(CodecEntry {
    codec,
    profile,
    bit_depth,
    decode,
    encode,
    backends,
    guidance
});

/// One audio codec on this machine (none before 6C: the rows carry the guidance).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioCodecEntry {
    // `MEDIA_AUDIO_CODECS`.
    #[serde(deserialize_with = "names_media::media_audio_codec")]
    pub codec: Name,
    pub decode: SupportClass,
    pub encode: SupportClass,
    pub backends: Vec<BackendEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guidance: Vec<GuidanceEntry>,
}
approx_struct!(AudioCodecEntry {
    codec,
    decode,
    encode,
    backends,
    guidance
});

/// A GPU by its numeric ids (owner decision 7): never a name or driver string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdapterIds {
    /// The PCI vendor id.
    pub vendor_id: u32,
    /// The PCI device id.
    pub device_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub driver_major: Option<u32>,
}
approx_struct!(AdapterIds {
    vendor_id,
    device_id,
    driver_major
});

/// An encoder that encoded the capability probe's test clip (6B-1 plan Decision 19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EncoderEntry {
    // `MEDIA_BACKENDS`.
    #[serde(deserialize_with = "names_media::media_backend")]
    pub backend: Name,
    pub class: BackendClass,
    // `MEDIA_CODECS`.
    #[serde(deserialize_with = "names_media::media_codec")]
    pub codec: Name,
    pub hw: bool,
    pub intra_only: bool,
    /// Intra-only frames per second at 540p on the test clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub intra_fps: Option<u32>,
    /// Concurrent sessions that opened, up to 4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub session_limit: Option<u32>,
    // The GPU a hardware encoder is bound to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AdapterIds")]
    pub adapter: Option<AdapterIds>,
}
approx_struct!(EncoderEntry {
    backend,
    class,
    codec,
    hw,
    intra_only,
    intra_fps,
    session_limit,
    adapter
});

/// The GPU adapter frames are composited on: numeric ids only (owner decision 7). The
/// adapter's and driver's name strings identify hardware and are never recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GpuEntry {
    // `MEDIA_BACKENDS` (`vulkan`, `metal`, `dx12`, …).
    #[serde(deserialize_with = "names_media::media_backend")]
    pub backend: Name,
    pub adapter_type: AdapterType,
    /// The PCI vendor id.
    pub vendor_id: u32,
    /// The PCI device id.
    pub device_id: u32,
    /// The driver's major version, when its version string has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub driver_major: Option<u32>,
    /// A CPU adapter (lavapipe, WARP).
    pub software: bool,
}
approx_struct!(GpuEntry {
    backend,
    adapter_type,
    vendor_id,
    device_id,
    driver_major,
    software
});

/// `media.capabilities` (`kv` 2, 6B-1 plan Decision 19): once per server run, what the media
/// worker can decode and encode per codec, profile and bit depth (with the guidance codes of
/// every gap), its audio rows, the encoders that passed their test clip with their session
/// limits and adapters, the GPU it draws on and the sandbox it runs in. `kv` 1 (6A) is gone, as
/// `job.done` 1 went: no upcaster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaCapabilitiesEvent {
    pub codecs: Vec<CodecEntry>,
    pub audio: Vec<AudioCodecEntry>,
    pub encoders: Vec<EncoderEntry>,
    // Absent when no adapter, not even a software one, opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "GpuEntry")]
    pub gpu: Option<GpuEntry>,
    pub copy_path: CopyPath,
    pub sandbox: Sandbox,
    /// `RENDER_VERSION`: part of every cache key.
    pub render_version: u32,
}
approx_struct!(MediaCapabilitiesEvent {
    codecs,
    audio,
    encoders,
    gpu,
    copy_path,
    sandbox,
    render_version
});

/// `render.stats` (`kv` 1): per-minute histograms of render stages in micros, one entry per
/// phase and backend. `backend` is the decode backend for `decode`, the GPU API for
/// `composite` and `readback`, and the encoder for `encode` (6B-1: proxies). A worker sends what it measured since its last job ended, so
/// several entries may share a window (`histogram.start`): readers add them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenderStats {
    pub phase: RenderPhase,
    // `MEDIA_BACKENDS`.
    #[serde(deserialize_with = "names_media::media_backend")]
    pub backend: Name,
    pub histogram: Histogram,
}
approx_struct!(RenderStats {
    phase,
    backend,
    histogram
});

approx_fixed!(10 =>
    ResolutionBucket, CopyPath, Sandbox, SupportClass, AdapterType, RenderPhase, BackendClass,
    CodecDirection, GapCode, GuidanceBatch,
);
