//! The media engine's name tables (sub-project 6, 6A Task 14), referenced from
//! [`super::names::TABLES`]: the codecs and backends the `job.done` media object, the
//! `media.capabilities` event and `render.stats` name. A value outside a table reads as
//! `"other"`; the tables grow with the backends (6B–6G) without a version bump.
//!
//! Backend names are the static names of `fin-codecs`' backends and `wgpu`'s API names;
//! adapter and driver *name strings* are never recorded (spec §15), only numeric ids.

use serde::{Deserialize, Deserializer, Serialize};

use super::class::Name;
use super::names::NameTable;

/// Video codecs (`fin-media`'s `VideoCodec`, camel case), audio codecs (`fin-media`'s
/// `AudioCodec`, 6C Decision 24) and `none` (a job that decoded nothing).
pub static MEDIA_CODECS: NameTable = NameTable::new(
    "MEDIA_CODECS",
    &[
        "av1", "h264", "hevc", "vp9", "vp8", "prores", "aac", "opus", "mp3", "flac", "alac",
        "vorbis", "pcm", "none",
    ],
);

/// Decode and encode backends (spec §6.1; the vendor class of each is [`backend_class`])
/// and the GPU APIs of the compositor (`wgpu`'s, lower case). `none` is a job that used no
/// backend of that kind.
pub static MEDIA_BACKENDS: NameTable = NameTable::new(
    "MEDIA_BACKENDS",
    &[
        // `software`
        "dav1d",
        "svt-av1",
        "libvpx",
        "libopus",
        "symphonia",
        "vo-aacenc",
        // `os`
        "videotoolbox",
        "audiotoolbox",
        // 6A's placeholder for Media Foundation: never emitted; 6B-1 names `mf-ms` and `mf-hw`.
        "mediafoundation",
        // Microsoft's own Media Foundation MFTs (6B-1 plan Decision 3).
        "mf-ms",
        // `gpu`
        // A GPU vendor's hardware MFT (6B-1 plan Decision 3; no vendor id reported is `gpu` too).
        "mf-hw",
        "vaapi",
        "d3d11va",
        "nvdec",
        "nvenc",
        // `plugin`
        "ffmpeg",
        // The compositor's GPU APIs.
        "vulkan",
        "metal",
        "dx12",
        "gl",
        "browserwebgpu",
        "none",
    ],
);

/// Who provides a backend (spec §6.1): the export-encoder policy and the capability report key
/// on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BackendClass {
    Os,
    Gpu,
    Software,
    Plugin,
    /// A GPU API (the compositor's), or a backend not in [`MEDIA_BACKENDS`].
    Other,
}

/// The vendor class of the backend `name`.
pub fn backend_class(name: &str) -> BackendClass {
    match name {
        "dav1d" | "svt-av1" | "libvpx" | "libopus" | "symphonia" | "vo-aacenc" => {
            BackendClass::Software
        }
        "videotoolbox" | "audiotoolbox" | "mediafoundation" | "mf-ms" => BackendClass::Os,
        "mf-hw" | "vaapi" | "d3d11va" | "nvdec" | "nvenc" => BackendClass::Gpu,
        "ffmpeg" => BackendClass::Plugin,
        _ => BackendClass::Other,
    }
}

pub fn media_codec<'de, D: Deserializer<'de>>(d: D) -> Result<Name, D::Error> {
    Ok(MEDIA_CODECS.get_or_other(&String::deserialize(d)?))
}

pub fn media_backend<'de, D: Deserializer<'de>>(d: D) -> Result<Name, D::Error> {
    Ok(MEDIA_BACKENDS.get_or_other(&String::deserialize(d)?))
}
