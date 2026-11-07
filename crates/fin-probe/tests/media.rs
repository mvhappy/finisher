//! Media probing through `fin-container` (plan 6A Task 8): rotation, VFR and edit lists surface
//! in `Probe.media`, while the model's `video`/`audio` keep their 4C meaning (coded size,
//! average rate, track timescale).

use std::io::Cursor;

use fin_container::mux::knobs::TrackKnobs;
use fin_container::{CodecConfig, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::Rational;
use fin_media::{ColourTags, ContainerKind, Rotation, Size, VideoCodec};
use fin_probe::probe;

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

/// A 1280×720 track of `durations` (ticks at 30000), rotated, with `knobs`.
fn clip(rotation: Rotation, durations: &[u32], knobs: TrackKnobs) -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w
        .add_video(VideoTrackConfig {
            config: CodecConfig::Other { fourcc: *b"avc1" },
            coded: Size::new(1280, 720),
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
