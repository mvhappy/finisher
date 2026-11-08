//! What this machine can decode and encode (spec §6.2), and opening a decoder for a track.

use fin_container::{CodecConfig, VideoTrack};
use fin_core::CoreError;
use fin_media::VideoCodec;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::av1::DecodeThreads;
use crate::traits::VideoDecoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Support {
    Hardware,
    Software,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecSupport {
    pub codec: VideoCodec,
    pub decode: Support,
    pub encode: Support,
    /// The backends that would be used, decode first.
    pub backends: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub video: Vec<CodecSupport>,
}

impl Capabilities {
    pub fn get(&self, codec: VideoCodec) -> Option<&CodecSupport> {
        self.video.iter().find(|c| c.codec == codec)
    }
}

/// 6A: AV1 decode (dav1d) and encode (SVT-AV1) in software; nothing else.
pub fn capabilities() -> Capabilities {
    let none = |codec| CodecSupport {
        codec,
        decode: Support::None,
        encode: Support::None,
        backends: Vec::new(),
    };
    Capabilities {
        video: vec![
            CodecSupport {
                codec: VideoCodec::Av1,
                decode: Support::Software,
                encode: Support::Software,
                backends: vec!["dav1d".into(), "svt-av1".into()],
            },
            none(VideoCodec::H264),
            none(VideoCodec::Hevc),
            none(VideoCodec::Vp9),
            none(VideoCodec::ProRes),
        ],
    }
}

fn unavailable(codec: &str, direction: &str) -> CoreError {
    CoreError::new(
        "CODEC_UNAVAILABLE",
        format!("no {codec} {direction}r on this machine"),
    )
    .with_details(json!({ "codec": codec, "direction": direction, "os": std::env::consts::OS }))
    .with_fix("see `fin media caps` for what this machine decodes; the codec plugin adds more")
}

/// A decoder for `track` (AV1 only in 6A) with dav1d's default threading (a worker per
/// logical processor, frame threading on); `CODEC_UNAVAILABLE` otherwise.
pub fn open_decoder(
    track: &VideoTrack,
    caps: &Capabilities,
) -> Result<Box<dyn VideoDecoder>, CoreError> {
    open_decoder_with(track, caps, DecodeThreads::default())
}

/// [`open_decoder`] with the decoder's threads and frames in flight chosen.
pub fn open_decoder_with(
    track: &VideoTrack,
    caps: &Capabilities,
    threads: DecodeThreads,
) -> Result<Box<dyn VideoDecoder>, CoreError> {
    open_decoder_limited(track, caps, threads, crate::av1::MAX_PIXELS)
}

/// [`open_decoder_with`] refusing pictures of more than `max_pixels` pixels.
pub fn open_decoder_limited(
    track: &VideoTrack,
    caps: &Capabilities,
    threads: DecodeThreads,
    max_pixels: u32,
) -> Result<Box<dyn VideoDecoder>, CoreError> {
    let codec = match &track.config {
        CodecConfig::Av1 { .. } => VideoCodec::Av1,
        CodecConfig::Avc { .. } => VideoCodec::H264,
        CodecConfig::Hevc { .. } => VideoCodec::Hevc,
        CodecConfig::Vp9 { .. } => VideoCodec::Vp9,
        CodecConfig::Other { fourcc } => VideoCodec::from_fourcc(fourcc),
        CodecConfig::Aac { .. } | CodecConfig::Opus { .. } => {
            return Err(unavailable("audio", "decode"));
        }
    };
    let name = serde_json::to_value(codec)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    match (codec, caps.get(codec).map(|c| c.decode)) {
        (VideoCodec::Av1, Some(Support::Software | Support::Hardware)) => Ok(Box::new(
            crate::av1::Dav1dDecoder::with_limit(threads, max_pixels)?,
        )),
        _ => Err(unavailable(&name, "decode")),
    }
}
