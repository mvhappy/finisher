//! Media probing through `fin-container` (plan 6A Task 8): rotation, VFR and edit lists surface
//! in `Probe.media`, while the model's `video`/`audio` keep their 4C meaning (coded size,
//! average rate, track timescale).

use std::io::Cursor;

use fin_container::mux::knobs::TrackKnobs;
use fin_container::{CodecConfig, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::{AssetReprobe, DisplaySize, MemIds, Rational, VideoInfo};
use fin_media::{ColourTags, ContainerKind, Rotation, Size, VideoCodec};
use fin_probe::probe;

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

/// A 1280×720 track of `durations` (ticks at 30000), rotated, with `knobs`.
fn clip(rotation: Rotation, durations: &[u32], knobs: TrackKnobs) -> Vec<u8> {
    clip_of(Size::new(1280, 720), rotation, durations, knobs)
}

/// A `coded` track of `durations` (ticks at 30000), rotated (the `tkhd` matrix), with `knobs`.
fn clip_of(coded: Size, rotation: Rotation, durations: &[u32], knobs: TrackKnobs) -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w
        .add_video(VideoTrackConfig {
            config: CodecConfig::Other { fourcc: *b"avc1" },
            coded,
            timescale: 30_000,
            rotation,
            pixel_aspect: Rational::from_int(1),
            colour: ColourTags::BT709,
            hdr: None,
        })
        .unwrap();
    w.set_knobs(v, knobs);
    let mut t = 0i64;
    for (i, &d) in durations.iter().enumerate() {
        w.write(v, &[i as u8; 16], t, t, d, i == 0).unwrap();
        t += i64::from(d);
    }
    w.finish().unwrap().into_inner()
}

fn probe_bytes(bytes: Vec<u8>) -> fin_probe::Probe {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    std::fs::write(&path, bytes).unwrap();
    probe(&path).unwrap()
}

#[test]
fn rotation_surfaces_in_media_and_the_model_keeps_the_coded_size() {
    let p = probe_bytes(clip(Rotation::R90, &[1001; 30], TrackKnobs::default()));
    let v = p.video.unwrap();
    assert_eq!((v.width, v.height, v.rate), (1280, 720, r(30_000, 1001)));
    // Schema 4: the model also keeps the display geometry.
    assert_eq!(
        (v.display, v.rotation, v.pixel_aspect),
        (
            Some(DisplaySize {
                width: 720,
                height: 1280
            }),
            Some(Rotation::R90),
            Some(Rational::from_int(1))
        )
    );
    let m = p.media.unwrap();
    assert_eq!(m.container, ContainerKind::Mp4);
    let mv = m.video.unwrap();
    assert_eq!(
        (mv.codec, mv.rotation, mv.coded, mv.display),
        (
            VideoCodec::H264,
            Rotation::R90,
            Size::new(1280, 720),
            Size::new(720, 1280)
        )
    );
    assert!(!mv.vfr);
}

#[test]
fn vfr_surfaces_in_media_and_the_model_rate_is_the_average() {
    let durations = [1001u32, 1001, 2002, 1001, 3003, 1001];
    let p = probe_bytes(clip(Rotation::R0, &durations, TrackKnobs::default()));
    // 6 frames in 9009 ticks at 30000: the average, as before.
    assert_eq!(p.video.unwrap().rate, r(6 * 30_000, 9009));
    let mv = p.media.unwrap().video.unwrap();
    assert!(mv.vfr);
    assert_eq!(mv.nominal_rate, r(30_000, 1001));
}

#[test]
fn an_edit_list_surfaces_in_the_duration_and_the_frames() {
    let p = probe_bytes(clip(
        Rotation::R0,
        &[1001; 10],
        TrackKnobs {
            empty_edit: Some(3000),
            ..Default::default()
        },
    ));
    // 3000 ticks of nothing, then 10 frames.
    assert_eq!(p.duration, Some(r(3000 + 10 * 1001, 30_000)));
    assert_eq!(p.media.unwrap().video.unwrap().frames, 10);
}

#[test]
fn a_bad_matrix_is_unsupported_transform_with_the_path() {
    let m = [0x10000, 0x8000, 0, -0x8000, 0x10000, 0, 0, 0, 0x4000_0000];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skew.mp4");
    std::fs::write(
        &path,
        clip(
            Rotation::R0,
            &[1001; 3],
            TrackKnobs {
                matrix: Some(m),
                ..Default::default()
            },
        ),
    )
    .unwrap();
    let e = probe(&path).unwrap_err();
    assert_eq!(e.code, "UNSUPPORTED_TRANSFORM");
    assert!(std::path::Path::new(e.details["path"].as_str().unwrap()).is_absolute());
}

/// WebM is dispatched by its EBML magic (6B-1 Task 12) and probes like MP4: the model's rate
/// is the average, the audio rate the track's sampling rate.
#[test]
fn webm_probes_by_its_magic() {
    use fin_container::testing::webm_writer::{Block, Options, Track, opus_head, write};
    let mut blocks: Vec<Block> = (0..30)
        .map(|i| Block::simple(1, (i * 1000 + 15) / 30, i % 10 == 0, vec![i as u8; 24]))
        .collect();
    blocks.extend((0..50).map(|i| Block::simple(2, i * 20, true, vec![0xFC; 12])));
    blocks.sort_by_key(|b| (b.ts, b.track));
    let tracks = [
        Track::video(1, "V_AV1", 640, 360),
        Track {
            private: Some(opus_head(2, 312)),
            ..Track::audio(2, "A_OPUS", 48_000.0, 2)
        },
    ];
    let p = probe_bytes(write(&Options::default(), &tracks, &blocks));
    let v = p.video.unwrap();
    assert_eq!((v.width, v.height), (640, 360));
    // 30 frames over 967 + 34 ms.
    assert_eq!(v.rate, r(30_000, 1001));
    let a = p.audio.unwrap();
    assert_eq!((a.sample_rate, a.channels), (48_000, 2));
    let m = p.media.unwrap();
    assert_eq!(m.container, ContainerKind::Webm);
    assert_eq!(m.video.unwrap().nominal_rate, r(30, 1));
    assert_eq!(m.audio.unwrap().priming, 312);

    // A track whose frames are compressed: the reason code survives with the path.
    let mut t = Track::video(1, "V_AV1", 64, 64);
    t.extra = [0x6D, 0x80, 0x83, 0x62, 0x40, 0x80].to_vec();
    let bytes = write(&Options::default(), &[t], &blocks[..1]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.webm");
    std::fs::write(&path, bytes).unwrap();
    let e = probe(&path).unwrap_err();
    assert_eq!(e.code, "UNSUPPORTED_CONTAINER");
    assert_eq!(e.details["reason"], "contentEncoding");
    assert!(e.details["path"].is_string());
}

/// 6B-1 Task 11: a portrait phone clip (1920×1080 coded, a 90° `tkhd` matrix) imported before
/// schema 5 has no display geometry; `asset.reprobe` with a fresh probe's streams gives it
/// `display` 1080×1920 and `rotation` 90, and its inverse takes them away again.
#[test]
fn reprobe_fills_rotation_of_portrait_clip() {
    let p = probe_bytes(clip_of(
        Size::new(1920, 1080),
        Rotation::R90,
        &[1001; 30],
        TrackKnobs::default(),
    ));
    let probed = p.video.clone().unwrap();
    // As schema 3 stored it: the coded size and rate, nothing else.
    let old = VideoInfo::coded(probed.width, probed.height, probed.rate);
    let project = fin_core::new_project("p1".into(), "P".into(), Rational::from_int(30)).unwrap();
    let tx = |ops| fin_core::Tx {
        tx_id: "t".into(),
        author: "test".into(),
        branch: "main".into(),
        base_rev: 0,
        schema_version: fin_core::SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    };
    let add = fin_core::AssetAdd::media(
        "$a",
        "/m/portrait.mp4",
        p.duration.unwrap(),
        Some(old),
        None,
        "fp",
    );
    let before = fin_core::apply_tx(
        &project,
        &tx(vec![add.into()]),
        &mut MemIds::after(&project),
    )
    .unwrap()
    .project;
    assert_eq!(before.assets["a1"].video.as_ref().unwrap().display, None);
    let reprobe = AssetReprobe {
        asset: "a1".into(),
        video: p.video.clone(),
        audio: p.audio.clone(),
    };
    let after = fin_core::apply_tx(
        &before,
        &tx(vec![reprobe.into()]),
        &mut MemIds::after(&before),
    )
    .unwrap();
    let v = after.project.assets["a1"].video.clone().unwrap();
    assert_eq!(
        v.display,
        Some(DisplaySize {
            width: 1080,
            height: 1920
        })
    );
    assert_eq!(v.rotation, Some(Rotation::R90));
    assert_eq!((v.width, v.height), (1920, 1080), "the coded size stays");
    let undone = fin_core::apply_internal(
        &after.project,
        &tx(after.inverse.clone()),
        &mut MemIds::after(&after.project),
    )
    .unwrap();
    assert_eq!(undone.project, before);
}
