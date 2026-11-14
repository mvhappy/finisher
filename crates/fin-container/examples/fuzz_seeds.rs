//! Writes the fuzz corpus seeds for `fuzz/` (muxer files: CFR, B-frames, an empty edit, a
//! rotation, pixel aspect, HDR boxes, fragments, audio with priming) into the folder given, and
//! with a second folder the `webm` target's seeds (the test-only WebM writer: cues, unknown
//! sizes, block groups, the three lacings, HDR colour, VP9, Matroska).
//! `cargo run -p fin-container --example fuzz_seeds --features testing -- <dir> [<webm dir>]`.

use std::io::Cursor;

use fin_container::mux::knobs::{FileKnobs, TrackKnobs};
use fin_container::{AudioTrackConfig, CodecConfig, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::Rational;
use fin_media::{ColourTags, ContentLight, HdrInfo, Rotation, Size};

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: fuzz_seeds <dir>"));
    std::fs::create_dir_all(&dir).expect("corpus folder");
    // av1C: marker/version, profile 0 level 8, 4:2:0.
    let av1 = CodecConfig::Av1 {
        av1c: vec![0x81, 0x08, 0x0c, 0x00],
    };
    let video = |rotation, pixel_aspect, hdr| VideoTrackConfig {
        config: av1.clone(),
        coded: Size::new(64, 36),
        timescale: 30_000,
        rotation,
        pixel_aspect,
        colour: ColourTags::BT709,
        hdr,
    };
    let one = Rational::from_int(1);
    let hdr = Some(HdrInfo {
        content_light: Some(ContentLight {
            max_cll: 1000,
            max_fall: 400,
        }),
        ..Default::default()
    });
    let cases: Vec<(&str, VideoTrackConfig, TrackKnobs, FileKnobs, bool)> = vec![
        (
            "cfr",
            video(Rotation::R0, one, None),
            TrackKnobs::default(),
            FileKnobs::default(),
            false,
        ),
        (
            "rot90-pasp",
            video(Rotation::R90, Rational::new(4, 3).unwrap(), None),
            TrackKnobs::default(),
            FileKnobs::default(),
            false,
        ),
        (
            "hdr-empty-edit",
            video(Rotation::R0, one, hdr),
            TrackKnobs {
                empty_edit: Some(500),
                ..Default::default()
            },
            FileKnobs::default(),
            false,
        ),
        (
            "fragmented",
            video(Rotation::R0, one, None),
            TrackKnobs::default(),
            FileKnobs {
                fragment_every: Some(4),
            },
            false,
        ),
        (
            "audio",
            video(Rotation::R180, one, None),
            TrackKnobs::default(),
            FileKnobs::default(),
            true,
        ),
    ];
    for (name, cfg, knobs, file, audio) in cases {
        let mut w = Mp4Writer::new(
            Cursor::new(Vec::new()),
            Mp4Options {
                moov_reserve: 0,
                knobs: file,
                ..Default::default()
            },
        )
        .unwrap();
        let v = w.add_video(cfg).unwrap();
        w.set_knobs(v, knobs);
        let a = audio.then(|| {
            let dops = CodecConfig::Opus {
                dops: vec![0, 2, 0x01, 0x38, 0, 0, 0xbb, 0x80, 0, 0, 0],
            };
            w.add_audio(AudioTrackConfig {
                config: dops,
                sample_rate: 48_000,
                channels: 2,
                priming: 312,
                presented: None,
            })
            .unwrap()
        });
        for i in 0..12i64 {
            // B-frame order in every other case: I P B B …
            let p = if name == "cfr" {
                [0, 3, 1, 2][i as usize % 4] + i / 4 * 4
            } else {
                i
            };
            w.write(v, &[i as u8; 7], p * 1001, (i - 1) * 1001, 1001, i % 4 == 0)
                .unwrap();
            if let Some(a) = a {
                w.write(a, &[0xa0 | i as u8; 3], i * 1600, i * 1600, 1600, true)
                    .unwrap();
            }
        }
        let bytes = w.finish().unwrap().into_inner();
        std::fs::write(dir.join(name), bytes).expect("seed written");
    }
    // The 6A-1 review's shapes: edits that repeat the media, a fragment run of zero-size
    // samples, and fragment decode times near i64::MAX.
    let mut w = Mp4Writer::new(
        Cursor::new(Vec::new()),
        Mp4Options {
            moov_reserve: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let v = w.add_video(video(Rotation::R0, one, None)).unwrap();
    w.set_knobs(
        v,
        TrackKnobs {
            edits: Some(vec![(12_000, Some(0)); 16]),
            ..Default::default()
        },
    );
    for i in 0..12i64 {
        w.write(v, &[i as u8; 3], i * 1000, i * 1000, 1000, i == 0)
            .unwrap();
    }
    std::fs::write(dir.join("edits-repeat"), w.finish().unwrap().into_inner())
        .expect("seed written");
    let fragmented = || {
        let opts = Mp4Options {
            knobs: FileKnobs {
                fragment_every: Some(1),
            },
            ..Default::default()
        };
        let mut w = Mp4Writer::new(Cursor::new(Vec::new()), opts).unwrap();
        let v = w.add_video(video(Rotation::R0, one, None)).unwrap();
        w.write(v, &[1; 3], 0, 0, 1000, true).unwrap();
        w.finish().unwrap().into_inner()
    };
    for (name, size, count, tfdt) in [
        ("trun-zero-size", 0u32, 1_000_000u32, None),
        ("tfdt-overflow", 1, 2, Some(i64::MAX as u64 - 10)),
    ] {
        let mut bytes = fragmented();
        bytes.extend(moof(size, count, tfdt));
        std::fs::write(dir.join(name), bytes).expect("seed written");
    }
    if let Some(webm) = std::env::args().nth(2) {
        webm_seeds(std::path::Path::new(&webm));
    }
}

fn webm_seeds(dir: &std::path::Path) {
    use fin_container::testing::webm_writer::{
        Block, Lacing, Options, Track, TrackKind, hdr10_colour, opus_head, write,
    };
    std::fs::create_dir_all(dir).expect("corpus folder");
    let av1 = Track {
        private: Some(vec![0x81, 0x08, 0x0c, 0x00]),
        default_duration: Some(33_366_667),
        ..Track::video(1, "V_AV1", 64, 36)
    };
    let opus = Track {
        private: Some(opus_head(2, 312)),
        codec_delay: Some(6_500_000),
        ..Track::audio(2, "A_OPUS", 48_000.0, 2)
    };
    let blocks = |group: bool, lacing: Lacing| {
        let mut v: Vec<Block> = (0..12)
            .map(|i| Block {
                group,
                duration: group.then_some(33),
                ..Block::simple(1, i * 33, i % 4 == 0, vec![i as u8; 7])
            })
            .collect();
        for k in 0..10 {
            v.push(Block {
                frames: match lacing {
                    Lacing::None => vec![vec![0xa0 | k as u8; 3]],
                    _ => vec![vec![0xb0; 3], vec![0xb1; 300], vec![0xb2; 5]],
                },
                lacing,
                ..Block::simple(2, k * 40, true, vec![])
            });
        }
        v.sort_by_key(|b| (b.ts, b.track));
        v
    };
    let mut hdr = av1.clone();
    hdr.kind = TrackKind::Video {
        width: 64,
        height: 36,
        display: Some((96, 36)),
        extra: hdr10_colour(1000, 400),
    };
    let vp9 = Track {
        codec_id: "V_VP9".into(),
        private: Some(vec![1, 1, 2, 2, 1, 31, 3, 1, 10, 4, 1, 1]),
        ..av1.clone()
    };
    let cued = Options {
        cues: true,
        ..Options::default()
    };
    let unknown = Options {
        unknown_size_clusters: true,
        unknown_size_segment: true,
        cues: true,
        ..Options::default()
    };
    let mkv = Options {
        doc_type: "matroska".into(),
        cluster_blocks: 3,
        ..Options::default()
    };
    let plain = Options::default();
    let none = Lacing::None;
    let cases: Vec<(&str, &Options, Vec<Track>, Vec<Block>)> = vec![
        (
            "webm-av1-opus",
            &plain,
            vec![av1.clone(), opus.clone()],
            blocks(false, none),
        ),
        (
            "webm-cues",
            &cued,
            vec![av1.clone(), opus.clone()],
            blocks(false, none),
        ),
        (
            "webm-unknown",
            &unknown,
            vec![av1.clone(), opus.clone()],
            blocks(false, none),
        ),
        ("webm-groups", &plain, vec![av1.clone()], blocks(true, none)),
        (
            "webm-xiph",
            &plain,
            vec![av1.clone(), opus.clone()],
            blocks(false, Lacing::Xiph),
        ),
        (
            "webm-ebml",
            &mkv,
            vec![av1.clone(), opus.clone()],
            blocks(false, Lacing::Ebml),
        ),
        ("webm-hdr", &plain, vec![hdr], blocks(false, none)),
        ("webm-vp9", &cued, vec![vp9], blocks(false, none)),
    ];
    for (name, o, tracks, list) in cases {
        std::fs::write(dir.join(name), write(o, &tracks, &list)).expect("seed written");
    }
}

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [&((body.len() + 8) as u32).to_be_bytes()[..], kind, body].concat()
}

fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = flags.to_be_bytes();
    b[0] = version;
    bx(kind, &[&b[..], body].concat())
}

/// A `moof` for track 1: base is the moof, default duration 1000 and size `size`, an optional
/// `tfdt` v1, a `trun` of `count` samples with no per-sample fields.
fn moof(size: u32, count: u32, tfdt: Option<u64>) -> Vec<u8> {
    let mut traf = full(
        b"tfhd",
        0,
        0x02_0000 | 0x08 | 0x10,
        &[
            1u32.to_be_bytes(),
            1000u32.to_be_bytes(),
            size.to_be_bytes(),
        ]
        .concat(),
    );
    if let Some(t) = tfdt {
        traf.extend(full(b"tfdt", 1, 0, &t.to_be_bytes()));
    }
    traf.extend(full(
        b"trun",
        0,
        0x1,
        &[count.to_be_bytes(), 0i32.to_be_bytes()].concat(),
    ));
    bx(
        b"moof",
        &[full(b"mfhd", 0, 0, &9u32.to_be_bytes()), bx(b"traf", &traf)].concat(),
    )
}
