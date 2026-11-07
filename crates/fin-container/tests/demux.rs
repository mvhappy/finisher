//! The demuxer against files from the muxer: every field round-trips (CFR, VFR, B-frames with
//! `ctts`, an empty edit, fragments, rotations, pixel aspect, colour and HDR boxes, audio
//! priming), plus real footage from `FIN_MEDIA_FIXTURES` when it is there.

mod common;

use std::io::Cursor;

use common::*;
use fin_container::mux::knobs::{ColrKnob, FileKnobs, TrackKnobs};
use fin_container::{CodecConfig, Demuxer, Limits};
use fin_core::Rational;
use fin_media::{
    AudioCodec, ChromaSiting, ColourTags, ContainerKind, ContentLight, DolbyVision, HdrInfo,
    MasteringDisplay, Matrix, Primaries, Range, Rotation, Size, Transfer, VideoCodec, decode_start,
    frame_at,
};

fn open(bytes: Vec<u8>) -> Demuxer<Cursor<Vec<u8>>> {
    Demuxer::open(Cursor::new(bytes), &Limits::default()).unwrap()
}

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

#[test]
fn cfr_round_trips_every_field() {
    let mut d = open(cfr(90));
    let info = d.info().clone();
    assert_eq!(info.container, ContainerKind::Mp4);
    assert!(!info.fragmented);
    assert_eq!(info.duration, r(90 * 1001, 30_000));
    let v = info.video.as_ref().unwrap();
    assert_eq!((v.codec, v.fourcc.as_str()), (VideoCodec::Av1, "av01"));
    assert_eq!((v.profile, v.level, v.bit_depth), (Some(0), Some(8), 8));
    assert_eq!(
        (v.coded, v.display, v.rotation),
        (Size::new(320, 180), Size::new(320, 180), Rotation::R0)
    );
    assert_eq!(v.pixel_aspect, r(1, 1));
    assert_eq!(
        v.colour,
        ColourTags {
            siting: ChromaSiting::Left,
            ..ColourTags::BT709
        }
    );
    assert_eq!(
        (v.nominal_rate, v.vfr, v.frames),
        (r(30_000, 1001), false, 90)
    );
    assert!(v.hdr.is_none() && !v.icc);
    assert!(info.audio.is_none());
    let track = d.video().unwrap().clone();
    assert!(matches!(track.config, CodecConfig::Av1 { ref av1c } if *av1c == common::av1c()));
    let t = &track.table;
    assert_eq!(t.timescale(), 30_000);
    for i in 0..90 {
        let f = t.frames()[i];
        assert_eq!(
            (f.pts, f.duration, f.decode_index, f.keyframe),
            (i as i64 * 1001, 1001, i as u32, i % 30 == 0)
        );
        assert_eq!(frame_at(t, t.pts_seconds(i)).index, i);
        assert_eq!(decode_start(t, i), i / 30 * 30);
        let mut buf = Vec::new();
        d.read_sample(track.track, i as u32, &mut buf).unwrap();
        assert_eq!(buf, data(i as u32));
    }
    assert_eq!(
        d.read_sample(track.track, 90, &mut Vec::new())
            .unwrap_err()
            .code,
        "INVALID_RANGE"
    );
}

#[test]
fn vfr_is_detected_and_the_nominal_rate_is_the_most_common() {
    let durations = [1001u32, 1001, 1001, 2002, 1001, 1500, 1001, 1001];
    let mut t = 0;
    let frames: Vec<_> = durations
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            let f = (t, t, d, i == 0);
            t += d as i64;
            f
        })
        .collect();
    let d = open(write(
        video(Rotation::R0),
        TrackKnobs::default(),
        FileKnobs::default(),
        &frames,
    ));
    let v = d.info().video.clone().unwrap();
    assert!(v.vfr);
    assert_eq!(v.nominal_rate, r(30_000, 1001));
    // The last frame's duration does not count: a CFR file with a short last frame is CFR.
    let mut cfr = cfr_frames(10);
    cfr[9].2 = 500;
    let d = open(write(
        video(Rotation::R0),
        TrackKnobs::default(),
        FileKnobs::default(),
        &cfr,
    ));
    assert!(!d.info().video.as_ref().unwrap().vfr);
}

#[test]
fn b_frames_ctts_and_the_edit_list_present_in_order() {
    let order = [0i64, 3, 1, 2, 6, 4, 5];
    let frames: Vec<_> = order
        .iter()
        .enumerate()
        .map(|(i, &p)| (p * 1001, (i as i64 - 1) * 1001, 1001, i % 4 == 0))
        .collect();
    let d = open(write(
        video(Rotation::R0),
        TrackKnobs::default(),
        FileKnobs::default(),
        &frames,
    ));
    let t = &d.video().unwrap().table;
    let pts: Vec<i64> = t.frames().iter().map(|f| f.pts).collect();
    assert_eq!(
        pts,
        (0..7).map(|p| p * 1001).collect::<Vec<_>>(),
        "the edit list moves the first pts to 0"
    );
    let decode: Vec<u32> = t.frames().iter().map(|f| f.decode_index).collect();
    assert_eq!(decode, [0, 2, 3, 1, 5, 6, 4]);
    // Frame 6 (decode 4) is the second keyframe; frames 4 and 5 lead it and need GOP 0.
    let starts: Vec<usize> = (0..7).map(|i| decode_start(t, i)).collect();
    assert_eq!(starts, [0, 0, 0, 0, 0, 0, 4]);
    assert_eq!(d.info().duration, r(7 * 1001, 30_000));
}

#[test]
fn an_empty_edit_delays_the_media() {
    let d = open(write(
        video(Rotation::R0),
        TrackKnobs {
            empty_edit: Some(1500),
            ..Default::default()
        },
        FileKnobs::default(),
        &cfr_frames(10),
    ));
    let t = &d.video().unwrap().table;
    assert_eq!(t.frames()[0].pts, 1500);
    assert!(frame_at(t, r(1, 30_000)).out_of_range);
    assert_eq!(frame_at(t, r(1500 + 1001, 30_000)).index, 1);
}

#[test]
fn fragmented_files_append_every_fragment() {
    let mut d = open(write(
        video(Rotation::R0),
        TrackKnobs::default(),
        FileKnobs {
            fragment_every: Some(10),
        },
        &cfr_frames(25),
    ));
    assert!(d.info().fragmented);
    let track = d.video().unwrap().clone();
    assert_eq!(track.table.len(), 25);
    assert_eq!(track.table.frames()[24].pts, 24 * 1001);
    let keys: Vec<usize> = track
        .table
        .frames()
        .iter()
        .enumerate()
        .filter(|(_, f)| f.keyframe)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(keys, [0]);
    for i in [0u32, 9, 10, 24] {
        let mut buf = Vec::new();
        d.read_sample(track.track, i, &mut buf).unwrap();
        assert_eq!(buf, data(i));
    }
}

#[test]
fn rotations_and_pixel_aspect_give_the_display_size() {
    for (rot, display) in [
        (Rotation::R90, Size::new(180, 320)),
        (Rotation::R180, Size::new(320, 180)),
        (Rotation::R270, Size::new(180, 320)),
    ] {
        let d = open(write(
            video(rot),
            TrackKnobs::default(),
            FileKnobs::default(),
            &cfr_frames(3),
        ));
        let v = d.info().video.clone().unwrap();
        assert_eq!(
            (v.rotation, v.display, v.coded),
            (rot, display, Size::new(320, 180))
        );
    }
    let mut cfg = video(Rotation::R90);
    cfg.pixel_aspect = r(4, 3);
    let v = open(write(
        cfg,
        TrackKnobs::default(),
        FileKnobs::default(),
        &cfr_frames(3),
    ))
    .info()
    .video
    .clone()
    .unwrap();
    assert_eq!((v.pixel_aspect, v.display), (r(4, 3), Size::new(180, 426)));
    // Any other matrix is refused.
    let m = [0x10000, 0x8000, 0, -0x8000, 0x10000, 0, 0, 0, 0x4000_0000];
    let bytes = write(
        video(Rotation::R0),
        TrackKnobs {
            matrix: Some(m),
            ..Default::default()
        },
        FileKnobs::default(),
        &cfr_frames(3),
    );
    let e = Demuxer::open(Cursor::new(bytes), &Limits::default())
        .err()
        .unwrap();
    assert_eq!(e.code, "UNSUPPORTED_TRANSFORM");
    let mirror = [-0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];
    let bytes = write(
        video(Rotation::R0),
        TrackKnobs {
            matrix: Some(mirror),
            ..Default::default()
        },
        FileKnobs::default(),
        &cfr_frames(3),
    );
    assert_eq!(
        Demuxer::open(Cursor::new(bytes), &Limits::default())
            .err()
            .unwrap()
            .code,
        "UNSUPPORTED_TRANSFORM"
    );
}

#[test]
fn colour_variants_and_hlg_with_mastering_metadata() {
    let none = open(write(
        video(Rotation::R0),
        TrackKnobs {
            colr: ColrKnob::None,
            ..Default::default()
        },
        FileKnobs::default(),
        &cfr_frames(2),
    ));
    let v = none.info().video.clone().unwrap();
    assert_eq!(
        (v.colour.primaries, v.colour.matrix, v.icc),
        (Primaries::Unknown, Matrix::Unknown, false)
    );
    let icc = open(write(
        video(Rotation::R0),
        TrackKnobs {
            colr: ColrKnob::Icc(vec![0; 16]),
            ..Default::default()
        },
        FileKnobs::default(),
        &cfr_frames(2),
    ));
    assert!(icc.info().video.as_ref().unwrap().icc);

    let mut cfg = video(Rotation::R90);
    cfg.colour = ColourTags {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Hlg,
        matrix: Matrix::Bt2020Ncl,
        range: Range::Limited,
        siting: ChromaSiting::Left,
    };
    let hdr = HdrInfo {
        mastering: Some(MasteringDisplay {
            primaries: [[8500, 39850], [6550, 2300], [35400, 14600]],
            white_point: [15635, 16450],
            max_luminance: 10_000_000,
            min_luminance: 50,
        }),
        content_light: Some(ContentLight {
            max_cll: 1000,
            max_fall: 400,
        }),
        dolby_vision: Some(DolbyVision {
            profile: 8,
            level: 5,
            rpu: true,
            enhancement_layer: false,
            base_layer: true,
            compatibility: 4,
        }),
    };
    cfg.hdr = Some(hdr);
    let v = open(write(
        cfg,
        TrackKnobs::default(),
        FileKnobs::default(),
        &cfr_frames(2),
    ))
    .info()
    .video
    .clone()
    .unwrap();
    assert_eq!(
        (v.colour.primaries, v.colour.transfer, v.colour.matrix),
        (Primaries::Bt2020, Transfer::Hlg, Matrix::Bt2020Ncl)
    );
    assert_eq!(v.hdr, Some(hdr));
    assert_eq!(v.rotation, Rotation::R90);
}

#[test]
fn audio_priming_from_the_edit_list_and_two_tracks() {
    let mut d = open(with_audio(60, 312));
    let a = d.info().audio.clone().unwrap();
    assert_eq!(
        (a.codec, a.fourcc.as_str(), a.sample_rate, a.channels),
        (AudioCodec::Opus, "Opus", 48_000, 2)
    );
    assert_eq!((a.priming, a.padding), (312, 0));
    let track = d.audio().unwrap().clone();
    assert_eq!(track.sample_count(), 120);
    let mut buf = Vec::new();
    d.read_sample(track.track, 7, &mut buf).unwrap();
    assert_eq!(buf, data(5007));
    // Video 60 × 1001/30000 s = 2.002 s; audio (120 × 800 − 312)/48000 s is shorter.
    assert_eq!(d.info().duration, r(60 * 1001, 30_000));
    // Without an edit list, Opus pre-skip is the priming.
    let mut w = fin_container::Mp4Writer::new(Cursor::new(Vec::new()), Default::default()).unwrap();
    let t = w
        .add_audio(fin_container::AudioTrackConfig {
            config: CodecConfig::Opus { dops: dops(200) },
            sample_rate: 48_000,
            channels: 2,
            priming: 0,
        })
        .unwrap();
    w.write(t, b"x", 0, 0, 960, true).unwrap();
    let info = open(w.finish().unwrap().into_inner()).info().clone();
    assert_eq!(info.audio.unwrap().priming, 200);
    assert!(info.video.is_none());
}

/// Real footage (spec §13.3): skipped without `FIN_MEDIA_FIXTURES`, failed with
/// `FIN_REQUIRE_MEDIA=1`.
#[test]
fn real_files_from_the_fixture_cache() {
    let Some(dir) = std::env::var_os("FIN_MEDIA_FIXTURES") else {
        assert!(
            std::env::var("FIN_REQUIRE_MEDIA").as_deref() != Ok("1"),
            "FIN_REQUIRE_MEDIA=1 but FIN_MEDIA_FIXTURES is unset"
        );
        eprintln!("notice: FIN_MEDIA_FIXTURES unset; real-footage demux tests skipped");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let open_file = |name: &str| {
        let f = std::fs::File::open(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        Demuxer::open(std::io::BufReader::new(f), &Limits::default()).unwrap()
    };
    let v = open_file("iphone-hevc-portrait.mov")
        .info()
        .video
        .clone()
        .unwrap();
    assert_eq!(
        (v.codec, v.rotation, v.colour.transfer),
        (VideoCodec::Hevc, Rotation::R90, Transfer::Hlg)
    );
    let dv = v
        .hdr
        .and_then(|h| h.dolby_vision)
        .expect("Dolby Vision configuration");
    assert_eq!((dv.profile, dv.compatibility), (8, 4));
    assert!(
        open_file("screen-recording.mp4")
            .info()
            .video
            .as_ref()
            .unwrap()
            .vfr
    );
    let edited = open_file("edit-list-export.mp4");
    assert_eq!(
        edited.video().unwrap().table.frames()[0].pts,
        0,
        "the edit list trims the start to 0"
    );
}
