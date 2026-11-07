//! Files written by the muxer for the demuxer tests.
#![allow(dead_code)]

use std::io::Cursor;

use fin_container::mux::knobs::{FileKnobs, TrackKnobs};
use fin_container::{AudioTrackConfig, CodecConfig, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::Rational;
use fin_media::{ColourTags, Rotation, Size};
use mp4_atom::{Atom, Av1c, Dops};

pub fn av1c() -> Vec<u8> {
    let c = Av1c {
        seq_level_idx_0: 8,
        chroma_subsampling_x: true,
        chroma_subsampling_y: true,
        chroma_sample_position: 1,
        ..Default::default()
    };
    let mut body = Vec::new();
    c.encode_body(&mut body).unwrap();
    body
}

pub fn dops(pre_skip: u16) -> Vec<u8> {
    let d = Dops {
        output_channel_count: 2,
        pre_skip,
        input_sample_rate: 48_000,
        output_gain: 0,
        channel_mapping: None,
    };
    let mut body = Vec::new();
    d.encode_body(&mut body).unwrap();
    body
}

pub fn video(rotation: Rotation) -> VideoTrackConfig {
    VideoTrackConfig {
        config: CodecConfig::Av1 { av1c: av1c() },
        coded: Size::new(320, 180),
        timescale: 30_000,
        rotation,
        pixel_aspect: Rational::from_int(1),
        colour: ColourTags::BT709,
        hdr: None,
    }
}

pub fn data(i: u32) -> Vec<u8> {
    (0..(20 + i % 13)).map(|k| (i * 31 + k) as u8).collect()
}

/// `frames` as `(pts, dts, duration, key)` in one video track.
pub fn write(
    cfg: VideoTrackConfig,
    knobs: TrackKnobs,
    file: FileKnobs,
    frames: &[(i64, i64, u32, bool)],
) -> Vec<u8> {
    let opts = Mp4Options {
        knobs: file,
        ..Default::default()
    };
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), opts).unwrap();
    let v = w.add_video(cfg).unwrap();
    w.set_knobs(v, knobs);
    for (i, &(pts, dts, d, key)) in frames.iter().enumerate() {
        w.write(v, &data(i as u32), pts, dts, d, key).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// `n` CFR frames of 1001 ticks at 30000, a keyframe every 30.
pub fn cfr_frames(n: u32) -> Vec<(i64, i64, u32, bool)> {
    (0..n)
        .map(|i| (i as i64 * 1001, i as i64 * 1001, 1001, i % 30 == 0))
        .collect()
}

pub fn cfr(n: u32) -> Vec<u8> {
    write(
        video(Rotation::R0),
        TrackKnobs::default(),
        FileKnobs::default(),
        &cfr_frames(n),
    )
}

/// Video plus Opus audio with `priming` samples of encoder delay.
pub fn with_audio(n: u32, priming: u32) -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w.add_video(video(Rotation::R0)).unwrap();
    let a = w
        .add_audio(AudioTrackConfig {
            config: CodecConfig::Opus {
                dops: dops(priming as u16),
            },
            sample_rate: 48_000,
            channels: 2,
            priming,
        })
        .unwrap();
    for i in 0..n {
        let t = i as i64 * 1001;
        w.write(v, &data(i), t, t, 1001, i % 30 == 0).unwrap();
        for k in 0..2 {
            let s = (i * 2 + k) as i64 * 800;
            w.write(a, &data(5000 + i * 2 + k), s, s, 800, true)
                .unwrap();
        }
    }
    w.finish().unwrap().into_inner()
}

/// Every box's start and end offset, descending into the containers the demuxer reads.
pub fn boundaries(bytes: &[u8]) -> Vec<usize> {
    fn walk(b: &[u8], base: usize, out: &mut Vec<usize>) {
        let mut at = 0;
        while at + 8 <= b.len() {
            let size = u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as usize;
            let (head, size) = match size {
                1 => (
                    16,
                    u64::from_be_bytes(b[at + 8..at + 16].try_into().unwrap()) as usize,
                ),
                0 => (8, b.len() - at),
                n => (8, n),
            };
            if size < head || at + size > b.len() {
                return;
            }
            out.push(base + at);
            out.push(base + at + size);
            let kind = &b[at + 4..at + 8];
            let skip = match kind {
                b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"dinf" | b"mvex"
                | b"moof" | b"traf" => Some(0),
                b"stsd" => Some(8),
                b"av01" => Some(78),
                b"Opus" => Some(28),
                _ => None,
            };
            if let Some(s) = skip
                && head + s <= size
            {
                walk(&b[at + head + s..at + size], base + at + head + s, out);
            }
            at += size;
        }
    }
    let mut out = Vec::new();
    walk(bytes, 0, &mut out);
    out.sort_unstable();
    out.dedup();
    out
}

/// The offset of the first `kind` box's header.
pub fn find_box(bytes: &[u8], kind: &[u8; 4]) -> usize {
    bytes
        .windows(4)
        .position(|w| w == kind)
        .expect("box present")
        - 4
}
