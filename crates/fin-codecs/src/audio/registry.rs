//! Audio backend choice (6C plan Decisions 9, 10 and 24): which backend decodes a track and which
//! encoders may serve an encode. [`open_audio_encoder`] filters its candidates through
//! `fin_media::backend_audio::audio_encoder_allowed`, the one audio policy function, and opens
//! them with an [`AudioOpenToken`] that only this file makes: **no other code opens an audio
//! encoder** (`scripts/check-encoder-call-sites.mjs`).

use fin_container::CodecConfig;
use fin_core::CoreError;
use fin_media::backend::EncodePurpose;
use fin_media::backend_audio::{AudioBackendEntry, audio_backend, audio_encoder_allowed};
use fin_media::{AudioCodec, AudioTrackInfo};
use serde_json::json;
use symphonia::core::codecs::audio::AudioCodecParameters;

use super::opus::{LibOpus, OpusDecoder};
use super::opus_head::OpusHeader;
use super::symphonia::{SymphoniaDecoder, channels_for, id_of};
use super::traits::{AudioConfig, AudioDecoder, AudioEncoder, AudioEncoderConfig};
use crate::registry::unavailable;

/// What only this file can make: an audio encoder backend's `open` takes one.
pub struct AudioOpenToken(pub(crate) ());

/// An audio encoder this build has (Tasks 13–16 add libopus, vo-aacenc and the OS encoders).
pub(crate) trait AudioEncoderBackend: Send + Sync {
    fn entry(&self) -> &'static AudioBackendEntry;
    fn encodes(&self, cfg: &AudioEncoderConfig) -> bool;
    fn open(
        &self,
        cfg: &AudioEncoderConfig,
        token: AudioOpenToken,
    ) -> Result<Box<dyn AudioEncoder>, CoreError>;
}

/// The audio encoders, in preference order (OS before software, as for video).
static ENCODERS: &[&dyn AudioEncoderBackend] = &[&LibOpus];

pub(crate) fn codec_name(codec: AudioCodec) -> String {
    serde_json::to_value(codec)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// `CODEC_UNAVAILABLE` for a codec whose MP4 sample entry carries a configuration this build
/// does not read yet.
fn needs_config(codec: AudioCodec) -> CoreError {
    unavailable(&codec_name(codec), "decode").with_details(json!({
        "codec": codec_name(codec), "direction": "decode", "os": std::env::consts::OS,
        "reason": "config",
    }))
}

/// `Ok` when symphonia's table row lists `codec` (on this target), else `CODEC_UNAVAILABLE`.
pub(crate) fn symphonia_decodes(codec: AudioCodec) -> Result<(), CoreError> {
    match audio_backend("symphonia") {
        Some(b) if b.decode.contains(&codec) => Ok(()),
        _ => Err(unavailable(&codec_name(codec), "decode")),
    }
}

/// A decoder for a track: libopus for Opus (from the MP4 `dOps` or the Ogg `OpusHead`);
/// symphonia for the codecs its table row lists (from symphonia's own parameters, or for MP3
/// in MP4 from the track's rate and channels); `CODEC_UNAVAILABLE` otherwise (AAC arrives
/// with Tasks 14–16).
pub fn open_audio_decoder(
    info: &AudioTrackInfo,
    config: &AudioConfig,
) -> Result<Box<dyn AudioDecoder>, CoreError> {
    if info.codec == AudioCodec::Opus {
        let header = match config {
            AudioConfig::Record(CodecConfig::Opus { dops }) => OpusHeader::from_dops(dops)?,
            AudioConfig::Symphonia(p) => match p.0.extra_data.as_deref() {
                Some(head) => OpusHeader::from_opus_head(head)?,
                None => return Err(needs_config(info.codec)),
            },
            AudioConfig::Record(_) => return Err(needs_config(info.codec)),
        };
        return Ok(Box::new(OpusDecoder::open(&header)?));
    }
    symphonia_decodes(info.codec)?;
    match config {
        AudioConfig::Symphonia(p) => Ok(Box::new(SymphoniaDecoder::open(&p.0)?)),
        AudioConfig::Record(CodecConfig::Other { .. }) => {
            let Some(id) = id_of(info.codec) else {
                return Err(needs_config(info.codec));
            };
            let mut p = AudioCodecParameters::new();
            p.for_codec(id).with_sample_rate(info.sample_rate);
            if let Some(ch) = channels_for(info.channels) {
                p.with_channels(ch);
            }
            Ok(Box::new(SymphoniaDecoder::open(&p)?))
        }
        AudioConfig::Record(_) => Err(needs_config(info.codec)),
    }
}

/// An audio encoder for `cfg`: the backends that encode it, filtered by
/// [`audio_encoder_allowed`] for `purpose`, tried in order (one that fails to open gives way to
/// the next). `CODEC_UNAVAILABLE` when none encodes the codec, or (with the policy's reason)
/// when the policy refuses every one.
pub fn open_audio_encoder(
    cfg: &AudioEncoderConfig,
    purpose: EncodePurpose,
) -> Result<Box<dyn AudioEncoder>, CoreError> {
    open_from(ENCODERS, cfg, purpose)
}

fn open_from(
    backends: &[&dyn AudioEncoderBackend],
    cfg: &AudioEncoderConfig,
    purpose: EncodePurpose,
) -> Result<Box<dyn AudioEncoder>, CoreError> {
    let candidates: Vec<_> = backends.iter().filter(|b| b.encodes(cfg)).collect();
    let Some(first) = candidates.first() else {
        return Err(unavailable(&codec_name(cfg.codec), "encode"));
    };
    let mut last = audio_encoder_allowed(first.entry().class, cfg.codec, purpose)
        .err()
        .map(|r| r.to_error(first.entry().name));
    for b in candidates {
        if audio_encoder_allowed(b.entry().class, cfg.codec, purpose).is_err() {
            continue;
        }
        match b.open(cfg, AudioOpenToken(())) {
            Ok(enc) => return Ok(enc),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| unavailable(&codec_name(cfg.codec), "encode")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::traits::EncodedLength;
    use crate::frame::EncodedPacket;
    use fin_media::backend::VendorClass;

    static GPU: AudioBackendEntry = AudioBackendEntry {
        name: "hypothetical-gpu",
        class: VendorClass::Gpu,
        decode: &[],
        encode: &[AudioCodec::Aac],
    };

    struct Fake;
    struct Enc;

    impl AudioEncoder for Enc {
        fn send(&mut self, _: &[f32], _: i64) -> Result<(), CoreError> {
            Ok(())
        }
        fn receive(&mut self) -> Result<Option<EncodedPacket>, CoreError> {
            Ok(None)
        }
        fn finish(&mut self) -> Result<EncodedLength, CoreError> {
            Ok(EncodedLength {
                priming: 0,
                valid: 0,
                padding: 0,
            })
        }
        fn config(&self) -> CodecConfig {
            CodecConfig::Other { fourcc: *b"test" }
        }
        fn priming(&self) -> u32 {
            0
        }
        fn frame_size(&self) -> u32 {
            1024
        }
    }

    impl AudioEncoderBackend for Fake {
        fn entry(&self) -> &'static AudioBackendEntry {
            &GPU
        }
        fn encodes(&self, cfg: &AudioEncoderConfig) -> bool {
            cfg.codec == AudioCodec::Aac
        }
        fn open(
            &self,
            _: &AudioEncoderConfig,
            _: AudioOpenToken,
        ) -> Result<Box<dyn AudioEncoder>, CoreError> {
            Ok(Box::new(Enc))
        }
    }

    fn cfg(codec: AudioCodec) -> AudioEncoderConfig {
        AudioEncoderConfig {
            codec,
            rate: 48_000,
            channels: 2,
            bitrate: 192_000,
        }
    }

    #[test]
    fn policy_filters_the_candidates() {
        let e = open_from(&[&Fake], &cfg(AudioCodec::Aac), EncodePurpose::Export)
            .err()
            .expect("a gpu encoder is refused for an export");
        assert_eq!(e.code, "CODEC_UNAVAILABLE");
        assert_eq!(e.details["reason"], "gpuForDistributed");
        assert!(open_from(&[&Fake], &cfg(AudioCodec::Aac), EncodePurpose::Proxy).is_ok());
        let e = open_from(&[&Fake], &cfg(AudioCodec::Opus), EncodePurpose::Proxy)
            .err()
            .expect("nothing encodes Opus here");
        assert_eq!(e.details["direction"], "encode");
    }

    #[test]
    fn libopus_is_the_opus_encoder() {
        let e = open_audio_encoder(&cfg(AudioCodec::Opus), EncodePurpose::Proxy)
            .expect("libopus encodes Opus");
        assert_eq!(e.frame_size(), 960);
        let e = open_audio_encoder(&cfg(AudioCodec::Mp3), EncodePurpose::Proxy)
            .err()
            .expect("nothing encodes MP3");
        assert_eq!(e.code, "CODEC_UNAVAILABLE");
    }
}
