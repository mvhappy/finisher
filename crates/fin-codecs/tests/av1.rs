//! Software AV1 round trips (spec §13.5): SVT-AV1 encodes frame-id patterns, `rav1d` decodes
//! them back with the right ids and quality; broken input is a typed error; encoding is
//! deterministic with one thread.

use std::io::Cursor;

use fin_codecs::fixture::{ClipSpec, pattern_clip};
use fin_codecs::{Av1EncoderConfig, FrameReader, RawFrame, capabilities, open_av1_encoder};
use fin_container::{Demuxer, Limits};
use fin_core::Rational;
use fin_media::testing::{pattern, read_id_y};
use fin_media::{ColourTags, PixelFormat, Size, VideoCodec};

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| (x as f64 - y as f64).powi(2))
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

#[test]
fn encode_120_pattern_frames_and_decode_them_back() {
    let spec = ClipSpec {
        frames: 120,
        size: Size::new(960, 540),
        keyint: 60,
        crf: 30,
        preset: 10,
        ..Default::default()
    };
    let file = pattern_clip(&spec).unwrap();
    let mut r = FrameReader::open(Cursor::new(file), &Limits::default(), &capabilities()).unwrap();
    let v = r.info().video.clone().unwrap();
    assert_eq!(
        (v.codec, v.coded, v.frames),
        (VideoCodec::Av1, Size::new(960, 540), 120)
    );
    assert_eq!(v.nominal_rate, Rational::from_int(30));
    assert_eq!(r.table().frames().iter().filter(|f| f.keyframe).count(), 2);
    let mut worst = f64::INFINITY;
    for i in 0..120 {
        let f = r.frame(i).unwrap();
        assert_eq!((f.format, f.size), (PixelFormat::I420, Size::new(960, 540)));
        assert_eq!(
            read_id_y(&f.planes[0].data, 960, 540),
            Some(i as u32),
            "frame {i}"
        );
        let src = pattern(i as u32, 960, 540, spec.seed);
        worst = worst.min(psnr(&f.planes[0].data, &src.y));
    }
    assert!(worst >= 35.0, "worst Y PSNR {worst:.2} dB");
    eprintln!("120 frames 960×540 CRF 30: worst Y PSNR {worst:.2} dB");
}

#[test]
fn a_truncated_sample_is_decode_failed_without_a_panic() {
    let file = pattern_clip(&ClipSpec {
        frames: 10,
        ..Default::default()
    })
    .unwrap();
    let mut d = Demuxer::open(Cursor::new(file), &Limits::default()).unwrap();
    let track = d.video().unwrap().clone();
    let mut dec = fin_codecs::open_decoder(&track, &capabilities()).unwrap();
    let mut buf = Vec::new();
    d.read_sample(track.track, 0, &mut buf).unwrap();
    let mut failed = None;
    for cut in [buf.len() / 2, buf.len() / 4, 3] {
        dec.flush().unwrap();
        match dec.send(&buf[..cut], 0).and_then(|_| dec.receive()) {
            Err(e) => failed = Some(e),
            Ok(Some(_)) => {}
            Ok(None) => {}
        }
        if failed.is_some() {
            break;
        }
    }
    let e = failed.expect("a cut keyframe fails to decode");
    assert_eq!(e.code, "DECODE_FAILED");
    assert_eq!(e.details["backend"], "rav1d");
}

#[test]
fn encoding_with_one_thread_is_deterministic() {
    let run = || {
        let cfg = Av1EncoderConfig {
            size: Size::new(320, 180),
            rate: Rational::from_int(30),
            preset: 10,
            crf: 35,
            keyint: 15,
            threads: 1,
            colour: ColourTags::BT709,
        };
        let mut enc = open_av1_encoder(&cfg).unwrap();
        let mut out = Vec::new();
        for i in 0..20 {
            let p = pattern(i, 320, 180, 9);
            enc.send(
                &RawFrame::i420(Size::new(320, 180), p.y, p.u, p.v),
                i as i64,
                false,
            )
            .unwrap();
            while let Some(pk) = enc.receive().unwrap() {
                out.push(pk);
            }
        }
        enc.finish().unwrap();
        while let Some(pk) = enc.receive().unwrap() {
            out.push(pk);
        }
        (out, enc.config())
    };
    let (a, ca) = run();
    let (b, cb) = run();
    assert_eq!(a.len(), 20, "one temporal unit per frame");
    assert_eq!(a, b);
    assert_eq!(ca, cb);
    assert!(a[0].keyframe && a[15].keyframe && !a[1].keyframe);
    assert_eq!(
        a.iter().map(|p| p.pts).collect::<Vec<_>>(),
        (0..20).collect::<Vec<_>>()
    );
}

#[test]
fn other_codecs_are_unavailable() {
    let caps = capabilities();
    assert!(
        caps.get(VideoCodec::H264)
            .is_some_and(|c| c.decode == fin_codecs::Support::None)
    );
}
