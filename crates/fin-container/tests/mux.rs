//! The MP4 muxer, checked by decoding its boxes with `mp4-atom` directly (the demuxer is
//! tested against these files in `demux.rs`) and by walking the box tree for the boxes
//! `mp4-atom` has no types for.

use std::io::Cursor;

use fin_container::mux::knobs::{ColrKnob, FileKnobs, TrackKnobs};
use fin_container::{AudioTrackConfig, CodecConfig, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::Rational;
use fin_media::{
    ColourTags, ContentLight, HdrInfo, MasteringDisplay, Matrix, Primaries, Range, Rotation, Size,
    Transfer,
};
use mp4_atom::{Any, Atom, Av1c, Codec, Decode, Dops, Moof, Moov, StszSamples};

fn av1c() -> Vec<u8> {
    let c = Av1c {
        seq_profile: 0,
        seq_level_idx_0: 8,
        chroma_subsampling_x: true,
        chroma_subsampling_y: true,
        ..Default::default()
    };
    let mut body = Vec::new();
    c.encode_body(&mut body).unwrap();
    body
}

fn dops(pre_skip: u16) -> Vec<u8> {
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

fn video(rotation: Rotation) -> VideoTrackConfig {
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

/// Deterministic sample bytes for sample `i`.
fn data(i: u32) -> Vec<u8> {
    (0..(20 + i % 13)).map(|k| (i * 31 + k) as u8).collect()
}

struct Written {
    bytes: Vec<u8>,
}

impl Written {
    fn top(&self) -> Vec<Any> {
        let mut buf = self.bytes.as_slice();
        let mut out = Vec::new();
        while !buf.is_empty() {
            out.push(Any::decode(&mut buf).unwrap());
        }
        out
    }

    fn moov(&self) -> Moov {
        self.top()
            .into_iter()
            .find_map(|a| if let Any::Moov(m) = a { Some(m) } else { None })
            .unwrap()
    }
}

/// Writes `n` CFR video frames (a keyframe every 30) with `opts` and the track `knobs`.
fn cfr(n: u32, opts: Mp4Options, cfg: VideoTrackConfig, knobs: TrackKnobs) -> Written {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), opts).unwrap();
    let v = w.add_video(cfg).unwrap();
    w.set_knobs(v, knobs);
    for i in 0..n {
        let t = i as i64 * 1001;
        w.write(v, &data(i), t, t, 1001, i % 30 == 0).unwrap();
    }
    Written {
        bytes: w.finish().unwrap().into_inner(),
    }
}

/// Reads every sample's bytes back through stsc/stco/stsz.
fn samples(bytes: &[u8], moov: &Moov, track: usize) -> Vec<Vec<u8>> {
    let stbl = &moov.trak[track].mdia.minf.stbl;
    let sizes: Vec<u32> = match &stbl.stsz.samples {
        StszSamples::Different { sizes } => sizes.clone(),
        StszSamples::Identical { count, size } => vec![*size; *count as usize],
    };
    let offsets: Vec<u64> = match (&stbl.stco, &stbl.co64) {
        (Some(s), _) => s.entries.iter().map(|&o| o as u64).collect(),
        (_, Some(c)) => c.entries.clone(),
        _ => panic!("no chunk offsets"),
    };
    let mut per_chunk = vec![0u32; offsets.len()];
    for (k, e) in stbl.stsc.entries.iter().enumerate() {
        let end = stbl
            .stsc
            .entries
            .get(k + 1)
            .map_or(offsets.len() as u32, |n| n.first_chunk - 1);
        for c in (e.first_chunk - 1)..end {
            per_chunk[c as usize] = e.samples_per_chunk;
        }
    }
    let mut out = Vec::new();
    let mut s = 0;
    for (c, &off) in offsets.iter().enumerate() {
        let mut at = off as usize;
        for _ in 0..per_chunk[c] {
            out.push(bytes[at..at + sizes[s] as usize].to_vec());
            at += sizes[s] as usize;
            s += 1;
        }
    }
    out
}

/// The body of the first box at `path` (a list of types), descending through containers and
/// sample entries.
fn find(bytes: &[u8], path: &[&[u8; 4]]) -> Option<Vec<u8>> {
    let (want, rest) = path.split_first()?;
    let mut at = 0usize;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = bytes[at + 4..at + 8].try_into().unwrap();
        let (head, size) = if size == 1 {
            (
                16,
                u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize,
            )
        } else {
            (8, size)
        };
        let body = &bytes[at + head..at + size];
        if &kind == *want {
            if rest.is_empty() {
                return Some(body.to_vec());
            }
            let skip = match &kind {
                b"stsd" => 8,
                b"av01" | b"hvc1" | b"avc1" => 78,
                b"mp4a" | b"Opus" => 28,
                _ => 0,
            };
            return find(&body[skip..], rest);
        }
        at += size;
    }
    None
}

#[test]
fn cfr_av1_structure_and_sample_bytes() {
    let n = 90;
    let w = cfr(
        n,
        Mp4Options::default(),
        video(Rotation::R0),
        TrackKnobs::default(),
    );
    let kinds: Vec<String> = w.top().iter().map(|a| a.kind().to_string()).collect();
    assert_eq!(
        kinds,
        ["ftyp", "moov", "free", "mdat"],
        "fast start: moov before mdat"
    );
    let moov = w.moov();
    assert_eq!(moov.mvhd.timescale, 30_000);
    assert_eq!(moov.mvhd.duration, 90 * 1001);
    let trak = &moov.trak[0];
    assert_eq!(trak.tkhd.width.integer(), 320);
    assert_eq!(
        (trak.tkhd.matrix.a, trak.tkhd.matrix.b, trak.tkhd.matrix.d),
        (0x10000, 0, 0x10000)
    );
    assert!(trak.edts.is_none(), "no edit list for a first pts of 0");
    let stbl = &trak.mdia.minf.stbl;
    assert_eq!(stbl.stts.entries.len(), 1);
    assert_eq!(
        (
            stbl.stts.entries[0].sample_count,
            stbl.stts.entries[0].sample_delta
        ),
        (90, 1001)
    );
    assert_eq!(stbl.stss.as_ref().unwrap().entries, [1, 31, 61]);
    assert!(stbl.ctts.is_none());
    // One chunk per second: 30000 / 1001 ≈ 29.97 → 30 samples a chunk.
    assert_eq!(stbl.stco.as_ref().unwrap().entries.len(), 3);
    let Codec::Av01(av01) = &stbl.stsd.codecs[0] else {
        panic!("not av01")
    };
    assert_eq!((av01.visual.width, av01.visual.height), (320, 180));
    assert_eq!(av01.av1c.seq_level_idx_0, 8);
    assert_eq!(
        av01.colr,
        Some(mp4_atom::Colr::Nclx {
            colour_primaries: 1,
            transfer_characteristics: 1,
            matrix_coefficients: 1,
            full_range_flag: false
        })
    );
    assert!(av01.pasp.is_none());
    let got = samples(&w.bytes, &moov, 0);
    assert_eq!(got, (0..n).map(data).collect::<Vec<_>>());
}

#[test]
fn identical_input_gives_identical_bytes() {
    let a = cfr(
        40,
        Mp4Options::default(),
        video(Rotation::R90),
        TrackKnobs::default(),
    );
    let b = cfr(
        40,
        Mp4Options::default(),
        video(Rotation::R90),
        TrackKnobs::default(),
    );
    assert_eq!(a.bytes, b.bytes);
}

#[test]
fn ten_thousand_samples_and_a_moov_larger_than_its_reserve() {
    for reserve in [64 << 10, 512, 0] {
        let opts = Mp4Options {
            moov_reserve: reserve,
            ..Default::default()
        };
        let w = cfr(10_000, opts, video(Rotation::R0), TrackKnobs::default());
        let kinds: Vec<String> = w.top().iter().map(|a| a.kind().to_string()).collect();
        assert_eq!(&kinds[..2], ["ftyp", "moov"], "reserve {reserve}");
        assert_eq!(kinds.last().unwrap(), "mdat");
        let moov = w.moov();
        let got = samples(&w.bytes, &moov, 0);
        assert_eq!(got.len(), 10_000);
        assert!(
            got.iter().enumerate().all(|(i, d)| *d == data(i as u32)),
            "reserve {reserve}"
        );
    }
}

#[test]
fn rotation_matrices_and_pixel_aspect() {
    for (r, (a, b, c, d)) in [
        (Rotation::R90, (0, 0x10000, -0x10000, 0)),
        (Rotation::R180, (-0x10000, 0, 0, -0x10000)),
        (Rotation::R270, (0, -0x10000, 0x10000, 0)),
    ] {
        let m = cfr(3, Mp4Options::default(), video(r), TrackKnobs::default())
            .moov()
            .trak[0]
            .tkhd
            .matrix
            .clone();
        assert_eq!((m.a, m.b, m.c, m.d), (a, b, c, d), "{r:?}");
    }
    let mut cfg = video(Rotation::R0);
    cfg.pixel_aspect = Rational::new(4, 3).unwrap();
    let moov = cfr(3, Mp4Options::default(), cfg, TrackKnobs::default()).moov();
    let Codec::Av01(av01) = &moov.trak[0].mdia.minf.stbl.stsd.codecs[0] else {
        panic!()
    };
    assert_eq!(
        av01.pasp.as_ref().map(|p| (p.h_spacing, p.v_spacing)),
        Some((4, 3))
    );
    // tkhd carries the display width before rotation.
    assert_eq!(moov.trak[0].tkhd.width.integer(), 426);
}

#[test]
fn colour_and_hdr_boxes() {
    let mut cfg = video(Rotation::R0);
    cfg.colour = ColourTags {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Hlg,
        matrix: Matrix::Bt2020Ncl,
        range: Range::Full,
        ..ColourTags::BT709
    };
    cfg.hdr = Some(HdrInfo {
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
        dolby_vision: None,
    });
    let w = cfr(2, Mp4Options::default(), cfg, TrackKnobs::default());
    let entry = [
        b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd", b"av01",
    ];
    let colr = find(&w.bytes, &[entry.as_slice(), &[b"colr"]].concat()).unwrap();
    assert_eq!(&colr[..4], b"nclx");
    assert_eq!(&colr[4..11], &[0, 9, 0, 18, 0, 9, 0x80]);
    let mdcv = find(&w.bytes, &[entry.as_slice(), &[b"mdcv"]].concat()).unwrap();
    assert_eq!(mdcv.len(), 24);
    assert_eq!(
        &mdcv[..4],
        &8500u16
            .to_be_bytes()
            .iter()
            .chain(&39850u16.to_be_bytes())
            .copied()
            .collect::<Vec<_>>()[..]
    );
    let clli = find(&w.bytes, &[entry.as_slice(), &[b"clli"]].concat()).unwrap();
    assert_eq!(clli, [0x03, 0xE8, 0x01, 0x90]);
    // Colour-box knobs.
    let none = cfr(
        2,
        Mp4Options::default(),
        video(Rotation::R0),
        TrackKnobs {
            colr: ColrKnob::None,
            ..Default::default()
        },
    );
    assert!(find(&none.bytes, &[entry.as_slice(), &[b"colr"]].concat()).is_none());
    let icc = cfr(
        2,
        Mp4Options::default(),
        video(Rotation::R0),
        TrackKnobs {
            colr: ColrKnob::Icc(vec![1, 2, 3]),
            ..Default::default()
        },
    );
    assert_eq!(
        find(&icc.bytes, &[entry.as_slice(), &[b"colr"]].concat()).unwrap(),
        [b'p', b'r', b'o', b'f', 1, 2, 3]
    );
}

#[test]
fn b_frames_write_ctts_and_an_edit_list() {
    // Decode order I P B B (pts 0 3 1 2 in frames), dts from −1 frame: offsets are positive
    // and the first composition time is 1 frame, so the edit list starts there.
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w.add_video(video(Rotation::R0)).unwrap();
    let order = [0i64, 3, 1, 2, 6, 4, 5];
    for (i, &p) in order.iter().enumerate() {
        w.write(
            v,
            &data(i as u32),
            p * 1001,
            (i as i64 - 1) * 1001,
            1001,
            i % 4 == 0,
        )
        .unwrap();
    }
    let moov = Written {
        bytes: w.finish().unwrap().into_inner(),
    }
    .moov();
    let stbl = &moov.trak[0].mdia.minf.stbl;
    let offsets: Vec<i64> = stbl
        .ctts
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .flat_map(|e| std::iter::repeat_n(e.sample_offset, e.sample_count as usize))
        .collect();
    assert_eq!(
        offsets,
        order
            .iter()
            .enumerate()
            .map(|(i, &p)| (p - i as i64 + 1) * 1001)
            .collect::<Vec<_>>()
    );
    let elst = moov.trak[0].edts.as_ref().unwrap().elst.as_ref().unwrap();
    assert_eq!(elst.entries.len(), 1);
    assert_eq!(elst.entries[0].media_time, Some(1001));
    assert_eq!(elst.entries[0].segment_duration, 7 * 1001);

    // Offsets that go negative make a version-1 ctts.
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w.add_video(video(Rotation::R0)).unwrap();
    for (i, &p) in [0i64, 3, 1, 2].iter().enumerate() {
        w.write(v, &data(i as u32), p * 1001, i as i64 * 1001, 1001, i == 0)
            .unwrap();
    }
    let bytes = w.finish().unwrap().into_inner();
    let ctts = find(
        &bytes,
        &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"ctts"],
    )
    .unwrap();
    assert_eq!(ctts[0], 1, "ctts version 1 for negative offsets");
}

#[test]
fn vfr_durations_and_an_empty_edit() {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w.add_video(video(Rotation::R0)).unwrap();
    w.set_knobs(
        v,
        TrackKnobs {
            empty_edit: Some(500),
            ..Default::default()
        },
    );
    let durations = [1001u32, 1001, 2002, 500, 1001];
    let mut t = 0i64;
    for (i, &d) in durations.iter().enumerate() {
        w.write(v, &data(i as u32), t, t, d, i == 0).unwrap();
        t += d as i64;
    }
    let moov = Written {
        bytes: w.finish().unwrap().into_inner(),
    }
    .moov();
    let stts: Vec<(u32, u32)> = moov.trak[0]
        .mdia
        .minf
        .stbl
        .stts
        .entries
        .iter()
        .map(|e| (e.sample_count, e.sample_delta))
        .collect();
    assert_eq!(stts, [(2, 1001), (1, 2002), (1, 500), (1, 1001)]);
    let elst = &moov.trak[0]
        .edts
        .as_ref()
        .unwrap()
        .elst
        .as_ref()
        .unwrap()
        .entries;
    assert_eq!((elst[0].segment_duration, elst[0].media_time), (500, None));
    assert_eq!(
        (elst[1].segment_duration, elst[1].media_time),
        (t as u64, Some(0))
    );
}

#[test]
fn audio_priming_edit_list_and_two_tracks() {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w.add_video(video(Rotation::R0)).unwrap();
    let a = w
        .add_audio(AudioTrackConfig {
            config: CodecConfig::Opus { dops: dops(312) },
            sample_rate: 48_000,
            channels: 2,
            priming: 312,
            presented: None,
        })
        .unwrap();
    for i in 0..60u32 {
        let t = i as i64 * 1001;
        w.write(v, &data(i), t, t, 1001, i % 30 == 0).unwrap();
        for k in 0..2 {
            let s = (i * 2 + k) as i64 * 960;
            w.write(a, &data(1000 + i * 2 + k), s, s, 960, true)
                .unwrap();
        }
    }
    let written = Written {
        bytes: w.finish().unwrap().into_inner(),
    };
    let moov = written.moov();
    assert_eq!(moov.trak.len(), 2);
    let audio = &moov.trak[1];
    assert_eq!(audio.mdia.hdlr.handler.to_string(), "soun");
    let Codec::Opus(opus) = &audio.mdia.minf.stbl.stsd.codecs[0] else {
        panic!("not Opus")
    };
    assert_eq!(opus.dops.pre_skip, 312);
    let elst = audio.edts.as_ref().unwrap().elst.as_ref().unwrap();
    assert_eq!(elst.entries[0].media_time, Some(312));
    // With audio the movie timescale is lcm(48000, 30000) = 240000 (6C plan Decision 22), so
    // 120 × 960 − 312 samples are exact.
    assert_eq!(moov.mvhd.timescale, 240_000);
    assert_eq!(elst.entries[0].segment_duration, (120 * 960 - 312) * 5);
    assert!(
        audio.mdia.minf.stbl.stss.is_none(),
        "all audio samples are sync samples"
    );
    let got = samples(&written.bytes, &moov, 1);
    assert_eq!(got, (0..120).map(|k| data(1000 + k)).collect::<Vec<_>>());
}

#[test]
fn matrix_knob_and_fragmented_output() {
    let m = [0x10000, 0x8000, 0, -0x8000, 0x10000, 0, 0, 0, 0x4000_0000];
    let w = cfr(
        3,
        Mp4Options::default(),
        video(Rotation::R0),
        TrackKnobs {
            matrix: Some(m),
            ..Default::default()
        },
    );
    let got = w.moov().trak[0].tkhd.matrix.clone();
    assert_eq!(
        [
            got.a, got.b, got.u, got.c, got.d, got.v, got.x, got.y, got.w
        ],
        m
    );

    let opts = Mp4Options {
        knobs: FileKnobs {
            fragment_every: Some(10),
        },
        ..Default::default()
    };
    let w = cfr(25, opts, video(Rotation::R0), TrackKnobs::default());
    let top = w.top();
    let kinds: Vec<String> = top.iter().map(|a| a.kind().to_string()).collect();
    assert_eq!(
        kinds,
        [
            "ftyp", "moov", "moof", "mdat", "moof", "mdat", "moof", "mdat"
        ]
    );
    let moov = w.moov();
    assert!(moov.mvex.is_some());
    assert_eq!(moov.trak[0].mdia.minf.stbl.stts.entries.len(), 0);
    let mut counts = Vec::new();
    let mut base = Vec::new();
    for a in &top {
        if let Any::Moof(Moof { traf, .. }) = a {
            let trun = &traf[0].trun[0];
            if let mp4_atom::TrunSamples::Explicit(e) = &trun.samples {
                counts.push(e.len());
            }
            base.push(traf[0].tfdt.as_ref().unwrap().base_media_decode_time);
        }
    }
    assert_eq!(counts, [10, 10, 5]);
    assert_eq!(base, [0, 10 * 1001, 20 * 1001]);
}

#[test]
fn inconsistent_timestamps_and_late_tracks_are_refused() {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w.add_video(video(Rotation::R0)).unwrap();
    w.write(v, b"a", 0, 0, 1001, true).unwrap();
    assert_eq!(
        w.write(v, b"b", 2000, 2000, 1001, false).unwrap_err().code,
        "INVALID_RANGE"
    );
    assert!(w.add_video(video(Rotation::R0)).is_err());
    let _ = Moov::KIND;
}
