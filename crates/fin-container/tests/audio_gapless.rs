//! The gapless length contract of the MP4 muxer (6C plan Decision 22): `presented` is written
//! exactly as the audio edit's segment duration, the movie timescale is the least common
//! multiple of the audio rate and the video timescale when a file has audio, and the demuxer
//! reads back exactly the padding the encoder reported. Files without audio are unchanged.

mod common;

use std::io::Cursor;

use common::{cfr, cfr_frames, data, dops, video, write, write_sized};
use fin_container::mux::knobs::{FileKnobs, TrackKnobs};
use fin_container::{AudioTrackConfig, CodecConfig, Demuxer, Limits, Mp4Options, Mp4Writer};
use fin_media::Rotation;
use mp4_atom::{Any, Decode, Moov};

fn moov(bytes: &[u8]) -> Moov {
    let mut buf = bytes;
    while !buf.is_empty() {
        if let Any::Moov(m) = Any::decode(&mut buf).unwrap() {
            return m;
        }
    }
    panic!("no moov")
}

/// What an encoder of 960-sample packets reports for `valid` samples after `priming`:
/// the packet count and the padding (`packets × 960 − priming − valid`).
fn encoded(priming: u32, valid: u64) -> (u32, u32) {
    let packets = (priming as u64 + valid).div_ceil(960).max(1);
    (
        packets as u32,
        (packets * 960 - priming as u64 - valid) as u32,
    )
}

fn audio(priming: u32, presented: Option<u64>) -> AudioTrackConfig {
    AudioTrackConfig {
        config: CodecConfig::Opus {
            dops: dops(priming as u16),
        },
        sample_rate: 48_000,
        channels: 2,
        priming,
        presented,
    }
}

/// Video (30 frames at 1001/30000) beside Opus audio of `packets` packets.
fn with_video(priming: u32, presented: Option<u64>, packets: u32) -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w.add_video(video(Rotation::R0)).unwrap();
    let a = w.add_audio(audio(priming, presented)).unwrap();
    for i in 0..30u32 {
        let t = i as i64 * 1001;
        w.write(v, &data(i), t, t, 1001, i % 30 == 0).unwrap();
    }
    for k in 0..packets {
        let s = k as i64 * 960;
        w.write(a, &data(900 + k), s, s, 960, true).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn audio_only(priming: u32, presented: Option<u64>, packets: u32) -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let a = w.add_audio(audio(priming, presented)).unwrap();
    for k in 0..packets {
        let s = k as i64 * 960;
        w.write(a, &data(900 + k), s, s, 960, true).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn mux_edit_exact_with_video() {
    let (priming, valid) = (312u32, 48_001u64);
    let (packets, padding) = encoded(priming, valid);
    let bytes = with_video(priming, Some(valid), packets);
    let m = moov(&bytes);
    assert_eq!(m.mvhd.timescale, 240_000, "lcm(48000, 30000)");
    let elst = &m.trak[1]
        .edts
        .as_ref()
        .unwrap()
        .elst
        .as_ref()
        .unwrap()
        .entries;
    assert_eq!(elst.len(), 1);
    assert_eq!(elst[0].media_time, Some(priming as u64));
    assert_eq!(
        elst[0].segment_duration,
        valid * 5,
        "presented × 240000/48000"
    );
    // The video keeps its exact length in the finer movie timescale.
    assert_eq!(m.trak[0].tkhd.duration, 30 * 1001 * 8);
    let d = Demuxer::open(Cursor::new(bytes), &Limits::default()).unwrap();
    let a = d.info().audio.clone().unwrap();
    assert_eq!((a.priming, a.padding), (priming, padding));
}

#[test]
fn presented_lengths_read_back_exactly() {
    for valid in [0u64, 1, 959, 960, 961, 48_000, 48_001] {
        let (packets, padding) = encoded(312, valid);
        for bytes in [
            audio_only(312, Some(valid), packets),
            with_video(312, Some(valid), packets),
        ] {
            let d = Demuxer::open(Cursor::new(bytes), &Limits::default()).unwrap();
            let a = d.info().audio.clone().unwrap();
            assert_eq!((a.priming, a.padding), (312, padding), "valid {valid}");
            let total: u64 = d.audio().unwrap().sample_count() as u64 * 960;
            assert_eq!(total - a.priming as u64 - a.padding as u64, valid);
        }
    }
}

#[test]
fn audio_only_movie_timescale_is_the_rate() {
    let m = moov(&audio_only(312, Some(1000), 2));
    assert_eq!(m.mvhd.timescale, 48_000);
    let elst = &m.trak[0]
        .edts
        .as_ref()
        .unwrap()
        .elst
        .as_ref()
        .unwrap()
        .entries;
    assert_eq!(
        (elst[0].segment_duration, elst[0].media_time),
        (1000, Some(312))
    );
    // Without `presented`, the edit runs to the end of the encoded samples (as before).
    let m = moov(&audio_only(312, None, 2));
    let elst = &m.trak[0]
        .edts
        .as_ref()
        .unwrap()
        .elst
        .as_ref()
        .unwrap()
        .entries;
    assert_eq!(elst[0].segment_duration, 2 * 960 - 312);
}

#[test]
fn presented_without_priming_still_cuts_the_padding() {
    let m = moov(&audio_only(0, Some(1500), 2));
    let elst = &m.trak[0]
        .edts
        .as_ref()
        .unwrap()
        .elst
        .as_ref()
        .unwrap()
        .entries;
    assert_eq!(
        (elst[0].segment_duration, elst[0].media_time),
        (1500, Some(0))
    );
}

/// The movie timescale falls back to the audio rate when the LCM passes `u32::MAX`.
#[test]
fn lcm_overflow_falls_back_to_the_audio_rate() {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let mut cfg = video(Rotation::R0);
    cfg.timescale = 4_294_967_291; // prime
    let v = w.add_video(cfg).unwrap();
    let a = w.add_audio(audio(312, Some(600))).unwrap();
    w.write(v, &data(0), 0, 0, 1000, true).unwrap();
    w.write(a, &data(1), 0, 0, 960, true).unwrap();
    let m = moov(&w.finish().unwrap().into_inner());
    assert_eq!(m.mvhd.timescale, 48_000);
}

/// BLAKE3 of video-only files muxed before Decision 22, pinned: they must not move.
#[test]
fn mux_without_audio_unchanged() {
    let order = [0i64, 3, 1, 2, 6, 4, 5];
    let b_frames: Vec<_> = order
        .iter()
        .enumerate()
        .map(|(i, &p)| (p * 1001, (i as i64 - 1) * 1001, 1001, i % 4 == 0))
        .collect();
    let files = [
        ("cfr90", cfr(90)),
        (
            "b-frames",
            write(
                video(Rotation::R90),
                TrackKnobs::default(),
                FileKnobs::default(),
                &b_frames,
            ),
        ),
        (
            "empty-edit",
            write(
                video(Rotation::R0),
                TrackKnobs {
                    empty_edit: Some(1500),
                    ..Default::default()
                },
                FileKnobs::default(),
                &cfr_frames(10),
            ),
        ),
        (
            "sized",
            write_sized(
                video(Rotation::R270),
                TrackKnobs::default(),
                &cfr_frames(45),
                100,
            ),
        ),
    ];
    let got: Vec<(&str, String)> = files
        .iter()
        .map(|(n, b)| (*n, blake3::hash(b).to_hex().to_string()))
        .collect();
    let want = [
        (
            "cfr90",
            "3c4e7f8d1d749a292b5eedc0b0ab6f550178657719813d88b11ba4098318f1d1",
        ),
        (
            "b-frames",
            "04e0b600fa611ac9a3a8235fff788b77a4bde1e327eeed97137e789d37ffdbc2",
        ),
        (
            "empty-edit",
            "33cd12722c9ed21020d0797ad98adac1f18d64e0ca7f2fbbd31a2f7d9789620d",
        ),
        (
            "sized",
            "7e4c21e274769fe2da56c96366b35527571a2f573655f731958bdd914fa9d98e",
        ),
    ];
    let want: Vec<(&str, String)> = want.iter().map(|(n, h)| (*n, h.to_string())).collect();
    assert_eq!(got, want);
}
