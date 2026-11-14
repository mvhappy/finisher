//! The encode pipeline over `fin-codecs`' fake backends (feature `testing`, a dev-dependency):
//! a proxy may take a `gpu` encoder (F3), an export never could, a hung encoder restarts the
//! output on the next one, and every frame comes out once.

use std::sync::Arc;
use std::time::Duration;

use fin_codecs::testing::fake::{EncodeBehaviour, FakeEncoderBackend};
use fin_codecs::{DecoderBackend, EncoderBackend, Gop, RateControl};
use fin_container::{Demuxer, Limits};
use fin_media::backend::{EncodePurpose, VendorClass};
use fin_media::{Size, VideoCodec};

use super::*;

struct Src {
    dir: tempfile::TempDir,
    frames_sent: Vec<u64>,
    outputs: u32,
}

impl Src {
    fn new() -> Src {
        Src {
            // Canonical: the checked opens refuse a link at any segment (macOS `/var`).
            dir: tempfile::tempdir().unwrap(),
            frames_sent: Vec::new(),
            outputs: 0,
        }
    }

    fn path(&self) -> std::path::PathBuf {
        let root = fin_shm::JobDir::resolve(&self.dir.path().join("x"))
            .unwrap()
            .root;
        root.with_file_name("proxy.mp4")
    }
}

impl Frames for Src {
    fn frame(&mut self, i: u64) -> Result<RawFrame, CoreError> {
        self.frames_sent.push(i);
        let size = Size::new(64, 64);
        Ok(RawFrame::nv12(size, vec![16; 64 * 64], vec![128; 64 * 32]))
    }

    fn between(&mut self, _done: u64) -> Result<(), CoreError> {
        Ok(())
    }

    fn output(&mut self) -> Result<File, CoreError> {
        self.outputs += 1;
        fin_shm::create_output(&self.path())
    }
}

fn cfg(codec: VideoCodec, purpose: EncodePurpose) -> EncoderConfig {
    EncoderConfig {
        codec,
        profile: None,
        size: Size::new(64, 64),
        rate: Rational::from_int(30),
        gop: Gop {
            key_every_frames: 7,
            closed: true,
            intra_only: false,
        },
        rc: RateControl::Crf(30),
        purpose,
        adapter: None,
        preset: Some(10),
        threads: 1,
        colour: ColourTags::BT709,
    }
}

const TRACK: Track = Track {
    timescale: 30_000,
    tick: 1000,
    colour: ColourTags::BT709,
};

fn probe(encoders: Vec<Arc<dyn EncoderBackend>>) -> ProbeResult {
    ProbeResult::new(Vec::<Arc<dyn DecoderBackend>>::new(), encoders)
}

/// F3: a GPU vendor's hardware encoder serves a proxy (and is preferred: hardware first).
#[test]
fn proxy_may_use_gpu_encoder() {
    let gpu = FakeEncoderBackend::new("mf-hw", true, &[VideoCodec::H264]).arc();
    let caps = probe(vec![gpu.clone(), Arc::new(fin_codecs::av1::SvtBackend)]);
    let mut src = Src::new();
    let out = encode(
        &caps,
        &cfg(VideoCodec::H264, EncodePurpose::Proxy),
        &TRACK,
        20,
        &mut src,
    )
    .unwrap();
    assert_eq!(out.entry.name, "mf-hw");
    assert_eq!(out.entry.class, VendorClass::Gpu);
    assert!(out.fell_back.is_empty());
    assert_eq!(src.frames_sent, (0..20).collect::<Vec<_>>());
    // One sample per frame at i × tick, keyframe as the encoder said.
    let d = Demuxer::open(
        fin_shm::checked_open(&src.path(), u64::MAX).unwrap(),
        &Limits::default(),
    )
    .unwrap();
    let t = &d.video().unwrap().table;
    assert_eq!(t.len(), 20);
    assert!(
        t.frames()
            .iter()
            .enumerate()
            .all(|(i, f)| f.pts == i as i64 * 1000)
    );
    // The same encoder for an export is refused by the policy (`gpuForDistributed`), and the
    // pipeline encodes proxies only.
    let e = fin_codecs::open_encoder(&cfg(VideoCodec::H264, EncodePurpose::Export), &caps)
        .err()
        .unwrap();
    assert_eq!(e.code, "CODEC_UNAVAILABLE");
    assert_eq!(e.details["reason"], "gpuForDistributed");
    let e = encode(
        &caps,
        &cfg(VideoCodec::H264, EncodePurpose::Export),
        &TRACK,
        2,
        &mut Src::new(),
    )
    .unwrap_err();
    assert_eq!(e.code, "INTERNAL");
}

/// No H.264 encoder at all (Linux before 6B-2): typed, with the guidance of `open_encoder`.
#[test]
fn no_encoder_for_the_codec_is_codec_unavailable() {
    let caps = ProbeResult::software_only();
    let e = encode(
        &caps,
        &cfg(VideoCodec::H264, EncodePurpose::Proxy),
        &TRACK,
        2,
        &mut Src::new(),
    )
    .unwrap_err();
    assert_eq!(e.code, "CODEC_UNAVAILABLE");
    assert_eq!(e.details["direction"], "encode");
    assert!(e.fix.is_some());
}

/// Decision 20: an encoder that hangs mid-stream is left behind and the output made again
/// from frame 0 on the next one; the result names the move.
#[test]
fn a_hung_encoder_restarts_the_output_on_the_next() {
    let hung = FakeEncoderBackend::new("mf-hw", true, &[VideoCodec::H264])
        .behaving(EncodeBehaviour::HangAfter(5))
        .arc();
    let ms = FakeEncoderBackend::new("mf-ms", false, &[VideoCodec::H264]).arc();
    let caps = probe(vec![hung, ms]);
    let mut src = Src::new();
    let mut c = cfg(VideoCodec::H264, EncodePurpose::Proxy);
    c.rate = Rational::from_int(1000); // the shortest deadline the policy allows (2 s)
    let started = std::time::Instant::now();
    let out = encode(&caps, &c, &TRACK, 12, &mut src).unwrap();
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(out.entry.name, "mf-ms");
    assert_eq!(out.fell_back.len(), 1);
    assert_eq!(out.fell_back[0].from, "mf-hw");
    assert_eq!(out.fell_back[0].reason, "hung");
    assert_eq!(src.outputs, 2, "the output was made again");
    let d = Demuxer::open(
        fin_shm::checked_open(&src.path(), u64::MAX).unwrap(),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(d.video().unwrap().table.len(), 12);
}

#[test]
fn rates_become_cfr_timing() {
    assert_eq!(timing(Rational::from_int(30)).unwrap(), (30_000, 1000));
    assert_eq!(
        timing(Rational::new(30_000, 1001).unwrap()).unwrap(),
        (30_000_000, 1_001_000)
    );
    // Too large for the ×1000 form: the rate's own terms.
    assert_eq!(
        timing(Rational::new(10_000_000, 3).unwrap()).unwrap(),
        (10_000_000, 3)
    );
    assert_eq!(
        timing(Rational::new(i64::from(u32::MAX) + 1, 1).unwrap())
            .unwrap_err()
            .code,
        "MEDIA_LIMIT"
    );
    assert_eq!(timing(Rational::zero()).unwrap_err().code, "MEDIA_CORRUPT");
}
