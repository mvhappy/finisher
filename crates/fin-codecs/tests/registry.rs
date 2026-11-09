//! Backend choice under the encoder policy and `ENCODER_HUNG` (plan 6B-1 Task 3), with fakes.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use fin_codecs::fixture::{ClipSpec, encode_all, fixture_config, pattern_clip};
use fin_codecs::testing::fake::{
    DecodeBehaviour, EncodeBehaviour, FakeDecoderBackend, FakeEncoderBackend,
};
use fin_codecs::{
    AdapterBinding, DecoderBackend, DecoderConfig, EncoderBackend, EncoderSession, FallbackNote,
    FallbackReason, HUNG_FRAMES, Output, ProbeResult, RawFrame, StreamDesc, Support, capabilities,
    open_decoder, open_encoder,
};
use fin_container::{Demuxer, Limits};
use fin_core::CoreError;
use fin_media::backend::EncodePurpose;
use fin_media::jobs::Profile;
use fin_media::{Chroma, Size, VideoCodec};

use VideoCodec::{Av1, H264, Hevc};

/// Short enough for tests, long enough for a loaded CI machine's scheduler.
const DEADLINE: Duration = Duration::from_secs(1);

fn probe(dec: Vec<Arc<dyn DecoderBackend>>, enc: Vec<Arc<dyn EncoderBackend>>) -> ProbeResult {
    ProbeResult::new(dec, enc)
}

fn av1_track() -> fin_container::VideoTrack {
    let clip = pattern_clip(&ClipSpec {
        frames: 3,
        ..ClipSpec::default()
    })
    .unwrap();
    Demuxer::open(Cursor::new(clip), &Limits::default())
        .unwrap()
        .video()
        .cloned()
        .unwrap()
}

fn stream(codec: VideoCodec, bit_depth: u8) -> StreamDesc {
    StreamDesc {
        codec,
        profile: None,
        bit_depth,
        chroma: Chroma::Yuv420,
    }
}

fn cfg(codec: VideoCodec, purpose: EncodePurpose) -> fin_codecs::EncoderConfig {
    let mut c = fixture_config(
        &ClipSpec {
            size: Size::new(64, 64),
            ..ClipSpec::default()
        },
        codec,
    );
    c.purpose = purpose;
    c
}

fn frame() -> RawFrame {
    RawFrame::i420(
        Size::new(64, 64),
        vec![16; 64 * 64],
        vec![128; 32 * 32],
        vec![128; 32 * 32],
    )
}

/// Encodes `n` frames and returns the pts of the packets in order.
fn run(enc: &mut EncoderSession, n: u32) -> Result<Vec<i64>, CoreError> {
    let packets = encode_all(enc, n, |_| frame())?;
    for p in &packets {
        assert_eq!(
            p.data,
            p.pts.to_le_bytes(),
            "a packet carries its frame's pts"
        );
    }
    Ok(packets.iter().map(|p| p.pts).collect())
}

#[test]
fn open_decoder_prefers_os_then_software() {
    let track = av1_track();
    let vt = FakeDecoderBackend::new("videotoolbox", true, &[Av1, H264]).arc();
    let hw = FakeDecoderBackend::new("mf-hw", true, &[Av1]).arc();
    let sw = FakeDecoderBackend::new("dav1d", false, &[Av1])
        .max_depth(12)
        .arc();
    let p = probe(vec![sw.clone(), hw.clone(), vt.clone()], vec![]);
    let names = |s: &StreamDesc| -> Vec<&str> {
        p.decoder_candidates(s, None)
            .iter()
            .map(|d| d.entry().name)
            .collect()
    };
    assert_eq!(names(&stream(Av1, 8)), ["videotoolbox", "mf-hw", "dav1d"]);
    // A depth the hardware fakes do not decode falls to software.
    assert_eq!(names(&stream(Av1, 12)), ["dav1d"]);
    let dec = open_decoder(&DecoderConfig::new(&track), &p).unwrap();
    assert_eq!(dec.backend().0, "videotoolbox");
    assert_eq!(vt.opened.load(Ordering::SeqCst), 1);

    // One that fails to open gives way to the next.
    let broken = FakeDecoderBackend::new("videotoolbox", true, &[Av1])
        .behaving(DecodeBehaviour::FailOpen)
        .arc();
    let p = probe(vec![sw.clone(), broken], vec![]);
    let dec = open_decoder(&DecoderConfig::new(&track), &p).unwrap();
    assert_eq!(dec.backend().0, "dav1d");

    // Nothing decodes it: CODEC_UNAVAILABLE.
    let p = probe(
        vec![FakeDecoderBackend::new("mf-ms", false, &[H264]).arc()],
        vec![],
    );
    let e = open_decoder(&DecoderConfig::new(&track), &p).err().unwrap();
    assert_eq!(e.code, "CODEC_UNAVAILABLE");
    assert_eq!(e.details["codec"], "av1");
}

#[test]
fn software_only_decodes_the_6a_way() {
    let track = av1_track();
    let dec = open_decoder(&DecoderConfig::new(&track), &ProbeResult::software_only()).unwrap();
    assert_eq!(dec.backend().0, "dav1d");
    let caps = capabilities();
    let av1 = caps.get(Av1).unwrap();
    assert_eq!(
        (av1.decode, av1.encode),
        (Support::Software, Support::Software)
    );
    assert_eq!(av1.backends, ["dav1d", "svt-av1"]);
    for c in [H264, Hevc, VideoCodec::Vp9, VideoCodec::ProRes] {
        let row = caps.get(c).unwrap();
        assert_eq!(
            (row.decode, row.encode),
            (Support::None, Support::None),
            "{c:?}"
        );
        assert!(row.backends.is_empty());
    }
    let e = open_encoder(
        &cfg(H264, EncodePurpose::Proxy),
        &ProbeResult::software_only(),
    )
    .err()
    .unwrap();
    assert_eq!(
        (e.code.as_str(), &e.details["direction"]),
        ("CODEC_UNAVAILABLE", &"encode".into())
    );
}

#[test]
fn gpu_candidate_skipped_for_export() {
    let gpu = FakeEncoderBackend::new("mf-hw", true, &[H264]).arc();
    let ms = FakeEncoderBackend::new("mf-ms", false, &[H264]).arc();
    let p = probe(vec![], vec![ms.clone(), gpu.clone()]);
    for purpose in [
        EncodePurpose::Export,
        EncodePurpose::QaRender,
        EncodePurpose::Matte,
    ] {
        let s = open_encoder(&cfg(H264, purpose), &p).unwrap();
        assert_eq!(s.entry().name, "mf-ms", "{purpose:?}");
    }
    // A proxy takes the hardware encoder first, gpu class included (F3).
    let s = open_encoder(&cfg(H264, EncodePurpose::Proxy), &p).unwrap();
    assert_eq!((s.entry().name, s.hw()), ("mf-hw", true));
    assert_eq!(gpu.opened.load(Ordering::SeqCst), 1);

    // Only a gpu encoder: an export is refused with the policy's reason.
    let p = probe(vec![], vec![gpu]);
    let e = open_encoder(&cfg(H264, EncodePurpose::Export), &p)
        .err()
        .unwrap();
    assert_eq!(e.code, "CODEC_UNAVAILABLE");
    assert_eq!(e.details["reason"], "gpuForDistributed");
    assert_eq!(e.details["backend"], "mf-hw");
}

#[test]
fn hevc_encoder_refused_outside_test_fixture() {
    let vt = FakeEncoderBackend::new("videotoolbox", true, &[H264, Hevc]).arc();
    let p = probe(vec![], vec![vt]);
    for purpose in [EncodePurpose::Proxy, EncodePurpose::Export] {
        let e = open_encoder(&cfg(Hevc, purpose), &p).err().unwrap();
        assert_eq!(e.details["reason"], "hevcBuiltIn", "{purpose:?}");
    }
    let s = open_encoder(&cfg(Hevc, EncodePurpose::TestFixture), &p).unwrap();
    assert_eq!(s.entry().name, "videotoolbox");
    // HEVC is not on mf-ms's row of the backend table: it is never a candidate.
    let ms = FakeEncoderBackend::new("mf-ms", false, &[Hevc]).arc();
    let e = open_encoder(
        &cfg(Hevc, EncodePurpose::TestFixture),
        &probe(vec![], vec![ms]),
    )
    .err()
    .unwrap();
    assert_eq!(e.code, "CODEC_UNAVAILABLE");
}

#[test]
fn profile_reaches_the_candidates() {
    let p = ProbeResult::software_only();
    assert_eq!(
        p.encoder_candidates(Av1, Some(Profile::Main), None).len(),
        1
    );
    assert!(
        p.encoder_candidates(Av1, Some(Profile::High), None)
            .is_empty()
    );
}

fn hanging_pair(first: EncodeBehaviour) -> ProbeResult {
    let hw = FakeEncoderBackend::new("mf-hw", true, &[H264])
        .behaving(first)
        .arc();
    let ms = FakeEncoderBackend::new("mf-ms", false, &[H264]).arc();
    probe(vec![], vec![hw, ms])
}

#[test]
fn hung_encoder_falls_back() {
    for first in [
        EncodeBehaviour::HangAfter(3),
        EncodeBehaviour::StallAfter(3),
    ] {
        let p = hanging_pair(first);
        let mut s = open_encoder(&cfg(H264, EncodePurpose::Proxy), &p)
            .unwrap()
            .with_hang_deadline(DEADLINE);
        let t = Instant::now();
        let got = run(&mut s, 10).unwrap();
        assert_eq!(
            got,
            (0..10).collect::<Vec<_>>(),
            "{first:?}: every frame, in order"
        );
        assert_eq!(s.entry().name, "mf-ms");
        assert_eq!(
            s.fell_back(),
            [FallbackNote {
                from: "mf-hw",
                to: "mf-ms",
                reason: FallbackReason::Hung
            }],
            "{first:?}"
        );
        // After `End`, `End` again: never a hang on an idle encoder.
        std::thread::sleep(DEADLINE + Duration::from_millis(200));
        assert_eq!(s.receive().unwrap(), Output::End);
        assert!(matches!(
            s.config().unwrap(),
            fin_container::CodecConfig::Other { .. }
        ));
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "{first:?}: {:?}",
            t.elapsed()
        );
    }
}

#[test]
fn two_hangs_fail_with_encoder_hung() {
    let a = FakeEncoderBackend::new("mf-hw", true, &[H264])
        .behaving(EncodeBehaviour::HangAfter(2))
        .arc();
    let b = FakeEncoderBackend::new("videotoolbox", true, &[H264])
        .behaving(EncodeBehaviour::StallAfter(1))
        .arc();
    let c = FakeEncoderBackend::new("mf-ms", false, &[H264]).arc();
    let p = probe(vec![], vec![a, b, c.clone()]);
    let mut s = open_encoder(&cfg(H264, EncodePurpose::Proxy), &p)
        .unwrap()
        .with_hang_deadline(DEADLINE);
    let e = run(&mut s, 6).unwrap_err();
    assert_eq!(e.code, "ENCODER_HUNG");
    // Hardware first, then by class: videotoolbox (os) stalls, then mf-hw (gpu) hangs.
    assert_eq!(e.details["backend"], "mf-hw");
    assert!(e.details["adapter"].is_null());
    assert_eq!(
        c.opened.load(Ordering::SeqCst),
        0,
        "no third encoder after two hangs"
    );
}

#[test]
fn a_hang_with_no_candidate_left_is_encoder_hung() {
    let a = FakeEncoderBackend::new("mf-hw", true, &[H264])
        .behaving(EncodeBehaviour::HangAfter(1))
        .arc();
    let mut s = open_encoder(&cfg(H264, EncodePurpose::Proxy), &probe(vec![], vec![a]))
        .unwrap()
        .with_hang_deadline(DEADLINE);
    assert_eq!(run(&mut s, 4).unwrap_err().code, "ENCODER_HUNG");
}

#[test]
fn an_encoder_that_fails_to_open_gives_way() {
    let a = FakeEncoderBackend::new("mf-hw", true, &[H264])
        .behaving(EncodeBehaviour::FailOpen)
        .arc();
    let b = FakeEncoderBackend::new("mf-ms", false, &[H264]).arc();
    let mut s = open_encoder(&cfg(H264, EncodePurpose::Proxy), &probe(vec![], vec![a, b])).unwrap();
    assert_eq!(run(&mut s, 3).unwrap(), [0, 1, 2]);
    assert_eq!(s.fell_back()[0].reason, FallbackReason::OpenFailed);
    assert_eq!(
        (s.fell_back()[0].from, s.fell_back()[0].to),
        ("mf-hw", "mf-ms")
    );
}

#[test]
fn svt_through_the_registry_keeps_its_config() {
    let spec = ClipSpec {
        frames: 4,
        size: Size::new(64, 64),
        ..ClipSpec::default()
    };
    let mut s = open_encoder(&fixture_config(&spec, Av1), &ProbeResult::software_only()).unwrap();
    assert_eq!((s.entry().name, s.hw()), ("svt-av1", false));
    let packets = encode_all(&mut s, 4, |_| frame()).unwrap();
    assert_eq!(packets.len(), 4);
    assert!(packets[0].keyframe);
    assert!(matches!(
        s.config().unwrap(),
        fin_container::CodecConfig::Av1 { .. }
    ));
}

#[test]
fn an_encoder_holding_too_many_frames_is_hung() {
    let p = hanging_pair(EncodeBehaviour::StallAfter(0));
    let mut s = open_encoder(&cfg(H264, EncodePurpose::Proxy), &p)
        .unwrap()
        .with_hang_deadline(DEADLINE);
    let n = HUNG_FRAMES + 5;
    assert_eq!(run(&mut s, n).unwrap(), (0..n as i64).collect::<Vec<_>>());
    assert_eq!(s.fell_back()[0].reason, FallbackReason::Hung);
}

#[test]
fn a_session_bound_to_an_adapter_takes_only_backends_on_it() {
    struct On(FakeEncoderBackend, AdapterBinding);
    impl EncoderBackend for On {
        fn entry(&self) -> &'static fin_media::backend::BackendEntry {
            self.0.entry()
        }
        fn hw(&self) -> bool {
            true
        }
        fn adapter(&self) -> Option<AdapterBinding> {
            Some(self.1)
        }
        fn encodes(&self, c: VideoCodec, p: Option<Profile>) -> bool {
            self.0.encodes(c, p)
        }
        fn open(
            &self,
            c: &fin_codecs::EncoderConfig,
            t: fin_codecs::OpenToken,
        ) -> Result<Box<dyn fin_codecs::VideoEncoder>, CoreError> {
            self.0.open(c, t)
        }
    }
    let gpu = |luid| AdapterBinding {
        id: fin_media::jobs::AdapterId {
            vendor_id: 0x1002,
            device_id: 1,
            driver_major: None,
        },
        luid: Some(luid),
    };
    let other = Arc::new(On(FakeEncoderBackend::new("mf-hw", true, &[H264]), gpu(7)));
    let ms = FakeEncoderBackend::new("mf-ms", false, &[H264]).arc();
    let p = probe(vec![], vec![other, ms]);
    let mut c = cfg(H264, EncodePurpose::Proxy);
    c.adapter = Some(gpu(9));
    assert_eq!(open_encoder(&c, &p).unwrap().entry().name, "mf-ms");
    c.adapter = Some(gpu(7));
    assert_eq!(open_encoder(&c, &p).unwrap().entry().name, "mf-hw");
}
