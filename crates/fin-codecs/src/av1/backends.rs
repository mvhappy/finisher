//! dav1d and SVT-AV1 as registry backends (class `software`, plan 6B-1 Task 3).

use fin_core::CoreError;
use fin_media::VideoCodec;
use fin_media::backend::BackendEntry;
use fin_media::jobs::Profile;
use serde_json::json;

use super::decode::Dav1dDecoder;
use super::svt::{Av1EncoderConfig, open_av1_encoder};
use crate::config::{DecoderConfig, EncoderConfig, RateControl, StreamDesc};
use crate::registry::{DecoderBackend, EncoderBackend, OpenToken, entry};
use crate::traits::{VideoDecoder, VideoEncoder};

/// dav1d: every AV1 profile and depth (the compositor reads what it can, spec §5.1).
pub struct Dav1dBackend;

impl DecoderBackend for Dav1dBackend {
    fn entry(&self) -> &'static BackendEntry {
        entry("dav1d")
    }

    fn hw(&self) -> bool {
        false
    }

    fn decodes(&self, s: &StreamDesc) -> bool {
        s.codec == VideoCodec::Av1
    }

    fn open(&self, cfg: &DecoderConfig) -> Result<Box<dyn VideoDecoder>, CoreError> {
        Ok(Box::new(Dav1dDecoder::with_limit(
            cfg.threads,
            cfg.max_pixels,
        )?))
    }
}

/// SVT-AV1: 8-bit 4:2:0 (Main) at a constant quality; closed GOPs (SVT-AV1's default
/// keyframe refresh), keyframes every `gop.interval()` frames.
pub struct SvtBackend;

/// The SVT-AV1 preset when the config names none (spec §7.1's proxy setting).
pub const DEFAULT_PRESET: u8 = 10;

impl EncoderBackend for SvtBackend {
    fn entry(&self) -> &'static BackendEntry {
        entry("svt-av1")
    }

    fn hw(&self) -> bool {
        false
    }

    fn encodes(&self, codec: VideoCodec, profile: Option<Profile>) -> bool {
        codec == VideoCodec::Av1 && matches!(profile, None | Some(Profile::Main))
    }

    fn open(&self, cfg: &EncoderConfig, _: OpenToken) -> Result<Box<dyn VideoEncoder>, CoreError> {
        let RateControl::Crf(crf) = cfg.rc else {
            return Err(CoreError::new(
                "ENCODE_FAILED",
                "SVT-AV1 encodes at a constant quality (Crf) only",
            )
            .with_details(json!({ "backend": "svt-av1", "reason": "rateControl" })));
        };
        open_av1_encoder(&Av1EncoderConfig {
            size: cfg.size,
            rate: cfg.rate,
            preset: cfg.preset.unwrap_or(DEFAULT_PRESET),
            crf,
            keyint: cfg.gop.interval(),
            threads: cfg.threads,
            colour: cfg.colour,
        })
    }
}
