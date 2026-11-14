//! The media engine's diagnostics (sub-project 6 spec §15, 6A plan Task 14): the optional
//! `media` object of `job.done` (`kv` 2), `media.capabilities` and `render.stats`.
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
    /// No emitter before sub-project 6B (export).
    Encode,
    /// Audio decode and resample into the canonical format (6C).
    DecodeAudio,
    /// The audio mixer: effects, gains, the master bus (6C).
    Mix,
    /// Audio encode (6C: Opus proxies, the export encoders).
    EncodeAudio,
}

/// The `media` object of `job.done` `kv` 2: what a media job did. `codec`, `backend` and `hw`
/// describe the decode that served the job (the first source it decoded; `none` when it
/// decoded nothing).
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
    /// readback time. `0` when the job took no measurable time.
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

/// A backend and who provides it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackendEntry {
    // `MEDIA_BACKENDS`.
    #[serde(deserialize_with = "names_media::media_backend")]
    pub name: Name,
    pub class: BackendClass,
}
approx_struct!(BackendEntry { name, class });

/// One codec's support on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodecEntry {
    // `MEDIA_CODECS`.
    #[serde(deserialize_with = "names_media::media_codec")]
    pub codec: Name,
    pub decode: SupportClass,
    pub encode: SupportClass,
    /// The backends that would be used, decode first.
    pub backends: Vec<BackendEntry>,
}
approx_struct!(CodecEntry {
    codec,
    decode,
    encode,
    backends
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

/// `media.capabilities` (`kv` 1): once per server run, what the media worker can decode,
/// encode and draw on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaCapabilitiesEvent {
    pub codecs: Vec<CodecEntry>,
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
    gpu,
    copy_path,
    sandbox,
    render_version
});

/// `render.stats` (`kv` 1): per-minute histograms of render stages in micros, one entry per
/// phase and backend. `backend` is the decode backend for `decode` and the GPU API for
/// `composite` and `readback`. A worker sends what it measured since its last job ended, so
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
);
