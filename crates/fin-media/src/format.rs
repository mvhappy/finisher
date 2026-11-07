//! Pixel formats, codecs and containers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A decoded picture's memory layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PixelFormat {
    /// 8-bit 4:2:0, Y plane then interleaved UV.
    Nv12,
    /// 10-bit 4:2:0 in 16-bit words (high bits), Y then interleaved UV.
    P010,
    /// 8-bit 4:2:0, three planes.
    I420,
    /// 10-bit 4:2:0, three planes of 16-bit words (low bits).
    I420p10,
    /// 8-bit 4:2:2, three planes.
    I422,
    /// 10-bit 4:2:2, three planes of 16-bit words (low bits).
    I422p10,
    /// 8-bit 4:4:4, three planes.
    I444,
    /// 10-bit 4:4:4, three planes of 16-bit words (low bits).
    I444p10,
    Rgba8,
    Rgba16f,
}

/// Chroma subsampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Chroma {
    Mono,
    Yuv420,
    Yuv422,
    Yuv444,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum VideoCodec {
    Av1,
    H264,
    Hevc,
    Vp9,
    Vp8,
    ProRes,
    /// Anything else; the sample entry's four-character code is in `VideoTrackInfo.fourcc`.
    Other,
}

impl VideoCodec {
    /// The codec of an ISOBMFF visual sample entry type.
    pub fn from_fourcc(fourcc: &[u8; 4]) -> VideoCodec {
        match fourcc {
            b"av01" => VideoCodec::Av1,
            b"avc1" | b"avc3" => VideoCodec::H264,
            b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" => VideoCodec::Hevc,
            b"vp09" => VideoCodec::Vp9,
            b"vp08" => VideoCodec::Vp8,
            b"apch" | b"apcn" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => VideoCodec::ProRes,
            _ => VideoCodec::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AudioCodec {
    Aac,
    Opus,
    Mp3,
    Flac,
    Alac,
    Pcm,
    Ac3,
    Eac3,
    /// Anything else; the sample entry's four-character code is in `AudioTrackInfo.fourcc`.
    Other,
}

impl AudioCodec {
    /// The codec of an ISOBMFF audio sample entry type.
    pub fn from_fourcc(fourcc: &[u8; 4]) -> AudioCodec {
        match fourcc {
            b"mp4a" => AudioCodec::Aac,
            b"Opus" => AudioCodec::Opus,
            b".mp3" => AudioCodec::Mp3,
            b"fLaC" => AudioCodec::Flac,
            b"alac" => AudioCodec::Alac,
            b"lpcm" | b"sowt" | b"twos" | b"ipcm" | b"fpcm" | b"in24" | b"in32" | b"fl32"
            | b"fl64" | b"raw " => AudioCodec::Pcm,
            b"ac-3" => AudioCodec::Ac3,
            b"ec-3" => AudioCodec::Eac3,
            _ => AudioCodec::Other,
        }
    }
}

/// The container family, from `ftyp`'s brands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ContainerKind {
    Mp4,
    /// QuickTime (`qt  ` major brand).
    Mov,
}
