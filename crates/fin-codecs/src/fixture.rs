//! Pattern clips as AV1 in MP4 (spec §13.3): frame-id patterns encoded with SVT-AV1 and muxed by
//! `fin-container`, for tests, `make_fixture` and the e2e scenario. Feature `testing`.

use std::io::Cursor;

use fin_container::{Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::{CoreError, Rational};
use fin_media::testing::pattern;
use fin_media::{ColourTags, Rotation, Size};

use crate::av1::{Av1EncoderConfig, open_av1_encoder};
use crate::frame::RawFrame;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipSpec {
    pub frames: u32,
    pub size: Size,
    /// Frames per second (the nominal rate of a VFR clip).
    pub rate: Rational,
    /// Durations cycle through 1, 1, 2, 1, 3 frame lengths.
    pub vfr: bool,
    pub rotation: Rotation,
    pub keyint: u32,
    pub crf: u8,
    pub preset: u8,
    pub seed: u64,
}

impl Default for ClipSpec {
    fn default() -> Self {
        ClipSpec {
            frames: 30,
            size: Size::new(320, 180),
            rate: Rational::from_int(30),
            vfr: false,
            rotation: Rotation::R0,
            keyint: 30,
            crf: 30,
            preset: 10,
            seed: 1,
        }
    }
}

/// The frame duration multipliers of a VFR clip.
const VFR: [i64; 5] = [1, 1, 2, 1, 3];

/// Ticks per frame and the timescale for `rate`: `rate.den × 1000` ticks a frame at
/// `rate.num × 1000` ticks a second.
pub fn timing(rate: Rational) -> (u32, u32) {
    ((rate.num() * 1000) as u32, (rate.den() * 1000) as u32)
}

/// Encodes `spec.frames` pattern frames (`pattern(i, …)` carries frame id `i`) and muxes them.
pub fn pattern_clip(spec: &ClipSpec) -> Result<Vec<u8>, CoreError> {
    let cfg = Av1EncoderConfig {
        size: spec.size,
        rate: spec.rate,
        preset: spec.preset,
        crf: spec.crf,
        keyint: spec.keyint,
        threads: 1,
        colour: ColourTags::BT709,
    };
    let mut enc = open_av1_encoder(&cfg)?;
    let (timescale, tick) = timing(spec.rate);
    let mut pts = Vec::with_capacity(spec.frames as usize);
    let mut t = 0i64;
    let mut packets = Vec::new();
    for i in 0..spec.frames {
        let p = pattern(i, spec.size.width, spec.size.height, spec.seed);
        pts.push(t);
        t += tick as i64
            * if spec.vfr {
                VFR[i as usize % VFR.len()]
            } else {
                1
            };
        enc.send(&RawFrame::i420(spec.size, p.y, p.u, p.v), i as i64, false)?;
        while let Some(pk) = enc.receive()? {
            packets.push(pk);
        }
    }
    enc.finish()?;
    while let Some(pk) = enc.receive()? {
        packets.push(pk);
    }
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default())?;
    let track = w.add_video(VideoTrackConfig {
        config: enc.config(),
        coded: spec.size,
        timescale,
        rotation: spec.rotation,
        pixel_aspect: Rational::from_int(1),
        colour: ColourTags::BT709,
        hdr: None,
    })?;
    pts.push(t);
    // AV1 temporal units come out in presentation order, one per input frame; the packet's
    // pts is the frame index it was sent with.
    for pk in &packets {
        let i = pk.pts as usize;
        let (at, next) = (pts[i], pts[i + 1]);
        w.write(track, &pk.data, at, at, (next - at) as u32, pk.keyframe)?;
    }
    Ok(w.finish()?.into_inner())
}
