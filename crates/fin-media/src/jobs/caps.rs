//! `media.capabilities`: the v1 answer of 6A (plan 6A Task 13) and the v2 report of 6B-1
//! (plan 6B-1 Task 2; the worker answers v2 from Task 8). The v1 types are `Legacy*` (and
//! `CapSupport`, `CodecCapabilities`) until Task 8 removes them; v2 has the plain names.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::GpuAdapter;
use crate::backend::VendorClass;
use crate::format::{AudioCodec, Chroma, VideoCodec};

/// `media.capabilities` v1 takes no params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilitiesParams {}

/// How a codec direction is served on this machine (spec §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CapSupport {
    Hardware,
    Software,
    None,
}

/// v1: one codec's support, the shape of `fin-codecs`' `CodecSupport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LegacyCodecCaps {
    pub codec: VideoCodec,
    pub decode: CapSupport,
    pub encode: CapSupport,
    /// The backends that would be used, decode first.
    pub backends: Vec<String>,
}

/// What this machine can decode and encode: the shape of `fin-codecs`' `Capabilities`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodecCapabilities {
    pub video: Vec<LegacyCodecCaps>,
}

/// v1: the worker binary's versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LegacyWorkerInfo {
    /// The `fin-workers` crate's version.
    pub version: String,
    /// [`RENDER_VERSION`](crate::RENDER_VERSION): part of every cache key (spec §2.5).
    pub render_version: u32,
}

/// What `media.capabilities` v1 answers (plan 6A Task 13): codecs, the GPU a render would use
/// (`null` when no adapter, not even a software one, can be opened) and the worker's versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMediaCapabilities {
    pub codecs: CodecCapabilities,
    pub gpu: Option<GpuAdapter>,
    pub worker: LegacyWorkerInfo,
}

// ---- v2 (plan 6B-1 Task 2; answered by the worker from Task 8, kind and journal kind v2) ----

/// How a codec direction is served (v2): in hardware, in software, or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CapLevel {
    Hw,
    Sw,
    None,
}

/// A codec profile a capability row is about (absent: the codec as a whole).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Profile {
    /// H.264 Constrained Baseline / Baseline.
    Baseline,
    /// H.264 Main, HEVC Main, AV1 Main.
    Main,
    /// H.264 High, AV1 High.
    High,
    /// HEVC Main 10.
    Main10,
    /// VP9 profile 0 (8-bit 4:2:0).
    Profile0,
    /// VP9 profile 2 (10/12-bit 4:2:0).
    Profile2,
    /// H.264 High 10 (10-bit 4:2:0; cameras, rarely in hardware).
    High10,
    /// H.264 High 4:2:2 (10-bit 4:2:2 cameras).
    High422,
    /// HEVC Main 4:2:2 10 (range extensions; cameras).
    Main422_10,
    /// ProRes 422 (Proxy, LT, standard, HQ).
    ProRes422,
    /// ProRes 4444 and 4444 XQ.
    ProRes4444,
    /// A profile with no row of its own: never served by a backend that claims only named
    /// profiles (`CODEC_UNAVAILABLE`, `codecPlugin` guidance).
    Other,
}

impl Profile {
    /// The profile a stream's sample entry and record name: `profile` is the record's number
    /// (H.264 `profile_idc`, HEVC `general_profile_idc`, AV1 `seq_profile`, the VP9 profile),
    /// `fourcc` the sample entry type (ProRes has no record). `None` when the stream names
    /// none (no record); a number with no row of its own is [`Profile::Other`].
    pub fn of(
        codec: VideoCodec,
        fourcc: [u8; 4],
        profile: Option<u8>,
        bit_depth: u8,
        chroma: Chroma,
    ) -> Option<Profile> {
        if codec == VideoCodec::ProRes {
            return Some(match &fourcc {
                b"ap4h" | b"ap4x" => Profile::ProRes4444,
                _ => Profile::ProRes422,
            });
        }
        let p = profile?;
        Some(match (codec, p) {
            (VideoCodec::H264, 66) => Profile::Baseline,
            (VideoCodec::H264, 77) => Profile::Main,
            (VideoCodec::H264, 100) => Profile::High,
            (VideoCodec::H264, 110) => Profile::High10,
            (VideoCodec::H264, 122) => Profile::High422,
            (VideoCodec::Hevc, 1) => Profile::Main,
            (VideoCodec::Hevc, 2) => Profile::Main10,
            // Range extensions: only 4:2:2 at 10 bits or less has a row.
            (VideoCodec::Hevc, 4) if chroma == Chroma::Yuv422 && bit_depth <= 10 => {
                Profile::Main422_10
            }
            (VideoCodec::Av1, 0) => Profile::Main,
            (VideoCodec::Av1, 1) => Profile::High,
            (VideoCodec::Vp9, 0) => Profile::Profile0,
            (VideoCodec::Vp9, 2) => Profile::Profile2,
            _ => Profile::Other,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Decode,
    Encode,
}

/// A backend that serves a codec row, by its `fin_media::backend` name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapBackend {
    pub name: String,
    pub class: VendorClass,
    pub direction: Direction,
    /// Runs on hardware (a hardware MFT, VideoToolbox's hardware decoder).
    pub hw: bool,
}

/// What a gap in the report is about: one closed table (`fin-codecs`' `probe/guidance.rs`
/// holds the text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GuidanceCode {
    /// Windows: the HEVC Video Extensions are not installed.
    HevcExtensions,
    /// Windows: the AV1 Video Extension is not installed.
    Av1Extension,
    /// Windows N: the Media Feature Pack is not installed.
    MediaFeaturePack,
    /// Windows LTSC may lack the Store codec extensions.
    LtscStoreCodecs,
    /// A GPU without a hardware decoder or encoder: its driver may be missing or old.
    GpuDriver,
    /// The codec plugin adds it (spec §6.5).
    CodecPlugin,
    /// A later batch of sub-project 6 adds it (`batch`).
    LaterBatch,
}

/// The sub-project 6 batch that closes a `laterBatch` gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Batch {
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

/// One line of guidance for a gap: what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Guidance {
    pub code: GuidanceCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Batch")]
    pub batch: Option<Batch>,
    pub fix: String,
}

/// One codec (and profile and bit depth, when the row is about one) on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodecCaps {
    pub codec: VideoCodec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Profile")]
    pub profile: Option<Profile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u8")]
    pub bit_depth: Option<u8>,
    pub decode: CapLevel,
    pub encode: CapLevel,
    /// The backends that would serve it, decode first, in the order they are tried.
    pub backends: Vec<CapBackend>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guidance: Vec<Guidance>,
}

/// A GPU by numeric ids only (owner decision 7): never a name or driver string. This is the
/// wire and journal form; binding a session to an adapter (Windows' LUID, plan 6B-1 Task 6)
/// is `fin-codecs`' own, never serialised, so no per-boot id reaches a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdapterId {
    /// The PCI vendor id.
    pub vendor_id: u32,
    /// The PCI device id.
    pub device_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub driver_major: Option<u32>,
}

/// An encoder that encoded the capability probe's test clip (plan 6B-1 Decision 19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EncoderCaps {
    pub backend: String,
    pub class: VendorClass,
    pub codec: VideoCodec,
    /// Runs on hardware (VideoToolbox, a hardware MFT, Microsoft's own hardware MFT); the
    /// `hwenc` pool and the proxy kind's resources key on it (Tasks 9, 14).
    pub hw: bool,
    /// Can encode every frame as a keyframe.
    pub intra_only: bool,
    /// Intra-only frames per second at 540p, measured on the test clip (Decision 8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub intra_fps: Option<u32>,
    /// Concurrent sessions that opened, up to 4; absent when not counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub session_limit: Option<u32>,
    // The GPU a hardware encoder is bound to (a doc comment here would make the generator
    // copy `AdapterId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AdapterId")]
    pub adapter: Option<AdapterId>,
}

/// The OS confinement the worker applied to itself (spec §3.5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum WorkerSandbox {
    None,
    /// 7C's Linux Landlock `worker` profile (6B-1 Task 18).
    Landlock,
    /// 7C's full `worker` profile on every OS (6G).
    Worker,
}

/// The worker binary's versions and confinement (v2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerInfo {
    /// The `fin-workers` crate's version.
    pub version: String,
    /// [`RENDER_VERSION`](crate::RENDER_VERSION): part of every cache key (spec §2.5).
    pub render_version: u32,
    pub sandbox: WorkerSandbox,
}

/// One audio codec on this machine (none decode or encode before 6C; the rows carry the
/// guidance, plan 6B-1 Decision 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioCodecCaps {
    pub codec: AudioCodec,
    pub decode: CapLevel,
    pub encode: CapLevel,
    pub backends: Vec<CapBackend>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guidance: Vec<Guidance>,
}

/// What `media.capabilities` v2 answers: per codec, profile and bit depth what this machine
/// decodes and encodes and through which backend (with guidance for every gap), the audio
/// codecs, the encoders that passed their test clip, the GPU a render would use and the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaCapabilities {
    pub codecs: Vec<CodecCaps>,
    pub audio: Vec<AudioCodecCaps>,
    pub encoders: Vec<EncoderCaps>,
    pub gpu: Option<GpuAdapter>,
    pub worker: WorkerInfo,
}

impl MediaCapabilities {
    /// The rows of `codec` (every profile and depth).
    pub fn rows(&self, codec: VideoCodec) -> impl Iterator<Item = &CodecCaps> {
        self.codecs.iter().filter(move |c| c.codec == codec)
    }
}
