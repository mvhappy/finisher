//! In-band HDR metadata (plan 6B-1 Task 11; sub-project 6 spec §6.3): H.264 and HEVC SEI
//! mastering display colour volume (payload 137) and content light level (144), read from a
//! track's first sample when the sample entry has no `mdcv`/`clli` box. The bitstreams are
//! written by hand (SEI NAL units only, no picture data): no media file is committed and no
//! H.264/HEVC code of our own encodes anything.

use std::io::Cursor;

use fin_container::demux::{in_band_hdr, sei_hdr};
use fin_container::{CodecConfig, Demuxer, Limits, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::Rational;
use fin_media::{ColourTags, ContentLight, HdrInfo, MasteringDisplay, Rotation, Size};

/// A minimal `avcC`: version 1, High profile, level 4.0, 4-byte lengths, no parameter sets.
fn avcc() -> Vec<u8> {
    vec![1, 100, 0, 40, 0xFF, 0xE0, 0]
}

/// A minimal `hvcC` (23 bytes): Main profile, level 4.0, 4:2:0, 8-bit, 4-byte lengths, no arrays.
fn hvcc() -> Vec<u8> {
    let mut b = vec![0u8; 23];
    b[0] = 1;
    b[1] = 1;
    b[12] = 120;
    b[16] = 0xFC | 1;
    b[17] = 0xF8;
    b[18] = 0xF8;
    b[21] = 0x03;
    b
}

/// The values a phone's HDR10 clip carries: P3 primaries (G, B, R), D65, 1000 / 0.0001 nits.
/// `min_luminance` 1 gives the payload `00 00 00 01`, which needs an emulation prevention byte.
fn mastering() -> MasteringDisplay {
    MasteringDisplay {
        primaries: [[13250, 34500], [7500, 3000], [34000, 16000]],
        white_point: [15635, 16450],
        max_luminance: 10_000_000,
        min_luminance: 1,
    }
}

fn light() -> ContentLight {
    ContentLight {
        max_cll: 1000,
        max_fall: 400,
    }
}

fn mdcv_payload(m: &MasteringDisplay) -> Vec<u8> {
    let mut p = Vec::new();
    for [x, y] in m.primaries {
        p.extend(x.to_be_bytes());
        p.extend(y.to_be_bytes());
    }
    p.extend(m.white_point[0].to_be_bytes());
    p.extend(m.white_point[1].to_be_bytes());
    p.extend(m.max_luminance.to_be_bytes());
    p.extend(m.min_luminance.to_be_bytes());
    p
}

fn clli_payload(c: &ContentLight) -> Vec<u8> {
    let mut p = c.max_cll.to_be_bytes().to_vec();
    p.extend(c.max_fall.to_be_bytes());
    p
}

/// SEI messages (type, payload) as an RBSP: `ff`-extended type and size, then the trailing bits.
fn sei_rbsp(messages: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (kind, payload) in messages {
        for mut v in [*kind, payload.len() as u32] {
            while v >= 255 {
                out.push(0xFF);
                v -= 255;
            }
            out.push(v as u8);
        }
        out.extend(payload);
    }
    out.push(0x80);
    out
}

/// `rbsp` with emulation prevention bytes inserted (`00 00 0x` → `00 00 03 0x`, x ≤ 3).
fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// NAL units with 4-byte big-endian lengths, as an MP4 sample holds them.
fn sample(nals: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for n in nals {
        out.extend((n.len() as u32).to_be_bytes());
        out.extend(n);
    }
    out
}

fn h264_sei(messages: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut nal = vec![0x06];
    nal.extend(escape(&sei_rbsp(messages)));
    nal
}

fn hevc_sei(messages: &[(u32, Vec<u8>)]) -> Vec<u8> {
    // nal_unit_type 39 (prefix SEI), layer 0, temporal id 1.
    let mut nal = vec![39 << 1, 1];
    nal.extend(escape(&sei_rbsp(messages)));
    nal
}

fn both() -> Vec<(u32, Vec<u8>)> {
    vec![
        (137, mdcv_payload(&mastering())),
        (144, clli_payload(&light())),
    ]
}

fn expected() -> HdrInfo {
    HdrInfo {
        mastering: Some(mastering()),
        content_light: Some(light()),
        dolby_vision: None,
    }
}

/// A one-track MP4 of `config` whose samples are `samples`, with no HDR boxes.
fn mp4(config: CodecConfig, samples: &[Vec<u8>]) -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w
        .add_video(VideoTrackConfig {
            config,
            coded: Size::new(1920, 1080),
            timescale: 30_000,
            rotation: Rotation::R0,
            pixel_aspect: Rational::from_int(1),
            colour: ColourTags::BT709,
            hdr: None,
        })
        .unwrap();
    for (i, s) in samples.iter().enumerate() {
        let t = i as i64 * 1001;
        w.write(v, s, t, t, 1001, i == 0).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn sei_hdr_read_when_no_boxes() {
    // An access unit delimiter, the SEI, then a (dummy) IDR slice.
    let first = sample(&[vec![0x09, 0xF0], h264_sei(&both()), vec![0x65, 0x88, 0x80]]);
    let file = mp4(
        CodecConfig::Avc { avcc: avcc() },
        &[first, vec![0, 0, 0, 1, 0x41]],
    );
    let mut d = Demuxer::open(Cursor::new(file), &Limits::default()).unwrap();
    let info = d.info().video.clone().unwrap();
    assert_eq!(info.hdr, None, "the sample entry has no mdcv/clli/dvcC box");
    assert_eq!(in_band_hdr(&mut d).unwrap(), Some(expected()));

    let first = sample(&[hevc_sei(&both()), vec![0x26, 0x01, 0xAF]]);
    let file = mp4(CodecConfig::Hevc { hvcc: hvcc() }, &[first]);
    let mut d = Demuxer::open(Cursor::new(file), &Limits::default()).unwrap();
    assert_eq!(d.info().video.as_ref().unwrap().hdr, None);
    assert_eq!(in_band_hdr(&mut d).unwrap(), Some(expected()));
}

#[test]
fn one_message_alone_and_other_payloads_skipped() {
    let avc = CodecConfig::Avc { avcc: avcc() };
    // A user-data payload (type 5) of 300 bytes (an `ff`-extended size) before the CLL.
    let msgs = vec![(5, vec![0xAB; 300]), (144, clli_payload(&light()))];
    let got = sei_hdr(&avc, &sample(&[h264_sei(&msgs)])).unwrap();
    assert_eq!(
        got,
        HdrInfo {
            content_light: Some(light()),
            ..HdrInfo::default()
        }
    );
    // An HEVC suffix SEI (type 40) counts too.
    let mut suffix = hevc_sei(&[(137, mdcv_payload(&mastering()))]);
    suffix[0] = 40 << 1;
    let hevc = CodecConfig::Hevc { hvcc: hvcc() };
    let got = sei_hdr(&hevc, &sample(&[suffix])).unwrap();
    assert_eq!(got.mastering, Some(mastering()));
    assert_eq!(got.content_light, None);
}

#[test]
fn no_sei_or_other_codecs_give_none() {
    let avc = CodecConfig::Avc { avcc: avcc() };
    assert_eq!(sei_hdr(&avc, &sample(&[vec![0x65, 0x88]])), None);
    // An AV1 or VP9 track carries HDR metadata in its own syntax, not in SEI.
    let av1 = CodecConfig::Av1 {
        av1c: vec![0x81, 0, 0x0C, 0],
    };
    assert_eq!(sei_hdr(&av1, &sample(&[h264_sei(&both())])), None);
    // AV1 files are untouched by `in_band_hdr` too.
    let file = mp4(av1, &[vec![0x12, 0]]);
    let mut d = Demuxer::open(Cursor::new(file), &Limits::default()).unwrap();
    assert_eq!(in_band_hdr(&mut d).unwrap(), None);
}

#[test]
fn malformed_sei_is_ignored_never_a_panic() {
    let avc = CodecConfig::Avc { avcc: avcc() };
    let good = sample(&[h264_sei(&both())]);
    // Every truncation and every single-byte change of a good sample: no panic, and nothing
    // read that is not a whole message.
    for n in 0..good.len() {
        let _ = sei_hdr(&avc, &good[..n]);
    }
    for i in 0..good.len() {
        for v in [0x00, 0x03, 0x80, 0xFF] {
            let mut b = good.clone();
            b[i] = v;
            let _ = sei_hdr(&avc, &b);
        }
    }
    // A payload size past the NAL, a NAL length past the sample, a short config record.
    let short = sample(&[vec![0x06, 137, 24, 1, 2, 3, 0x80]]);
    assert_eq!(sei_hdr(&avc, &short), None);
    let mut past = good.clone();
    past[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(sei_hdr(&avc, &past), None);
    assert_eq!(sei_hdr(&CodecConfig::Avc { avcc: vec![1, 2] }, &good), None);
    assert_eq!(
        sei_hdr(&CodecConfig::Hevc { hvcc: vec![1; 10] }, &good),
        None
    );
}
