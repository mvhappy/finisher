//! A small hand-written box builder, so the demuxer is not tested only against our own writer:
//! a QuickTime file with what `fin-container`'s muxer never writes (an `nclc` `colr`, `stz2`,
//! `ctts` v1 with negative offsets, an edit that cuts the first frame, the `moov` after the
//! `mdat`, an unknown box between known ones), and a `co64` offset past the end of the file.

use std::io::Cursor;

use fin_container::{CodecConfig, Demuxer, Limits};
use fin_core::Rational;
use fin_media::{ContainerKind, Matrix, Primaries, Range, Size, Transfer, VideoCodec, frame_at};

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

const IDENTITY: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];

fn avc1(width: u16, height: u16) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend_from_slice(&1u16.to_be_bytes());
    b.extend_from_slice(&[0u8; 16]);
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&height.to_be_bytes());
    b.extend_from_slice(&u32s(&[0x0048_0000, 0x0048_0000, 0]));
    b.extend_from_slice(&1u16.to_be_bytes());
    b.extend_from_slice(&[0u8; 32]);
    b.extend_from_slice(&0x18u16.to_be_bytes());
    b.extend_from_slice(&(-1i16).to_be_bytes());
    // avcC: version 1, High profile (100), level 4.0, 4-byte lengths, no parameter sets.
    b.extend(bx(b"avcC", &[1, 100, 0, 40, 0xff, 0xe0, 0]));
    // QuickTime `nclc`: BT.709 primaries and transfer, BT.601 matrix; no range flag.
    b.extend(bx(
        b"colr",
        &[b"nclc".as_slice(), &[0, 1, 0, 1, 0, 6]].concat(),
    ));
    bx(b"avc1", &b)
}

/// The file: `ftyp` (`qt  `), `mdat`, an unknown box, then the `moov`. `offsets_box` writes the
/// chunk offset (relative to the file) as `stco` or `co64` (`bad_offset` overrides it).
fn file(offsets_box: &[u8; 4], bad_offset: Option<u64>) -> Vec<u8> {
    let ftyp = bx(
        b"ftyp",
        &[b"qt  ".as_slice(), &0x0200u32.to_be_bytes(), b"qt  "].concat(),
    );
    let sizes = [10u8, 11, 12, 13, 14];
    let data: Vec<u8> = sizes
        .iter()
        .enumerate()
        .flat_map(|(i, &n)| vec![i as u8 + 1; n as usize])
        .collect();
    let mdat = bx(b"mdat", &data);
    let unknown = bx(b"abcd", b"not a box we know");
    let data_at = (ftyp.len() + 8) as u64;

    let mut mvhd = u32s(&[0, 0, 600, 100, 0x10000]);
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
    mvhd.extend_from_slice(&[0u8; 10]);
    mvhd.extend(u32s(&IDENTITY));
    mvhd.extend_from_slice(&[0u8; 24]);
    mvhd.extend(u32s(&[2]));
    let mvhd = full(b"mvhd", 0, 0, &mvhd);

    let mut tkhd = u32s(&[0, 0, 1, 0, 90, 0, 0]);
    tkhd.extend_from_slice(&[0u8; 8]);
    tkhd.extend(u32s(&IDENTITY));
    tkhd.extend(u32s(&[640 << 16, 360 << 16]));
    let tkhd = full(b"tkhd", 0, 3, &tkhd[..]);
    // The edit starts 10 ticks into the first frame (frames are 20 ticks) and runs 90 ticks.
    let elst = full(
        b"elst",
        0,
        0,
        &[u32s(&[1, 90, 10]), vec![0, 1, 0, 0]].concat(),
    );
    let edts = bx(b"edts", &elst);

    let mdhd = full(
        b"mdhd",
        0,
        0,
        &[u32s(&[0, 0, 600, 100]), vec![0x55, 0xc4, 0, 0]].concat(),
    );
    let hdlr = full(
        b"hdlr",
        0,
        0,
        &[b"mhlr".as_slice(), b"vide", &[0u8; 12], &[0]].concat(),
    );
    let vmhd = full(b"vmhd", 0, 1, &[0u8; 8]);
    let dinf = bx(
        b"dinf",
        &full(
            b"dref",
            0,
            0,
            &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
        ),
    );
    let stsd = full(b"stsd", 0, 0, &[u32s(&[1]), avc1(640, 360)].concat());
    let stts = full(b"stts", 0, 0, &u32s(&[1, 5, 20]));
    // Decode order I P B P B: composition offsets 0, +20, −20, +20, −20 (version 1, signed).
    let offsets: [i32; 5] = [0, 20, -20, 20, -20];
    let mut ctts = u32s(&[5]);
    for o in offsets {
        ctts.extend(u32s(&[1]));
        ctts.extend_from_slice(&o.to_be_bytes());
    }
    let ctts = full(b"ctts", 1, 0, &ctts);
    let stss = full(b"stss", 0, 0, &u32s(&[1, 1]));
    let stsc = full(b"stsc", 0, 0, &u32s(&[1, 1, 5, 1]));
    let stz2 = full(
        b"stz2",
        0,
        0,
        &[vec![0, 0, 0, 8], u32s(&[5]), sizes.to_vec()].concat(),
    );
    let at = bad_offset.unwrap_or(data_at);
    let chunk = match offsets_box {
        b"co64" => full(
            b"co64",
            0,
            0,
            &[u32s(&[1]), at.to_be_bytes().to_vec()].concat(),
        ),
        _ => full(b"stco", 0, 0, &u32s(&[1, at as u32])),
    };
    let stbl = bx(
        b"stbl",
        &cat(&[
            stsd,
            stts,
            bx(b"zzzz", b"unknown"),
            ctts,
            stss,
            stsc,
            stz2,
            chunk,
        ]),
    );
    let minf = bx(b"minf", &cat(&[vmhd, dinf, stbl]));
    let mdia = bx(b"mdia", &cat(&[mdhd, hdlr, minf]));
    let trak = bx(b"trak", &cat(&[tkhd, edts, mdia]));
    let moov = bx(b"moov", &cat(&[mvhd, trak]));
    cat(&[ftyp, mdat, unknown, moov])
}

#[test]
fn a_hand_built_quicktime_file() {
    let mut d = Demuxer::open(Cursor::new(file(b"stco", None)), &Limits::default()).unwrap();
    let info = d.info().clone();
    assert_eq!(info.container, ContainerKind::Mov);
    let v = info.video.unwrap();
    assert_eq!(
        (v.codec, v.fourcc.as_str(), v.profile, v.level),
        (VideoCodec::H264, "avc1", Some(100), Some(40))
    );
    assert_eq!(
        (v.coded, v.display),
        (Size::new(640, 360), Size::new(640, 360))
    );
    assert_eq!(
        (
            v.colour.primaries,
            v.colour.transfer,
            v.colour.matrix,
            v.colour.range
        ),
        (
            Primaries::Bt709,
            Transfer::Bt709,
            Matrix::Bt601,
            Range::Limited
        )
    );
    let track = d.video().unwrap().clone();
    assert!(matches!(&track.config, CodecConfig::Avc { avcc } if avcc[1] == 100));
    // Composition times 0, 40, 20, 80, 60; the edit starts at 10, so the first frame is cut
    // and keeps pts 0, and the rest move 10 ticks earlier.
    let t = &track.table;
    let got: Vec<(i64, u32, u32)> = t
        .frames()
        .iter()
        .map(|f| (f.pts, f.duration, f.decode_index))
        .collect();
    assert_eq!(
        got,
        [
            (0, 10, 0),
            (10, 20, 2),
            (30, 20, 1),
            (50, 20, 4),
            (70, 20, 3)
        ]
    );
    assert_eq!(t.end_ticks(), 90);
    assert_eq!(info.duration, Rational::new(90, 600).unwrap());
    assert_eq!(
        frame_at(t, Rational::new(20, 600).unwrap()).index,
        1,
        "20 is the midpoint of 10 and 30: the earlier frame"
    );
    // stz2 sizes and the data behind them.
    for (k, n) in [10usize, 11, 12, 13, 14].iter().enumerate() {
        let mut buf = Vec::new();
        d.read_sample(track.track, k as u32, &mut buf).unwrap();
        assert_eq!(buf, vec![k as u8 + 1; *n]);
    }
}

#[test]
fn co64_offsets_work_and_past_the_end_are_corrupt() {
    let d = Demuxer::open(Cursor::new(file(b"co64", None)), &Limits::default()).unwrap();
    assert_eq!(d.video().unwrap().table.len(), 5);
    for bad in [1 << 40, u64::MAX - 3] {
        let e = Demuxer::open(Cursor::new(file(b"co64", Some(bad))), &Limits::default())
            .err()
            .unwrap();
        assert_eq!(e.code, "MEDIA_CORRUPT", "{e:?}");
    }
}

#[test]
fn not_an_mp4_is_unsupported() {
    for bytes in [
        b"RIFF\0\0\0\0WAVEfmt ".to_vec(),
        b"\x1aE\xdf\xa3matroska".to_vec(),
        vec![0; 3],
    ] {
        let e = Demuxer::open(Cursor::new(bytes), &Limits::default())
            .err()
            .unwrap();
        assert_eq!(e.code, "UNSUPPORTED_CONTAINER");
    }
}
