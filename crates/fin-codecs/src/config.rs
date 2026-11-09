//! What a decoder or an encoder is opened with (plan 6B-1 Task 3).

use std::time::Duration;

use fin_container::VideoTrack;
use fin_core::Rational;
use fin_media::backend::EncodePurpose;
use fin_media::jobs::{AdapterId, Profile};
use fin_media::{Chroma, ColourTags, Size, VideoCodec};

use crate::av1::DecodeThreads;

/// The GPU a session is bound to: its numeric ids (what a report records) and, on Windows,
/// its LUID (what `MFT_ENUM_ADAPTER_LUID` and the D3D11 device take). Never serialised: the
/// LUID changes per boot and identifies the machine's adapter set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdapterBinding {
    pub id: AdapterId,
    pub luid: Option<u64>,
}

/// A stream as backend choice sees it: codec, profile and bit depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamDesc {
    pub codec: VideoCodec,
    pub profile: Option<Profile>,
    pub bit_depth: u8,
    pub chroma: Chroma,
}

impl StreamDesc {
    pub fn of(track: &VideoTrack) -> StreamDesc {
        let p = &track.params;
        StreamDesc {
            codec: p.codec,
            profile: Profile::of(p.codec, p.fourcc, p.profile, p.bit_depth, p.chroma),
            bit_depth: p.bit_depth,
            chroma: p.chroma,
        }
    }
}

/// Opening a decoder for `track` (its `config` record is the decoder's input).
#[derive(Debug, Clone, Copy)]
pub struct DecoderConfig<'a> {
    pub track: &'a VideoTrack,
    pub threads: DecodeThreads,
    /// Pictures of more pixels are `DECODE_FAILED` before the decoder allocates for them.
    pub max_pixels: u32,
    /// The compositor's adapter, which a hardware decoder binds to (arch §27.3).
    pub adapter: Option<AdapterBinding>,
}

impl<'a> DecoderConfig<'a> {
    /// Default threading, 6A's pixel limit, no adapter.
    pub fn new(track: &'a VideoTrack) -> DecoderConfig<'a> {
        DecoderConfig {
            track,
            threads: DecodeThreads::default(),
            max_pixels: crate::av1::MAX_PIXELS,
            adapter: None,
        }
    }

    pub fn stream(&self) -> StreamDesc {
        StreamDesc::of(self.track)
    }
}

/// Keyframe placement. No B-frames anywhere in 6B-1 (frame-accurate seeks, Decision 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gop {
    /// Frames between keyframes (1 with `intra_only`).
    pub key_every_frames: u32,
    /// Closed GOPs: no frame references across a keyframe.
    pub closed: bool,
    /// Every frame a keyframe.
    pub intra_only: bool,
}

impl Gop {
    /// The keyframe interval an encoder is given: 1 when intra-only, else at least 1.
    pub fn interval(&self) -> u32 {
        if self.intra_only {
            1
        } else {
            self.key_every_frames.max(1)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateControl {
    /// Constant quality (SVT-AV1 1…63; hardware encoders map it to their quality scale).
    Crf(u8),
    /// Variable bitrate around `bps`.
    Vbr { bps: u32 },
    /// Constant quantiser.
    Cqp(u8),
}

/// Opening an encoder. There is no `Default`: `purpose` is always written out, since it alone
/// decides which classes may serve it (`fin_media::backend::encoder_allowed`,
/// `scripts/check-encoder-call-sites.mjs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderConfig {
    pub codec: VideoCodec,
    pub profile: Option<Profile>,
    pub size: Size,
    /// Frames per second (VFR sources pass their nominal rate).
    pub rate: Rational,
    pub gop: Gop,
    pub rc: RateControl,
    pub purpose: EncodePurpose,
    /// A hardware encoder on this adapter only.
    pub adapter: Option<AdapterBinding>,
    /// The software encoders' speed preset (SVT-AV1 0 slowest … 13 fastest); hardware
    /// encoders ignore it.
    pub preset: Option<u8>,
    /// Software encoders' parallelism; 1 makes SVT-AV1's output depend only on its input.
    pub threads: u32,
    pub colour: ColourTags,
}

impl EncoderConfig {
    /// How long an encoder that has frames may go without producing a packet before it is
    /// abandoned (plan 6B-1 Decision 20): `max(2 s, 4 frame durations × 30)`.
    pub fn hang_deadline(&self) -> Duration {
        let (num, den) = (self.rate.num().max(1) as f64, self.rate.den().max(1) as f64);
        let frames = Duration::from_secs_f64(120.0 * den / num);
        frames.max(Duration::from_secs(2))
    }
}
