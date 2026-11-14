//! SVT-AV1 takes NV12 as well as I420 (plan 6B-1 Task 14: the proxy pipeline hands every
//! encoder the compositor's NV12): the same pictures in either layout give the same packets.

use fin_codecs::fixture::{ClipSpec, encode_all, fixture_config};
use fin_codecs::{ProbeResult, RawFrame, open_encoder};
use fin_media::backend::EncodePurpose;
use fin_media::testing::pattern;
use fin_media::{Size, VideoCodec};

#[test]
fn nv12_and_i420_encode_alike() {
    let spec = ClipSpec {
        frames: 6,
        size: Size::new(128, 72),
        ..ClipSpec::default()
    };
    // Test clips are encoded for `TestFixture` (the call-site gate's rule).
    assert_eq!(
        fixture_config(&spec, VideoCodec::Av1).purpose,
        EncodePurpose::TestFixture
    );
    let encode = |nv12: bool| {
        let mut enc = open_encoder(
            &fixture_config(&spec, VideoCodec::Av1),
            &ProbeResult::software_only(),
        )
        .unwrap();
        encode_all(&mut enc, spec.frames, |i| {
            let p = pattern(i, spec.size.width, spec.size.height, spec.seed);
            if nv12 {
                let uv = p.u.iter().zip(&p.v).flat_map(|(u, v)| [*u, *v]).collect();
                RawFrame::nv12(spec.size, p.y, uv)
            } else {
                RawFrame::i420(spec.size, p.y, p.u, p.v)
            }
        })
        .unwrap()
    };
    let (a, b) = (encode(false), encode(true));
    assert_eq!(a.len(), spec.frames as usize);
    assert_eq!(a, b);
}

#[test]
fn other_layouts_and_sizes_are_refused() {
    let spec = ClipSpec {
        frames: 1,
        size: Size::new(128, 72),
        ..ClipSpec::default()
    };
    let mut enc = open_encoder(
        &fixture_config(&spec, VideoCodec::Av1),
        &ProbeResult::software_only(),
    )
    .unwrap();
    let p = pattern(0, 128, 72, 1);
    // A short interleaved plane.
    let short = RawFrame::nv12(spec.size, p.y.clone(), vec![128; 10]);
    // The session reports the encoder's refusal on a later call.
    let e = enc
        .send(short, 0, false)
        .and_then(|_| enc.finish())
        .and_then(|_| {
            loop {
                if let fin_codecs::Output::End = enc.receive()? {
                    break Ok(());
                }
            }
        })
        .unwrap_err();
    assert_eq!(e.code, "ENCODE_FAILED", "{e:?}");
}
