//! The libopus backend (6C plan Decisions 3, 11 and 22): `fin-opus-sys`'s shim wrapped once.
//! This module is the only place with `unsafe` in `fin-codecs`' audio code.
//!
//! Decoding is at 48 kHz for mapping families 0 and 1 (up to 8 channels, Vorbis order), with
//! the header's output gain; pre-skip is the reader's to trim (`AudioTrackInfo.priming`). The
//! encoder is `OPUS_APPLICATION_AUDIO`, 20 ms frames (960 samples), VBR at the requested
//! bitrate; its lookahead is the priming and the `dOps` pre-skip, and `finish` feeds zeros
//! until the last input sample is out, so the decoded total is `priming + valid + padding` with
//! `padding < 960`.

use std::collections::VecDeque;
use std::ffi::CStr;
use std::ptr::{self, NonNull};

use fin_container::CodecConfig;
use fin_core::CoreError;
use fin_media::backend_audio::{AudioBackendEntry, audio_backend};
use fin_media::{AudioCodec, audio_layout::ChannelLayout};
use fin_opus_sys as sys;
use serde_json::json;

use super::opus_head::{BACKEND, OpusHeader};
use super::registry::{AudioEncoderBackend, AudioOpenToken};
use super::traits::{AudioDecoder, AudioEncoder, AudioEncoderConfig, DecodedAudio, EncodedLength};
use crate::frame::EncodedPacket;
use crate::traits::BackendId;

/// Opus's only decode rate here, and the MP4 Opus track timescale.
const RATE: u32 = 48_000;
/// 20 ms at 48 kHz.
const FRAME: usize = 960;
/// The longest Opus packet: 120 ms at 48 kHz.
const MAX_FRAMES: usize = 5760;
/// Room for one 20 ms packet per stream (RFC 6716: at most 1,275 bytes a frame) and the
/// multistream self-delimiting lengths.
const MAX_PACKET_PER_STREAM: usize = 1_280;

/// libopus's text for `code`.
fn strerror(code: i32) -> String {
    // SAFETY: libopus returns a static NUL-terminated string for every code.
    unsafe { CStr::from_ptr(sys::fin_opus_strerror(code)) }
        .to_string_lossy()
        .into_owned()
}

/// The linked libopus's version string.
#[cfg(test)]
fn version() -> String {
    // SAFETY: libopus returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(sys::fin_opus_version()) }
        .to_string_lossy()
        .into_owned()
}

fn failed(code: &'static str, during: &str, e: i32) -> CoreError {
    CoreError::new(
        code,
        format!("libopus failed while {during}: {}", strerror(e)),
    )
    .with_details(json!({
        "backend": BACKEND, "during": during, "reason": strerror(e), "status": e,
    }))
}

/// A libopus multistream decoder; closed on drop.
struct Dec(NonNull<sys::FinOpusDec>);

// SAFETY: a libopus decoder has no thread affinity; it is used from one thread at a time
// (`&mut self` on every call).
unsafe impl Send for Dec {}

impl Drop for Dec {
    fn drop(&mut self) {
        // SAFETY: the decoder is open and closed once, here.
        unsafe { sys::fin_opus_dec_close(self.0.as_ptr()) }
    }
}

/// An Opus decoder behind the [`AudioDecoder`] trait.
pub(crate) struct OpusDecoder {
    dec: Dec,
    channels: usize,
    layout: ChannelLayout,
    buf: Vec<f32>,
    out: VecDeque<DecodedAudio>,
}

impl OpusDecoder {
    pub(crate) fn open(h: &OpusHeader) -> Result<Self, CoreError> {
        let mut d = ptr::null_mut();
        // SAFETY: `mapping` has one entry per channel (checked by `OpusHeader`); `d` is valid
        // to write and the shim fills it on success.
        let r = unsafe {
            sys::fin_opus_dec_open(
                h.channels as i32,
                h.streams as i32,
                h.coupled as i32,
                h.mapping.as_ptr(),
                h.gain as i32,
                &mut d,
            )
        };
        let dec = match (r, NonNull::new(d)) {
            (sys::OK, Some(d)) => Dec(d),
            (e, _) => return Err(failed("DECODE_FAILED", "opening the decoder", e)),
        };
        let channels = h.channels as usize;
        Ok(OpusDecoder {
            dec,
            channels,
            layout: h.layout(),
            buf: vec![0.0; MAX_FRAMES * channels],
            out: VecDeque::new(),
        })
    }
}

impl AudioDecoder for OpusDecoder {
    fn send(&mut self, packet: &[u8], pts: i64) -> Result<(), CoreError> {
        let len = i32::try_from(packet.len()).unwrap_or(i32::MAX);
        // SAFETY: the decoder is open; `packet` is a live slice of at least `len` bytes and
        // `buf` holds `MAX_FRAMES` frames of `channels` samples.
        let n = unsafe {
            sys::fin_opus_dec_decode(
                self.dec.0.as_ptr(),
                packet.as_ptr(),
                len,
                self.buf.as_mut_ptr(),
                MAX_FRAMES as i32,
            )
        };
        if n < 0 {
            return Err(failed("DECODE_FAILED", "decoding a packet", n));
        }
        let samples = n as usize * self.channels;
        self.out.push_back(DecodedAudio {
            interleaved: self.buf[..samples].to_vec(),
            layout: self.layout.clone(),
            rate: RATE,
            pts,
        });
        Ok(())
    }

    fn receive(&mut self) -> Result<Option<DecodedAudio>, CoreError> {
        Ok(self.out.pop_front())
    }

    fn finish(&mut self) -> Result<(), CoreError> {
        Ok(())
    }

    fn flush(&mut self) -> Result<(), CoreError> {
        self.out.clear();
        // SAFETY: the decoder is open.
        match unsafe { sys::fin_opus_dec_reset(self.dec.0.as_ptr()) } {
            sys::OK => Ok(()),
            e => Err(failed("DECODE_FAILED", "resetting the decoder", e)),
        }
    }

    fn backend(&self) -> BackendId {
        BackendId(BACKEND)
    }
}

/// A libopus multistream encoder; closed on drop.
struct Enc(NonNull<sys::FinOpusEnc>);

// SAFETY: as for `Dec`: no thread affinity, one thread at a time.
unsafe impl Send for Enc {}

impl Drop for Enc {
    fn drop(&mut self) {
        // SAFETY: the encoder is open and closed once, here.
        unsafe { sys::fin_opus_enc_close(self.0.as_ptr()) }
    }
}

/// An Opus encoder behind the [`AudioEncoder`] trait.
pub(crate) struct OpusEncoder {
    enc: Enc,
    channels: usize,
    header: OpusHeader,
    /// Input not yet a whole frame.
    pending: Vec<f32>,
    packet: Vec<u8>,
    out: VecDeque<EncodedPacket>,
    sent: u64,
    packets: u64,
    pts0: Option<i64>,
    finished: Option<EncodedLength>,
}

fn encode_failed(why: &str, details: serde_json::Value) -> CoreError {
    CoreError::new("ENCODE_FAILED", format!("Opus encode failed: {why}")).with_details(details)
}

impl OpusEncoder {
    fn open(cfg: &AudioEncoderConfig) -> Result<Self, CoreError> {
        let refuse = |reason: &str| {
            CoreError::new(
                "CODEC_UNAVAILABLE",
                format!(
                    "libopus does not encode {} channels at {} Hz and {} b/s",
                    cfg.channels, cfg.rate, cfg.bitrate
                ),
            )
            .with_details(json!({
                "codec": "opus", "direction": "encode", "backend": BACKEND, "reason": reason,
                "rate": cfg.rate, "channels": cfg.channels,
            }))
        };
        if cfg.rate != RATE {
            return Err(refuse("rate"));
        }
        if !(1..=8).contains(&cfg.channels) {
            return Err(refuse("channels"));
        }
        let Ok(bitrate) = i32::try_from(cfg.bitrate) else {
            return Err(refuse("bitrate"));
        };
        if bitrate <= 0 {
            return Err(refuse("bitrate"));
        }
        let (mut e, mut info) = (ptr::null_mut(), sys::FinOpusEncInfo::default());
        // SAFETY: `e` and `info` are valid to write; the shim fills both on success.
        let r = unsafe { sys::fin_opus_enc_open(cfg.channels as i32, bitrate, &mut e, &mut info) };
        let enc = match (r, NonNull::new(e)) {
            (sys::OK, Some(e)) => Enc(e),
            (e, _) => return Err(failed("ENCODE_FAILED", "opening the encoder", e)),
        };
        let channels = cfg.channels as usize;
        let header = OpusHeader {
            channels: channels as u8,
            pre_skip: u16::try_from(info.lookahead).unwrap_or(u16::MAX),
            gain: 0,
            family: if channels <= 2 { 0 } else { 1 },
            streams: info.streams as u8,
            coupled: info.coupled as u8,
            mapping: info.mapping[..channels].to_vec(),
        };
        Ok(OpusEncoder {
            enc,
            channels,
            packet: vec![0; MAX_PACKET_PER_STREAM * info.streams.max(1) as usize],
            header,
            pending: Vec::new(),
            out: VecDeque::new(),
            sent: 0,
            packets: 0,
            pts0: None,
            finished: None,
        })
    }

    /// Encodes the first frame of `pending`.
    fn encode_frame(&mut self) -> Result<(), CoreError> {
        let max = i32::try_from(self.packet.len()).unwrap_or(i32::MAX);
        // SAFETY: the encoder is open; `pending` holds at least `FRAME` frames of `channels`
        // samples and `packet` has `max` bytes of room.
        let n = unsafe {
            sys::fin_opus_enc_encode(
                self.enc.0.as_ptr(),
                self.pending.as_ptr(),
                FRAME as i32,
                self.packet.as_mut_ptr(),
                max,
            )
        };
        if n < 0 {
            return Err(failed("ENCODE_FAILED", "encoding a frame", n));
        }
        self.pending.drain(..FRAME * self.channels);
        let pts = self.pts0.unwrap_or(0) + (self.packets * FRAME as u64) as i64;
        self.packets += 1;
        self.out.push_back(EncodedPacket {
            data: self.packet[..n as usize].to_vec(),
            pts,
            dts: pts,
            keyframe: true,
        });
        Ok(())
    }
}

impl AudioEncoder for OpusEncoder {
    fn send(&mut self, pcm: &[f32], pts: i64) -> Result<(), CoreError> {
        if self.finished.is_some() {
            return Err(encode_failed(
                "samples after the end",
                json!({ "backend": BACKEND }),
            ));
        }
        if !pcm.len().is_multiple_of(self.channels) {
            return Err(encode_failed(
                "a partial frame",
                json!({ "backend": BACKEND, "samples": pcm.len(), "channels": self.channels }),
            ));
        }
        if pcm.iter().any(|x| !x.is_finite()) {
            return Err(encode_failed(
                "a sample that is not a finite number",
                json!({ "backend": BACKEND, "reason": "nonFinite" }),
            ));
        }
        self.pts0.get_or_insert(pts);
        self.sent += (pcm.len() / self.channels) as u64;
        self.pending.extend_from_slice(pcm);
        while self.pending.len() >= FRAME * self.channels {
            self.encode_frame()?;
        }
        Ok(())
    }

    fn receive(&mut self) -> Result<Option<EncodedPacket>, CoreError> {
        Ok(self.out.pop_front())
    }

    /// Feeds zeros until every input sample is out past the lookahead (at least one packet).
    fn finish(&mut self) -> Result<EncodedLength, CoreError> {
        if let Some(len) = self.finished {
            return Ok(len);
        }
        let priming = self.header.pre_skip as u64;
        let total = (priming + self.sent).div_ceil(FRAME as u64).max(1);
        while self.packets < total {
            let missing = FRAME * self.channels - self.pending.len().min(FRAME * self.channels);
            self.pending.extend(std::iter::repeat_n(0.0, missing));
            self.encode_frame()?;
        }
        let len = EncodedLength {
            priming: priming as u32,
            valid: self.sent,
            padding: (total * FRAME as u64 - priming - self.sent) as u32,
        };
        self.finished = Some(len);
        Ok(len)
    }

    fn config(&self) -> CodecConfig {
        CodecConfig::Opus {
            dops: self.header.to_dops(),
        }
    }

    fn priming(&self) -> u32 {
        self.header.pre_skip as u32
    }

    fn frame_size(&self) -> u32 {
        FRAME as u32
    }
}

/// libopus in the audio encoder registry.
pub(crate) struct LibOpus;

impl AudioEncoderBackend for LibOpus {
    fn entry(&self) -> &'static AudioBackendEntry {
        audio_backend(BACKEND).expect("AUDIO_BACKENDS lists libopus")
    }

    fn encodes(&self, cfg: &AudioEncoderConfig) -> bool {
        cfg.codec == AudioCodec::Opus
    }

    fn open(
        &self,
        cfg: &AudioEncoderConfig,
        _: AudioOpenToken,
    ) -> Result<Box<dyn AudioEncoder>, CoreError> {
        Ok(Box::new(OpusEncoder::open(cfg)?))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn linked_version_is_1_6_1() {
        assert_eq!(super::version(), "libopus 1.6.1");
    }
}
