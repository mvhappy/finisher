//! The WebM/Matroska demuxer (6B-1 Task 12): files from the test-only writer read as the same
//! tracks, tables and `MediaInfo` as MP4; hostile files are typed errors, never a panic.

mod common;

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

use common::av1c;
use fin_container::testing::webm_writer::{
    Block, Lacing, Options, Track, TrackKind, UNKNOWN_SIZE, colour, ebml_header, element,
    hdr10_colour, id_bytes, opus_head, uint, write,
};
use fin_container::{CodecConfig, Demuxer, Limits};
use fin_core::{CoreError, Rational};
use fin_media::{
    AudioCodec, Chroma, ChromaSiting, ContainerKind, Matrix, Primaries, Range, Size, Transfer,
    VideoCodec,
};

type Opened = Demuxer<Cursor<Vec<u8>>>;

fn open(bytes: Vec<u8>) -> Result<Opened, CoreError> {
    Demuxer::open(Cursor::new(bytes), &Limits::default())
}

fn payload(i: usize) -> Vec<u8> {
    let mut v = format!("frame-{i}-").into_bytes();
    v.extend(std::iter::repeat_n(i as u8, 20 + i % 7));
    v
}

fn av1_track() -> Track {
    Track {
        private: Some(av1c()),
        ..Track::video(1, "V_AV1", 320, 180)
    }
}

fn opus_track() -> Track {
    Track {
        private: Some(opus_head(2, 312)),
        codec_delay: Some(6_500_000),
        ..Track::audio(2, "A_OPUS", 48_000.0, 2)
    }
}

/// `n` video blocks at `ts(i)` ms (keyframe every `key` frames) interleaved with 20 ms Opus
/// blocks covering the same span.
fn blocks(n: usize, key: usize, ts: impl Fn(usize) -> i64, audio: bool) -> Vec<Block> {
    let mut v: Vec<Block> = (0..n)
        .map(|i| Block::simple(1, ts(i), i % key == 0, payload(i)))
        .collect();
    if audio {
        let end = ts(n - 1) + 33;
        let mut t = 0;
        let mut k = 0;
        while t < end {
            v.push(Block::simple(2, t, true, payload(1000 + k)));
            t += 20;
            k += 1;
        }
        v.sort_by_key(|b| (b.ts, b.track));
    }
    v
}

fn cfr(i: usize) -> i64 {
    (i as i64 * 1000 + 15) / 30
}

fn basic(o: &Options) -> Vec<u8> {
    write(o, &[av1_track(), opus_track()], &blocks(30, 10, cfr, true))
}

#[test]
fn opens_like_mp4() {
    let mut d = open(basic(&Options::default())).unwrap();
    let info = d.info().clone();
    assert_eq!(info.container, ContainerKind::Webm);
    let v = info.video.as_ref().unwrap();
    assert_eq!(v.codec, VideoCodec::Av1);
    assert_eq!(v.fourcc, "av01");
    assert_eq!((v.profile, v.level, v.bit_depth), (Some(0), Some(8), 8));
    assert_eq!(v.chroma, Chroma::Yuv420);
    assert_eq!(v.coded, Size::new(320, 180));
    assert_eq!(v.display, Size::new(320, 180));
    assert_eq!(v.frames, 30);
    assert_eq!(v.nominal_rate, Rational::from_int(30));
    assert!(!v.vfr);
    let track = d.video().unwrap().clone();
    assert!(matches!(track.config, CodecConfig::Av1 { ref av1c } if *av1c == common::av1c()));
    assert_eq!(track.params.codec, VideoCodec::Av1);
    assert_eq!(track.table.timescale(), 1000);
    let sync: Vec<u32> = track.table.sync().iter().map(|s| s.decode_index).collect();
    assert_eq!(sync, vec![0, 10, 20]);
    let mut buf = Vec::new();
    for i in 0..30 {
        assert_eq!(track.table.frames()[i].pts, cfr(i));
        d.read_sample(track.track, i as u32, &mut buf).unwrap();
        assert_eq!(buf, payload(i), "frame {i}");
    }
    let a = info.audio.as_ref().unwrap();
    assert_eq!(a.codec, AudioCodec::Opus);
    assert_eq!((a.sample_rate, a.channels, a.priming), (48_000, 2, 312));
    let at = d.audio().unwrap().clone();
    assert_eq!(at.timescale, 48_000);
    assert!(matches!(at.config, CodecConfig::Opus { .. }));
    let first = at.sample(1).unwrap();
    assert_eq!((first.dts, first.duration), (960, 960));
    d.read_sample(at.track, 3, &mut buf).unwrap();
    assert_eq!(buf, payload(1003));
    assert!(info.duration > Rational::new(99, 100).unwrap());
}

#[test]
fn vfr_block_timestamps() {
    // 1, 1, 2, 1, 3 frame lengths of 40 ms.
    let steps = [1, 1, 2, 1, 3];
    let at = |i: usize| (0..i).map(|k| steps[k % 5] * 40).sum::<i64>();
    let bytes = write(
        &Options::default(),
        &[av1_track()],
        &blocks(20, 5, at, false),
    );
    let d = open(bytes).unwrap();
    let v = d.info().video.clone().unwrap();
    assert!(v.vfr);
    // The most common duration (40 ms) gives the nominal rate.
    assert_eq!(v.nominal_rate, Rational::from_int(25));
    let t = &d.video().unwrap().table;
    for i in 0..19 {
        assert_eq!(t.frames()[i].pts, at(i));
        assert_eq!(t.frames()[i].duration as i64, at(i + 1) - at(i));
    }
    // CFR in rounded ticks (33 and 34 ms) is not VFR; nor is a non-dividing scale.
    let d = open(write(
        &Options::default(),
        &[av1_track()],
        &blocks(30, 10, cfr, false),
    ))
    .unwrap();
    assert!(!d.info().video.as_ref().unwrap().vfr);
    let o = Options {
        timestamp_scale: 33_333_333,
        ..Options::default()
    };
    let d = open(write(
        &o,
        &[av1_track()],
        &blocks(30, 10, |i| i as i64, false),
    ))
    .unwrap();
    let v = d.info().video.clone().unwrap();
    assert!(!v.vfr);
    assert_eq!(v.nominal_rate, Rational::from_int(30));
    assert_eq!(d.video().unwrap().table.timescale(), 1_000_000_000);
    assert_eq!(d.video().unwrap().table.frames()[3].pts, 99_999_999);
}

#[test]
fn default_duration_sets_the_rate_and_the_last_frame() {
    let t = Track {
        default_duration: Some(33_366_667),
        ..av1_track()
    };
    let at = |i: usize| (i as i64 * 1001 + 15) / 30;
    let d = open(write(&Options::default(), &[t], &blocks(10, 5, at, false))).unwrap();
    let v = d.info().video.clone().unwrap();
    assert_eq!(v.nominal_rate, Rational::new(30_000, 1001).unwrap());
    assert_eq!(d.video().unwrap().table.frames()[9].duration, 33);
}

fn tables(d: &Opened) -> (fin_media::SampleTable, Vec<fin_container::SampleInfo>) {
    let v = d.video().unwrap();
    (
        v.table.clone(),
        (0..v.sample_count())
            .map(|i| v.sample(i).unwrap())
            .collect(),
    )
}

#[test]
fn cues_and_scan_agree() {
    let tracks = [av1_track(), opus_track()];
    let b = blocks(40, 8, cfr, true);
    let plain = open(write(&Options::default(), &tracks, &b)).unwrap();
    let cued = Options {
        cues: true,
        ..Options::default()
    };
    let with_cues = open(write(&cued, &tracks, &b)).unwrap();
    assert_eq!(tables(&plain).0, tables(&with_cues).0);
    // Sample offsets differ by nothing: the cues come after the clusters.
    assert_eq!(tables(&plain).1, tables(&with_cues).1);
    assert!(!with_cues.video().unwrap().table.sync().is_empty());
    // A disagreeing index (times that name no frame) changes nothing.
    let shifted = Options {
        cue_shift: 7,
        ..cued.clone()
    };
    assert_eq!(
        tables(&open(write(&shifted, &tracks, &b)).unwrap()).0,
        tables(&plain).0
    );
    // A muxer that left the keyframe flags unset: the cues give the sync points.
    let unflagged = Options {
        unflag_keys: true,
        ..cued.clone()
    };
    let d = open(write(&unflagged, &tracks, &b)).unwrap();
    let sync: Vec<u32> = d
        .video()
        .unwrap()
        .table
        .sync()
        .iter()
        .map(|s| s.decode_index)
        .collect();
    // The writer cues the first keyframe in each cluster that has one: frames 0, 8, 16, 24, 32
    // (one keyframe every 8 frames; clusters of 8 blocks hold about 3 video frames).
    assert_eq!(sync, vec![0, 8, 16, 24, 32]);
    let unflagged_none = Options {
        cues: false,
        ..unflagged
    };
    let d = open(write(&unflagged_none, &tracks, &b)).unwrap();
    assert!(d.video().unwrap().table.sync().is_empty());
}

#[test]
fn unknown_size_cluster() {
    let known = open(basic(&Options::default())).unwrap();
    for (clusters, segment) in [(true, false), (false, true), (true, true)] {
        let o = Options {
            unknown_size_clusters: clusters,
            unknown_size_segment: segment,
            cues: true,
            ..Options::default()
        };
        let d = open(basic(&o)).unwrap();
        assert_eq!(d.info(), known.info(), "{clusters} {segment}");
        // The same frames; only their byte offsets move with the size fields.
        let shape = |d: &Opened| {
            let t = &d.video().unwrap().table;
            let f: Vec<_> = t
                .frames()
                .iter()
                .map(|f| (f.pts, f.duration, f.keyframe, f.decode_index, f.size))
                .collect();
            (f, t.sync().to_vec())
        };
        assert_eq!(shape(&d), shape(&known));
        let a = d.audio().unwrap();
        assert_eq!(a.sample_count(), known.audio().unwrap().sample_count());
    }
    // An unknown-sized cluster at the end of an unknown-sized Segment runs to the file's end.
    let o = Options {
        unknown_size_clusters: true,
        unknown_size_segment: true,
        cluster_blocks: 1000,
        ..Options::default()
    };
    assert_eq!(
        open(basic(&o))
            .unwrap()
            .info()
            .video
            .as_ref()
            .unwrap()
            .frames,
        30
    );
}

fn content_encodings() -> Vec<u8> {
    element(
        0x6D80,
        &element(
            0x6240,
            &[uint(0x5031, 0), uint(0x5032, 1), uint(0x5033, 1)].concat(),
        ),
    )
}

#[test]
fn content_encoding_refused() {
    let mut v = av1_track();
    v.extra = content_encodings();
    let e = open(write(&Options::default(), &[v], &blocks(5, 5, cfr, false)))
        .err()
        .unwrap();
    assert_eq!(e.code, "UNSUPPORTED_CONTAINER");
    assert_eq!(e.details["reason"], "contentEncoding");
    assert_eq!(e.details["format"], "webm");
    // On an audio track too; on a track that is not read, nothing is refused.
    let mut a = opus_track();
    a.extra = content_encodings();
    let o = Options {
        doc_type: "matroska".into(),
        ..Options::default()
    };
    let e = open(write(&o, &[av1_track(), a], &blocks(5, 5, cfr, true)))
        .err()
        .unwrap();
    assert_eq!(e.details["reason"], "contentEncoding");
    assert_eq!(e.details["format"], "matroska");
    let subs = Track {
        kind: TrackKind::Other(0x11),
        extra: content_encodings(),
        ..Track::video(3, "S_TEXT/UTF8", 0, 0)
    };
    let d = open(write(&o, &[av1_track(), subs], &blocks(5, 5, cfr, false))).unwrap();
    assert_eq!(d.info().container, ContainerKind::Matroska);
    assert_eq!(d.info().ignored.len(), 1);
    assert_eq!(d.info().ignored[0].id, 3);
    assert_eq!(d.info().ignored[0].handler, "subt");
}

#[test]
fn laced_audio_frames() {
    for lacing in [Lacing::Xiph, Lacing::Ebml, Lacing::Fixed] {
        let sizes: [usize; 4] = match lacing {
            Lacing::Fixed => [40; 4],
            _ => [10, 300, 7, 520],
        };
        let mut list = Vec::new();
        for b in 0..3 {
            list.push(Block {
                frames: sizes
                    .iter()
                    .enumerate()
                    .map(|(k, &s)| vec![(b * 4 + k) as u8; s])
                    .collect(),
                lacing,
                ..Block::simple(2, b as i64 * 80, true, vec![])
            });
        }
        let t = Track {
            default_duration: Some(20_000_000),
            ..opus_track()
        };
        let mut d = open(write(&Options::default(), &[t], &list)).unwrap();
        let a = d.audio().unwrap().clone();
        assert_eq!(a.sample_count(), 12, "{lacing:?}");
        let mut buf = Vec::new();
        for i in 0..12 {
            d.read_sample(a.track, i as u32, &mut buf).unwrap();
            assert_eq!(buf, vec![i as u8; sizes[i % 4]], "{lacing:?} frame {i}");
            let s = a.sample(i).unwrap();
            assert_eq!((s.dts, s.duration), (i as i64 * 960, 960), "{lacing:?} {i}");
        }
        assert!(d.video().is_none());
    }
}

#[test]
fn laced_video_refused() {
    let b = Block {
        frames: vec![vec![1; 10], vec![2; 10]],
        lacing: Lacing::Xiph,
        ..Block::simple(1, 0, true, vec![])
    };
    let e = open(write(&Options::default(), &[av1_track()], &[b]))
        .err()
        .unwrap();
    assert_eq!(e.code, "UNSUPPORTED_CONTAINER");
}

#[test]
fn block_groups_and_reference_blocks() {
    let mut list = blocks(12, 4, cfr, false);
    for b in &mut list {
        b.group = true;
    }
    list.last_mut().unwrap().duration = Some(50);
    let d = open(write(&Options::default(), &[av1_track()], &list)).unwrap();
    let t = &d.video().unwrap().table;
    let sync: Vec<u32> = t.sync().iter().map(|s| s.decode_index).collect();
    assert_eq!(sync, vec![0, 4, 8]);
    assert_eq!(t.frames()[11].duration, 50);
}

#[test]
fn b_frames_decode_in_stored_order() {
    // Decode order I0 P3 B1 B2 (presentation times stored per block).
    let order = [0usize, 3, 1, 2, 4, 7, 5, 6];
    let list: Vec<Block> = order
        .iter()
        .map(|&p| Block::simple(1, p as i64 * 40, p == 0, payload(p)))
        .collect();
    let mut t = av1_track();
    t.codec_id = "V_MPEG4/ISO/AVC".into();
    t.private = Some(vec![1, 100, 0, 40, 0xFF, 0xE0, 0]);
    let d = open(write(&Options::default(), &[t], &list)).unwrap();
    let v = d.video().unwrap();
    assert_eq!(d.info().video.as_ref().unwrap().codec, VideoCodec::H264);
    assert_eq!(v.params.profile, Some(100));
    let f = v.table.frames();
    for (k, e) in f.iter().enumerate() {
        assert_eq!(e.pts, k as i64 * 40);
        assert_eq!(
            e.decode_index as usize,
            order.iter().position(|&p| p == k).unwrap()
        );
    }
    let dts: Vec<i64> = (0..8).map(|i| v.sample(i).unwrap().dts).collect();
    assert!(dts.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn colour_and_hdr_metadata() {
    let mut t = av1_track();
    t.kind = TrackKind::Video {
        width: 320,
        height: 180,
        display: None,
        extra: hdr10_colour(1000, 400),
    };
    let d = open(write(&Options::default(), &[t], &blocks(3, 3, cfr, false))).unwrap();
    let v = d.info().video.clone().unwrap();
    assert_eq!(v.colour.primaries, Primaries::Bt2020);
    assert_eq!(v.colour.transfer, Transfer::Pq);
    assert_eq!(v.colour.matrix, Matrix::Bt2020Ncl);
    assert_eq!(v.colour.range, Range::Limited);
    assert_eq!(v.colour.siting, ChromaSiting::Left);
    let hdr = v.hdr.unwrap();
    let cl = hdr.content_light.unwrap();
    assert_eq!((cl.max_cll, cl.max_fall), (1000, 400));
    let m = hdr.mastering.unwrap();
    assert_eq!(m.primaries, [[13250, 34500], [7500, 3000], [34000, 16000]]);
    assert_eq!(m.white_point, [15635, 16450]);
    assert_eq!((m.max_luminance, m.min_luminance), (10_000_000, 50));
    // Full range BT.709 without HDR.
    let mut t = av1_track();
    t.kind = TrackKind::Video {
        width: 320,
        height: 180,
        display: None,
        extra: colour(1, 1, 1, 2),
    };
    let d = open(write(&Options::default(), &[t], &blocks(3, 3, cfr, false))).unwrap();
    let v = d.info().video.clone().unwrap();
    assert_eq!(
        (v.colour.primaries, v.colour.range),
        (Primaries::Bt709, Range::Full)
    );
    assert!(v.hdr.is_none());
}

#[test]
fn projection_refused_and_display_size() {
    for (kind, roll) in [(1u64, 0.0f64), (0, 90.0)] {
        let projection = element(
            0x7670,
            &[
                uint(0x7671, kind),
                fin_container::testing::webm_writer::float(0x7675, roll),
            ]
            .concat(),
        );
        let mut t = av1_track();
        t.kind = TrackKind::Video {
            width: 320,
            height: 180,
            display: None,
            extra: projection,
        };
        let e = open(write(&Options::default(), &[t], &blocks(3, 3, cfr, false)))
            .err()
            .unwrap();
        assert_eq!(e.code, "UNSUPPORTED_TRANSFORM", "{kind} {roll}");
    }
    // A flat projection is fine; anamorphic display sizes give the pixel aspect.
    let mut t = av1_track();
    t.kind = TrackKind::Video {
        width: 720,
        height: 576,
        display: Some((1024, 576)),
        extra: element(0x7670, &uint(0x7671, 0)),
    };
    let d = open(write(&Options::default(), &[t], &blocks(3, 3, cfr, false))).unwrap();
    let v = d.info().video.clone().unwrap();
    assert_eq!(v.pixel_aspect, Rational::new(64, 45).unwrap());
    assert_eq!(v.display, Size::new(1024, 576));
}

#[test]
fn vp9_feature_records_and_other_codecs() {
    let mut t = av1_track();
    t.codec_id = "V_VP9".into();
    // Profile 2, level 31, 10-bit, 4:2:0 colocated.
    t.private = Some(vec![1, 1, 2, 2, 1, 31, 3, 1, 10, 4, 1, 1]);
    let d = open(write(&Options::default(), &[t], &blocks(3, 3, cfr, false))).unwrap();
    let v = d.info().video.clone().unwrap();
    assert_eq!(v.codec, VideoCodec::Vp9);
    assert_eq!((v.profile, v.level, v.bit_depth), (Some(2), Some(31), 10));
    let track = d.video().unwrap();
    assert!(matches!(track.config, CodecConfig::Vp9 { .. }));
    assert_eq!(track.params.fourcc, *b"vp09");
    // No CodecPrivate: profile from the Colour bit depth.
    let mut t = av1_track();
    t.codec_id = "V_VP9".into();
    t.private = None;
    t.kind = TrackKind::Video {
        width: 64,
        height: 64,
        display: None,
        extra: element(0x55B0, &uint(0x55B2, 8)),
    };
    let d = open(write(&Options::default(), &[t], &blocks(3, 3, cfr, false))).unwrap();
    let v = d.info().video.clone().unwrap();
    assert_eq!((v.profile, v.level, v.bit_depth), (Some(0), None, 8));
    // An unknown codec is `Other`, still demuxed.
    let mut t = av1_track();
    t.codec_id = "V_THEORA".into();
    let d = open(write(&Options::default(), &[t], &blocks(3, 3, cfr, false))).unwrap();
    assert_eq!(d.info().video.as_ref().unwrap().codec, VideoCodec::Other);
}

#[test]
fn doc_types() {
    let o = Options {
        doc_type: "matroska".into(),
        ..Options::default()
    };
    assert_eq!(
        open(basic(&o)).unwrap().info().container,
        ContainerKind::Matroska
    );
    let o = Options {
        doc_type: "x-other".into(),
        ..Options::default()
    };
    assert_eq!(open(basic(&o)).err().unwrap().code, "UNSUPPORTED_CONTAINER");
}

// Hostile inputs: each a typed error.

fn segment(body: &[u8]) -> Vec<u8> {
    [ebml_header("webm"), element(0x1853_8067, body)].concat()
}

fn tracks_el(tracks: &[Track]) -> Vec<u8> {
    let full = write(&Options::default(), tracks, &[]);
    let at = full
        .windows(4)
        .position(|w| w == [0x16, 0x54, 0xAE, 0x6B])
        .unwrap();
    full[at..].to_vec()
}

#[test]
fn hostile_size_past_the_parent() {
    // Info declares 4 bytes; its TimestampScale child declares 8.
    let info = [
        id_bytes(0x1549_A966),
        vec![0x84],
        vec![0x2A, 0xD7, 0xB1, 0x88],
    ]
    .concat();
    let e = open(segment(&[info, tracks_el(&[av1_track()])].concat()))
        .err()
        .unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT");
}

#[test]
fn hostile_huge_and_unknown_sizes() {
    // A 2⁵⁶ - 2 byte Tracks (the largest 8-byte size) in a small file.
    let huge = [
        id_bytes(0x1654_AE6B),
        vec![0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE],
    ]
    .concat();
    let e = open(segment(&huge)).err().unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT");
    assert!(e.message.contains("72057594037927934"), "{}", e.message);
    // An unknown size on an element that may not have one.
    let unknown = [id_bytes(0x1654_AE6B), UNKNOWN_SIZE.to_vec()].concat();
    let e = open(segment(&unknown)).err().unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT");
    // A size or id longer than EBML allows (a zero first byte).
    let e = open(segment(&[0x00, 0x81, 0x00])).err().unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT");
}

#[test]
fn hostile_depth_and_counts() {
    let bytes = basic(&Options::default());
    let shallow = Limits {
        max_depth: 3,
        ..Limits::default()
    };
    let e = Demuxer::open(Cursor::new(bytes.clone()), &shallow)
        .err()
        .unwrap();
    assert_eq!(
        (e.code.as_str(), e.details["limit"].as_str()),
        ("MEDIA_LIMIT", Some("maxDepth"))
    );
    let few = Limits {
        max_samples: 20,
        ..Limits::default()
    };
    let e = Demuxer::open(Cursor::new(bytes.clone()), &few)
        .err()
        .unwrap();
    assert_eq!(e.details["limit"], "maxSamples");
    let elements = Limits {
        max_boxes: 10,
        ..Limits::default()
    };
    let e = Demuxer::open(Cursor::new(bytes), &elements).err().unwrap();
    assert_eq!(e.details["limit"], "maxBoxes");
}

#[test]
fn hostile_lacing_count_over_the_block() {
    // 256 Xiph-laced frames of 510 bytes each declared in a 40-byte block.
    let mut body = vec![0x82, 0, 0, 0x02, 0xFF];
    for _ in 0..255 {
        body.extend([0xFF, 0xFF, 0x00]);
    }
    let block = element(0xA3, &body[..40]);
    let cluster = element(0x1F43_B675, &[uint(0xE7, 0), block].concat());
    let e = open(segment(&[tracks_el(&[opus_track()]), cluster].concat()))
        .err()
        .unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT");
    assert!(e.message.contains("lacing"), "{}", e.message);
    // EBML lacing whose sizes run negative.
    let block = element(0xA3, &[0x82, 0, 0, 0x06, 0x02, 0x81, 0x80, 0x00, 1, 2, 3]);
    let cluster = element(0x1F43_B675, &[uint(0xE7, 0), block].concat());
    let e = open(segment(&[tracks_el(&[opus_track()]), cluster].concat()))
        .err()
        .unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT");
}

#[test]
fn hostile_blocks() {
    // A block before the cluster's Timestamp, and an empty video frame.
    for c in [
        element(0x1F43_B675, &element(0xA3, &[0x81, 0, 0, 0x80, 1])),
        element(
            0x1F43_B675,
            &[uint(0xE7, 0), element(0xA3, &[0x81, 0, 0, 0x80])].concat(),
        ),
    ] {
        let e = open(segment(&[tracks_el(&[av1_track()]), c].concat()))
            .err()
            .unwrap();
        assert_eq!(e.code, "MEDIA_CORRUPT", "{}", e.message);
    }
    // Two frames at one time.
    let list = [
        Block::simple(1, 0, true, payload(0)),
        Block::simple(1, 0, true, payload(1)),
    ];
    let e = open(write(&Options::default(), &[av1_track()], &list))
        .err()
        .unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT");
}

#[test]
fn hostile_times() {
    // Frames 2⁶³ ticks apart: their gap overflows.
    let list = [
        Block::simple(1, -32_768, true, payload(0)),
        Block::simple(1, i64::MAX - 1, false, payload(1)),
    ];
    let e = open(write(&Options::default(), &[av1_track()], &list))
        .err()
        .unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT", "{}", e.message);
    // A cluster time past i64, and audio times that overflow at a huge TimestampScale.
    let c = element(
        0x1F43_B675,
        &[uint(0xE7, u64::MAX), element(0xA3, &[0x81, 0, 0, 0x80, 1])].concat(),
    );
    let e = open(segment(&[tracks_el(&[av1_track()]), c].concat()))
        .err()
        .unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT");
    let o = Options {
        timestamp_scale: u64::MAX,
        ..Options::default()
    };
    let list = [
        Block::simple(2, 0, true, payload(0)),
        Block::simple(2, 30_000, true, payload(1)),
    ];
    let e = open(write(&o, &[opus_track()], &list)).err().unwrap();
    assert_eq!(e.code, "MEDIA_CORRUPT");
}

const TYPED: [&str; 4] = [
    "MEDIA_CORRUPT",
    "MEDIA_LIMIT",
    "UNSUPPORTED_CONTAINER",
    "UNSUPPORTED_TRANSFORM",
];

fn attempt(bytes: Vec<u8>, what: &str) {
    let got = catch_unwind(AssertUnwindSafe(|| {
        Demuxer::open(Cursor::new(bytes), &Limits::default()).map(|mut d| {
            let tracks: Vec<_> = [
                d.video().map(|v| (v.track, v.sample_count())),
                d.audio().map(|a| (a.track, a.sample_count())),
            ]
            .into_iter()
            .flatten()
            .collect();
            let mut buf = Vec::new();
            for (t, n) in tracks {
                for i in 0..n {
                    if let Err(e) = d.read_sample(t, i as u32, &mut buf) {
                        assert!(TYPED.contains(&e.code.as_str()), "{what}: untyped {e:?}");
                    }
                }
            }
        })
    }))
    .unwrap_or_else(|_| panic!("{what}: the demuxer panicked"));
    if let Err(e) = got {
        assert!(TYPED.contains(&e.code.as_str()), "{what}: untyped {e:?}");
    }
}

#[test]
fn truncated_and_mutated_files_are_typed_errors() {
    let o = Options {
        cues: true,
        ..Options::default()
    };
    let bytes = write(&o, &[av1_track(), opus_track()], &blocks(12, 4, cfr, true));
    for cut in 0..bytes.len() {
        attempt(bytes[..cut].to_vec(), &format!("cut at {cut}"));
    }
    let n: u64 = std::env::var("FIN_MUTATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4000);
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for case in 0..n {
        let mut m = bytes.clone();
        for _ in 0..1 + next() % 4 {
            let at = (next() % m.len() as u64) as usize;
            m[at] = match next() % 3 {
                0 => next() as u8,
                1 => 0xFF,
                _ => m[at] ^ (1 << (next() % 8)),
            };
        }
        attempt(m, &format!("mutation {case}"));
    }
}
