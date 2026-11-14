//! The proxy's audio track (6C plan Task 20; sub-project 6 spec §7.1, proxy recipe v2).
//!
//! The source's first audio track through [`DecodedSource`] (48 kHz, its canonical layout: a
//! mono source stays one channel, 3 to 8 channels are downmixed to stereo), encoded with libopus
//! through [`open_audio_encoder`] under `EncodePurpose::Proxy` at
//! [`ProxyAudio::STEREO_BITRATE`] or [`ProxyAudio::MONO_BITRATE`]. The whole track is encoded
//! before the video, because the muxer takes its presented length when the track is added
//! (Decision 22: `presented = EncodedLength.valid`, media time = the encoder's priming); the
//! packets are held in one buffer (16 KB a second at 128 kb/s) and muxed beside the video by
//! time (`encode_pipeline`), again from the first one when the video encode restarts on another
//! encoder.
//!
//! A source whose audio this build cannot read (`CODEC_UNAVAILABLE`: a codec without a decoder
//! here, or a layout it cannot map; `UNSUPPORTED_CONTAINER`; `MEDIA_LIMIT`) gives a video-only
//! proxy, so its preview keeps its frames; any other failure (a corrupt track) is the item's.

use std::fs::File;

use fin_codecs::audio::{AudioEncoder, AudioEncoderConfig, open_audio_encoder};
use fin_container::AudioTrackConfig;
use fin_core::CoreError;
use fin_media::AudioCodec;
use fin_media::backend::EncodePurpose;
use fin_media::jobs::{PcmLayout, PcmRecipe, ProxyAudio};
use serde_json::json;

use super::audio_src::DecodedSource;

/// The proxy's rate (spec §7.1: 48 kHz).
const RATE: u32 = 48_000;

/// The encoded track: its muxer configuration and its packets, in order.
#[derive(Debug)]
pub struct AudioPackets {
    pub track: AudioTrackConfig,
    /// Samples each packet carries (its duration in the track's 48 kHz timescale).
    pub frame: u32,
    pub result: ProxyAudio,
    data: Vec<u8>,
    /// Where each packet ends in `data`.
    ends: Vec<usize>,
}

impl AudioPackets {
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    /// Packet `i`'s bytes.
    pub fn packet(&self, i: usize) -> &[u8] {
        let start = if i == 0 { 0 } else { self.ends[i - 1] };
        &self.data[start..self.ends[i]]
    }

    fn take(&mut self, enc: &mut dyn AudioEncoder) -> Result<(), CoreError> {
        while let Some(p) = enc.receive()? {
            self.data.extend_from_slice(&p.data);
            self.ends.push(self.data.len());
        }
        Ok(())
    }
}

/// The codes of an audio track this build cannot read (module docs).
fn unreadable(e: &CoreError) -> bool {
    matches!(
        e.code.as_str(),
        "CODEC_UNAVAILABLE" | "UNSUPPORTED_CONTAINER" | "MEDIA_LIMIT"
    )
}

fn encode_failed(message: String, details: serde_json::Value) -> CoreError {
    CoreError::new("ENCODE_FAILED", message).with_details(details)
}

/// `file`'s audio as the proxy's Opus track; `None` for a track this build cannot read or one
/// with no samples. `between` runs before each decoded chunk (cancel).
pub fn encode(
    file: File,
    mut between: impl FnMut() -> Result<(), CoreError>,
) -> Result<Option<AudioPackets>, CoreError> {
    match encode_all(file, &mut between) {
        Err(e) if unreadable(&e) => Ok(None),
        other => other,
    }
}

fn encode_all(
    file: File,
    between: &mut dyn FnMut() -> Result<(), CoreError>,
) -> Result<Option<AudioPackets>, CoreError> {
    let mut src = DecodedSource::open(file, PcmRecipe::Pcm48)?;
    let (channels, bitrate) = match src.feed_layout() {
        PcmLayout::Mono => (1u16, ProxyAudio::MONO_BITRATE),
        PcmLayout::Stereo => (2, ProxyAudio::STEREO_BITRATE),
    };
    let cfg = AudioEncoderConfig {
        codec: AudioCodec::Opus,
        rate: RATE,
        channels,
        bitrate,
    };
    // The encoder is opened before the first decode: a refusal here is the encoder's, which
    // `encode` must not mistake for an unreadable source.
    let mut enc = open_audio_encoder(&cfg, EncodePurpose::Proxy)
        .map_err(|e| encode_failed(e.message, json!({ "backend": "libopus", "cause": e.code })))?;
    let mut out = AudioPackets {
        track: AudioTrackConfig {
            config: enc.config(),
            sample_rate: RATE,
            channels,
            priming: 0,
            presented: None,
        },
        frame: enc.frame_size(),
        result: ProxyAudio {
            codec: AudioCodec::Opus,
            bitrate,
            priming: 0,
        },
        data: Vec::new(),
        ends: Vec::new(),
    };
    let mut sent = 0u64;
    while let Some(pcm) = {
        between()?;
        src.next_interleaved()?
    } {
        enc.send(&pcm, sent as i64)?;
        sent += (pcm.len() / channels as usize) as u64;
        out.take(&mut *enc)?;
    }
    if sent == 0 {
        return Ok(None);
    }
    let len = enc.finish()?;
    out.take(&mut *enc)?;
    let decoded = out.len() as u64 * out.frame as u64;
    if len.valid != sent
        || decoded != len.priming as u64 + len.valid + len.padding as u64
        || len.priming != enc.priming()
    {
        return Err(encode_failed(
            format!(
                "the encoder's length ({} priming, {} valid, {} padding) is not the {sent} samples \
                 sent in {} packets",
                len.priming,
                len.valid,
                len.padding,
                out.len()
            ),
            json!({ "backend": "libopus" }),
        ));
    }
    // The config is read again after the stream: `dOps` carries the encoder's pre-skip.
    out.track.config = enc.config();
    out.track.priming = len.priming;
    out.track.presented = Some(len.valid);
    out.result.priming = len.priming;
    Ok(Some(out))
}

#[cfg(test)]
#[path = "proxy_audio_tests.rs"]
mod tests;
