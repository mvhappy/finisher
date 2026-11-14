//! AAC-LC on Linux (6C plan Decisions 9, 12 and 22): symphonia's AAC-LC decoder (backend
//! `symphonia`) and vo-aacenc through `fin-aacenc-sys`'s shim (backend `vo-aacenc`). Compiled
//! for Linux targets only: Finisher's own AAC code ships nowhere else (spec §16.4); macOS and
//! Windows use the OS codec. Counsel item 6.1 stays open.
//!
//! Decoding refuses HE-AAC (SBR or PS signalled in the AudioSpecificConfig) before any packet,
//! with the codec plugin as the fix, and anything symphonia's AAC-LC decoder cannot do (other
//! object types, more than two channels, a program config element). The encoder takes
//! interleaved `f32` (fed only mixer PCM, so never untrusted input), converts it to 16-bit with
//! rounding and clamping, and writes raw AAC-LC access units of 1,024 samples, CBR; its delay is
//! one frame (1,024 samples), reported as `priming`, and `finish` feeds zeros until the last
//! input sample is out, so the decoded total is `priming + valid + padding`, `padding < 1024`.

use std::collections::VecDeque;
use std::ptr::{self, NonNull};

use fin_aacenc_sys as sys;
use fin_container::CodecConfig;
use fin_core::CoreError;
use fin_media::backend_audio::{AudioBackendEntry, audio_backend};
use fin_media::{AudioCodec, AudioTrackInfo};
use serde_json::json;
use symphonia::core::codecs::audio::AudioCodecParameters;
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;

use super::aac_config::{AOT_LC, asc_of_esds, esds, lc_asc, parse_asc};
use super::registry::{AudioEncoderBackend, AudioOpenToken, needs_config};
use super::symphonia::SymphoniaDecoder;
use super::traits::{AudioConfig, AudioDecoder, AudioEncoder, AudioEncoderConfig, EncodedLength};
use crate::frame::EncodedPacket;

/// The encoder's backend name (Decision 24).
const ENCODER: &str = "vo-aacenc";
/// AAC-LC's frame: 1,024 samples a channel.
const FRAME: usize = sys::FRAME;
/// vo-aacenc's delay: one frame.
const PRIMING: u32 = FRAME as u32;
/// An AAC access unit is at most 6,144 bits a channel (14496-3 §4.5.3.1).
const MAX_AU_PER_CHANNEL: usize = 6144 / 8;

fn unavailable(reason: &str, message: String, extra: serde_json::Value) -> CoreError {
    let mut details = json!({
        "codec": "aac", "direction": "decode", "os": std::env::consts::OS, "reason": reason,
    });
    if let (Some(d), Some(x)) = (details.as_object_mut(), extra.as_object()) {
        d.extend(x.clone());
    }
    CoreError::new("CODEC_UNAVAILABLE", message)
        .with_details(details)
        .with_fix("see `fin media caps` for what this machine decodes; the codec plugin adds more")
}

/// A decoder for an AAC track: symphonia's AAC-LC decoder from the track's AudioSpecificConfig
/// (the MP4 `esds`, or the extra data a symphonia reader found). HE-AAC is `CODEC_UNAVAILABLE
/// {profile: he-aac, fix: codecPlugin}` before any decode; a record that does not parse is
/// `MEDIA_CORRUPT`.
pub(crate) fn open_decoder(
    info: &AudioTrackInfo,
    config: &AudioConfig,
) -> Result<Box<dyn AudioDecoder>, CoreError> {
    let corrupt = || {
        CoreError::new(
            "MEDIA_CORRUPT",
            "the AAC configuration record (AudioSpecificConfig) is malformed",
        )
        .with_details(json!({ "at": "esds", "codec": "aac" }))
    };
    let asc: Vec<u8> = match config {
        AudioConfig::Record(CodecConfig::Aac { esds }) => {
            asc_of_esds(esds).ok_or_else(corrupt)?.to_vec()
        }
        AudioConfig::Symphonia(p) => match p.0.extra_data.as_deref() {
            Some(asc) => asc.to_vec(),
            // ADTS: no AudioSpecificConfig; the reader's parameters are LC by construction.
            None => return Ok(Box::new(SymphoniaDecoder::open(&p.0)?)),
        },
        AudioConfig::Record(_) => return Err(needs_config(info.codec)),
    };
    let a = parse_asc(&asc).ok_or_else(corrupt)?;
    if a.he_aac {
        return Err(CoreError::new(
            "CODEC_UNAVAILABLE",
            "HE-AAC (SBR/PS) is not decoded on Linux: Finisher decodes AAC-LC only",
        )
        .with_details(json!({
            "codec": "aac", "direction": "decode", "profile": "he-aac", "os": "linux",
            "reason": "profile", "fix": "codecPlugin",
        }))
        .with_fix("install the codec plugin to decode HE-AAC on Linux"));
    }
    if a.object != AOT_LC {
        return Err(unavailable(
            "profile",
            format!(
                "AAC object type {} is not decoded here (AAC-LC only)",
                a.object
            ),
            json!({ "objectType": a.object }),
        ));
    }
    if !(1..=2).contains(&a.channels) {
        return Err(unavailable(
            "channels",
            format!(
                "AAC channel configuration {} is not decoded here (mono and stereo only)",
                a.channels
            ),
            json!({ "channelConfiguration": a.channels }),
        ));
    }
    let mut p = AudioCodecParameters::new();
    p.for_codec(CODEC_ID_AAC)
        .with_sample_rate(a.rate)
        .with_extra_data(asc.into_boxed_slice());
    Ok(Box::new(SymphoniaDecoder::open(&p)?))
}

/// A vo-aacenc encoder; closed on drop.
struct Enc(NonNull<sys::FinAacEnc>);

// SAFETY: a vo-aacenc encoder has no thread affinity; it is used from one thread at a time
// (`&mut self` on every call).
unsafe impl Send for Enc {}

impl Drop for Enc {
    fn drop(&mut self) {
        // SAFETY: the encoder is open and closed once, here.
        unsafe { sys::fin_aacenc_close(self.0.as_ptr()) }
    }
}

fn failed(during: &str, status: u32) -> CoreError {
    CoreError::new(
        "ENCODE_FAILED",
        format!("vo-aacenc failed while {during} (status {status:#x})"),
    )
    .with_details(json!({ "backend": ENCODER, "during": during, "status": status }))
}

fn encode_failed(why: &str, details: serde_json::Value) -> CoreError {
    CoreError::new("ENCODE_FAILED", format!("AAC encode failed: {why}")).with_details(details)
}

/// An AAC-LC encoder behind the [`AudioEncoder`] trait.
pub(crate) struct AacEncoder {
    enc: Enc,
    channels: usize,
    config: CodecConfig,
    /// Input not yet a whole frame, already 16-bit.
    pending: Vec<i16>,
    packet: Vec<u8>,
    out: VecDeque<EncodedPacket>,
    sent: u64,
    packets: u64,
    pts0: Option<i64>,
    finished: Option<EncodedLength>,
}

impl AacEncoder {
    fn open(cfg: &AudioEncoderConfig) -> Result<Self, CoreError> {
        let refuse = |reason: &str| {
            CoreError::new(
                "CODEC_UNAVAILABLE",
                format!(
                    "vo-aacenc does not encode {} channels at {} Hz and {} b/s",
                    cfg.channels, cfg.rate, cfg.bitrate
                ),
            )
            .with_details(json!({
                "codec": "aac", "direction": "encode", "backend": ENCODER, "reason": reason,
                "rate": cfg.rate, "channels": cfg.channels, "bitrate": cfg.bitrate,
            }))
        };
        if !(1..=2).contains(&cfg.channels) {
            return Err(refuse("channels"));
        }
        let Some(asc) = lc_asc(cfg.rate, cfg.channels) else {
            return Err(refuse("rate"));
        };
        // vo-aacenc silently replaces a bitrate outside 8–160 kb/s a channel (and above six
        // bits a sample); refuse it instead, so the stream is what was asked for.
        let per = cfg.bitrate / cfg.channels as u32;
        if !(8_000..=160_000).contains(&per)
            || cfg.bitrate as u64 > cfg.rate as u64 * 6 * cfg.channels as u64
        {
            return Err(refuse("bitrate"));
        }
        let mut e = ptr::null_mut();
        // SAFETY: `e` is valid to write; the shim fills it on success.
        let r = unsafe {
            sys::fin_aacenc_open(
                cfg.rate as i32,
                cfg.channels as i32,
                cfg.bitrate as i32,
                &mut e,
            )
        };
        let enc = match (r, NonNull::new(e)) {
            (sys::OK, Some(e)) => Enc(e),
            (r, _) => return Err(failed("opening the encoder", r)),
        };
        let channels = cfg.channels as usize;
        let max = MAX_AU_PER_CHANNEL * channels;
        Ok(AacEncoder {
            enc,
            channels,
            config: CodecConfig::Aac {
                esds: esds(&asc, cfg.bitrate, max as u32),
            },
            pending: Vec::new(),
            packet: vec![0; max],
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
        let mut written = 0i32;
        // SAFETY: the encoder is open; `pending` holds at least `FRAME` frames of `channels`
        // samples (the shim reads exactly that many), `packet` has `max` bytes of room and
        // `written` is valid to write.
        let r = unsafe {
            sys::fin_aacenc_encode(
                self.enc.0.as_ptr(),
                self.pending.as_ptr(),
                self.packet.as_mut_ptr(),
                max,
                &mut written,
            )
        };
        if r != sys::OK {
            return Err(failed("encoding a frame", r));
        }
        let n = usize::try_from(written)
            .ok()
            .filter(|n| *n <= self.packet.len())
            .ok_or_else(|| failed("encoding a frame", sys::OUTPUT_BUFFER_SMALL))?;
        self.pending.drain(..FRAME * self.channels);
        let pts = self.pts0.unwrap_or(0) + (self.packets * FRAME as u64) as i64;
        self.packets += 1;
        self.out.push_back(EncodedPacket {
            data: self.packet[..n].to_vec(),
            pts,
            dts: pts,
            keyframe: true,
        });
        Ok(())
    }
}

/// `x` as 16-bit PCM: `round(x × 32767)`, clamped.
fn to_i16(x: f32) -> i16 {
    (x as f64 * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}

impl AudioEncoder for AacEncoder {
    fn send(&mut self, pcm: &[f32], pts: i64) -> Result<(), CoreError> {
        if self.finished.is_some() {
            return Err(encode_failed(
                "samples after the end",
                json!({ "backend": ENCODER }),
            ));
        }
        if !pcm.len().is_multiple_of(self.channels) {
            return Err(encode_failed(
                "a partial frame",
                json!({ "backend": ENCODER, "samples": pcm.len(), "channels": self.channels }),
            ));
        }
        if pcm.iter().any(|x| !x.is_finite()) {
            return Err(encode_failed(
                "a sample that is not a finite number",
                json!({ "backend": ENCODER, "reason": "nonFinite" }),
            ));
        }
        self.pts0.get_or_insert(pts);
        self.sent += (pcm.len() / self.channels) as u64;
        self.pending.extend(pcm.iter().map(|&x| to_i16(x)));
        while self.pending.len() >= FRAME * self.channels {
            self.encode_frame()?;
        }
        Ok(())
    }

    fn receive(&mut self) -> Result<Option<EncodedPacket>, CoreError> {
        Ok(self.out.pop_front())
    }

    /// Feeds zeros until every input sample is out past the delay (at least one packet).
    fn finish(&mut self) -> Result<EncodedLength, CoreError> {
        if let Some(len) = self.finished {
            return Ok(len);
        }
        let priming = PRIMING as u64;
        let total = (priming + self.sent).div_ceil(FRAME as u64).max(1);
        while self.packets < total {
            let missing = FRAME * self.channels - self.pending.len().min(FRAME * self.channels);
            self.pending.extend(std::iter::repeat_n(0, missing));
            self.encode_frame()?;
        }
        let len = EncodedLength {
            priming: PRIMING,
            valid: self.sent,
            padding: (total * FRAME as u64 - priming - self.sent) as u32,
        };
        self.finished = Some(len);
        Ok(len)
    }

    fn config(&self) -> CodecConfig {
        self.config.clone()
    }

    fn priming(&self) -> u32 {
        PRIMING
    }

    fn frame_size(&self) -> u32 {
        FRAME as u32
    }
}

/// vo-aacenc in the audio encoder registry (Linux only).
pub(crate) struct VoAacEnc;

impl AudioEncoderBackend for VoAacEnc {
    fn entry(&self) -> &'static AudioBackendEntry {
        audio_backend(ENCODER).expect("AUDIO_BACKENDS lists vo-aacenc on Linux")
    }

    fn encodes(&self, cfg: &AudioEncoderConfig) -> bool {
        cfg.codec == AudioCodec::Aac
    }

    fn open(
        &self,
        cfg: &AudioEncoderConfig,
        _: AudioOpenToken,
    ) -> Result<Box<dyn AudioEncoder>, CoreError> {
        Ok(Box::new(AacEncoder::open(cfg)?))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn samples_round_and_clamp() {
        assert_eq!(super::to_i16(0.5), 16384);
        assert_eq!(super::to_i16(-1.5), -32768);
        assert_eq!(super::to_i16(1.0), 32767);
    }
}
