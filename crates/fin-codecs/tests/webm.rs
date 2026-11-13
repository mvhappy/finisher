//! WebM through the decoders (6B-1 Task 12): the same AV1 packets written into WebM and MP4
//! decode to identical planes; VP9 in WebM demuxes, and on Linux decodes to `CODEC_UNAVAILABLE`
//! until libvpx (6B-2, plan Decision 16).

use std::io::Cursor;

use fin_codecs::fixture::{ClipSpec, encode_all, fixture_config, timing};
use fin_codecs::{FrameReader, ProbeResult, RawFrame, open_encoder};
use fin_container::testing::webm_writer::{Block, Options, Track, write};
use fin_container::{CodecConfig, Limits, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::Rational;
use fin_media::backend::EncodePurpose;
use fin_media::testing::{pattern, read_id_y};
use fin_media::{ColourTags, ContainerKind, Rotation, Size, VideoCodec};

#[test]
fn av1_in_webm_decodes_like_mp4() {
    let spec = ClipSpec {
        frames: 24,
        size: Size::new(320, 180),
        keyint: 8,
        ..Default::default()
    };
    let cfg = fixture_config(&spec, VideoCodec::Av1);
    assert_eq!(cfg.purpose, EncodePurpose::TestFixture);
    let mut enc = open_encoder(&cfg, &ProbeResult::software_only()).unwrap();
    let packets = encode_all(&mut enc, spec.frames, |i| {
        let p = pattern(i, 320, 180, spec.seed);
        RawFrame::i420(spec.size, p.y, p.u, p.v)
    })
    .unwrap();
    let config = enc.config().unwrap();
    let CodecConfig::Av1 { av1c } = &config else {
        panic!("not an av1C: {config:?}")
    };

    let (timescale, tick) = timing(spec.rate);
    let mut mp4 = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let track = mp4
        .add_video(VideoTrackConfig {
            config: config.clone(),
            coded: spec.size,
            timescale,
            rotation: Rotation::R0,
            pixel_aspect: Rational::from_int(1),
            colour: ColourTags::BT709,
            hdr: None,
        })
        .unwrap();
    let mut blocks = Vec::new();
    for pk in &packets {
        let at = pk.pts * tick as i64;
        mp4.write(track, &pk.data, at, at, tick, pk.keyframe)
            .unwrap();
        // Milliseconds, rounded: 0, 33, 67, …
        blocks.push(Block::simple(
            1,
            (pk.pts * 1000 + 15) / 30,
            pk.keyframe,
            pk.data.clone(),
        ));
    }
    let mp4 = mp4.finish().unwrap().into_inner();
    let webm = write(
        &Options::default(),
        &[Track {
            private: Some(av1c.clone()),
            ..Track::video(1, "V_AV1", 320, 180)
        }],
        &blocks,
    );

    let caps = ProbeResult::software_only();
    let mut a = FrameReader::open(Cursor::new(mp4), &Limits::default(), &caps).unwrap();
    let mut b = FrameReader::open(Cursor::new(webm), &Limits::default(), &caps).unwrap();
    assert_eq!(b.info().container, ContainerKind::Webm);
    let (va, vb) = (
        a.info().video.clone().unwrap(),
        b.info().video.clone().unwrap(),
    );
    assert_eq!(
        (
            va.codec,
            va.profile,
            va.level,
            va.bit_depth,
            va.chroma,
            va.coded
        ),
        (
            vb.codec,
            vb.profile,
            vb.level,
            vb.bit_depth,
            vb.chroma,
            vb.coded
        )
    );
    assert_eq!((va.frames, va.nominal_rate), (vb.frames, vb.nominal_rate));
    assert_eq!(a.table().len(), b.table().len());
    let syncs = |r: &FrameReader<Cursor<Vec<u8>>>| -> Vec<u32> {
        r.table().sync().iter().map(|s| s.decode_index).collect()
    };
    assert_eq!(syncs(&a), syncs(&b));
    // In order, then backwards (random access from the sync samples).
    let order: Vec<usize> = (0..24).chain((0..24).rev()).collect();
    for i in order {
        let fa = a.frame(i).unwrap().clone();
        let fb = b.frame(i).unwrap();
        assert_eq!(fa.planes, fb.planes, "frame {i}");
        assert_eq!(read_id_y(&fb.planes[0].data, 320, 180), Some(i as u32));
    }
}

#[test]
fn vp9_in_webm_demuxes_and_needs_a_decoder() {
    // Packets need not be real VP9: the track is demuxed, then no decoder is found.
    let blocks: Vec<Block> = (0..4)
        .map(|i| Block::simple(1, i * 40, i == 0, vec![0x82, 0x49, 0x83, i as u8]))
        .collect();
    let webm = write(
        &Options::default(),
        &[Track {
            private: Some(vec![1, 1, 0, 3, 1, 8]),
            ..Track::video(1, "V_VP9", 64, 64)
        }],
        &blocks,
    );
    let d = fin_container::Demuxer::open(Cursor::new(webm.clone()), &Limits::default()).unwrap();
    let v = d.video().unwrap();
    assert_eq!(v.params.codec, VideoCodec::Vp9);
    assert_eq!((v.params.profile, v.params.bit_depth), (Some(0), 8));
    assert!(matches!(v.config, CodecConfig::Vp9 { .. }));
    let caps = ProbeResult::software_only();
    let err = FrameReader::open(Cursor::new(webm), &Limits::default(), &caps)
        .err()
        .unwrap();
    // Linux has no VP9 decoder in 6B-1 (Decision 16); neither has the software-only probe on
    // any OS.
    assert_eq!(err.code, "CODEC_UNAVAILABLE", "{err:?}");
}
