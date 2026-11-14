//! Pattern clips as AV1 in MP4 (spec §13.3): frame-id patterns encoded with SVT-AV1 and muxed by
//! `fin-container`, for tests, `make_fixture` and the e2e scenario. Feature `testing`.

use std::io::Cursor;

use fin_container::{Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::{CoreError, Rational};
use fin_media::backend::EncodePurpose;
use fin_media::testing::pattern;
use fin_media::{ColourTags, HdrInfo, Rotation, Size, VideoCodec};

use crate::config::{EncoderConfig, Gop, RateControl};
use crate::frame::{EncodedPacket, RawFrame};
use crate::hung::{EncoderSession, Fed, Output};
use crate::registry::{ProbeResult, open_encoder};

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
    /// The colour tags the stream and its `colr` box carry (the pixels are the pattern's either
    /// way: a PQ- or HLG-tagged clip is an HDR source to every reader).
    pub colour: ColourTags,
    /// `mdcv`/`clli` boxes.
    pub hdr: Option<HdrInfo>,
    /// An edit list (`(movie ticks, media time or None)`, in the track's ticks) in place of the
    /// computed one.
    pub edits: Option<Vec<(u64, Option<u64>)>>,
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
            colour: ColourTags::BT709,
            hdr: None,
            edits: None,
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

/// The encoder config for a pattern clip of `codec` (test fixtures only).
pub fn fixture_config(spec: &ClipSpec, codec: VideoCodec) -> EncoderConfig {
    EncoderConfig {
        codec,
        profile: None,
        size: spec.size,
        rate: spec.rate,
        gop: Gop {
            key_every_frames: spec.keyint,
            closed: true,
            intra_only: false,
        },
        rc: RateControl::Crf(spec.crf),
        purpose: EncodePurpose::TestFixture,
        adapter: None,
        preset: Some(spec.preset),
        threads: 1,
        colour: spec.colour,
    }
}

/// Sends every frame `frame(i)` gives through `enc` and returns the packets in output order,
/// starting again from frame 0 whenever the session restarts on another encoder
/// (`ENCODER_HUNG`'s fallback).
pub fn encode_all(
    enc: &mut EncoderSession,
    frames: u32,
    mut frame: impl FnMut(u32) -> RawFrame,
) -> Result<Vec<EncodedPacket>, CoreError> {
    'again: loop {
        let mut packets = Vec::new();
        for i in 0..frames {
            if enc.send(frame(i), i as i64, false)? == Fed::Restart {
                continue 'again;
            }
            loop {
                match enc.receive()? {
                    Output::Packet(p) => packets.push(p),
                    Output::Pending | Output::End => break,
                    Output::Restart => continue 'again,
                }
            }
        }
        if enc.finish()? == Fed::Restart {
            continue 'again;
        }
        loop {
            match enc.receive()? {
                Output::Packet(p) => packets.push(p),
                Output::End => return Ok(packets),
                Output::Pending => {}
                Output::Restart => continue 'again,
            }
        }
    }
}

/// Encodes `spec.frames` pattern frames (`pattern(i, …)` carries frame id `i`) as AV1 with
/// SVT-AV1 and muxes them.
pub fn pattern_clip(spec: &ClipSpec) -> Result<Vec<u8>, CoreError> {
    let mut enc = open_encoder(
        &fixture_config(spec, VideoCodec::Av1),
        &ProbeResult::software_only(),
    )?;
    let (timescale, tick) = timing(spec.rate);
    let mut pts = Vec::with_capacity(spec.frames as usize + 1);
    let mut t = 0i64;
    for i in 0..spec.frames {
        pts.push(t);
        t += tick as i64
            * if spec.vfr {
                VFR[i as usize % VFR.len()]
            } else {
                1
            };
    }
    let packets = encode_all(&mut enc, spec.frames, |i| {
        let p = pattern(i, spec.size.width, spec.size.height, spec.seed);
        RawFrame::i420(spec.size, p.y, p.u, p.v)
    })?;
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default())?;
    let track = w.add_video(VideoTrackConfig {
        config: enc.config()?,
        coded: spec.size,
        timescale,
        rotation: spec.rotation,
        pixel_aspect: Rational::from_int(1),
        colour: spec.colour,
        hdr: spec.hdr,
    })?;
    if let Some(edits) = &spec.edits {
        w.set_knobs(
            track,
            fin_container::mux::knobs::TrackKnobs {
                edits: Some(edits.clone()),
                ..Default::default()
            },
        );
    }
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
