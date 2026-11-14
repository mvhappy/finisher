//! `media.probeDeep` v1 (plan 6B-1 Task 11), driven over the envelope as a host drives it
//! (`fin-workers media`, cold): the first frame decoded through `open_decoder`, the backend and
//! its vendor class reported, in-band HDR SEI read when the container has no HDR boxes, and a
//! manifest whose fingerprint is not the submitted one refused.
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::io::Cursor;

use fin_container::{CodecConfig, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::Rational;
use fin_jobs::HostMsg;
use fin_media::backend::VendorClass;
use fin_media::jobs::{
    FirstFrame, HdrSource, MANIFEST_VERSION, ManifestAsset, ManifestFonts, OriginalFile,
    ProbeDeepParams, ProbeDeepResult, RenderManifest,
};
use fin_media::{ColourTags, PixelFormat, Rotation, Size, VideoCodec};
use media_support::{CLIP_SIZE, Fixture};
use serde_json::Value;
use support::{Job, Proc, Run, expect_of, no_calls, spawn_with};

fn params(asset: &str, fingerprint: &str) -> ProbeDeepParams {
    ProbeDeepParams {
        asset: asset.into(),
        fingerprint: fingerprint.into(),
        project_id: None,
        branch: None,
        display: None,
        render_version: None,
        caps_digest: None,
    }
}

fn probe(job: &Job, p: &ProbeDeepParams) -> Run {
    let mut w: Proc = spawn_with(&["media"], &[]);
    w.hello();
    let start = job.start(
        "media.probeDeep",
        serde_json::to_value(p).unwrap(),
        &["in/manifest.json"],
    );
    w.send(&HostMsg::Start(start));
    w.drive(no_calls)
}

fn result(run: &Run) -> ProbeDeepResult {
    serde_json::from_value(run.result().clone()).unwrap()
}

fn original(p: &std::path::Path) -> OriginalFile {
    let e = serde_json::to_value(expect_of(p)).unwrap();
    OriginalFile {
        path: p.to_str().unwrap().into(),
        size: e["size"].as_u64().unwrap(),
        mtime_ns: e["mtimeNs"].as_str().unwrap().into(),
        file_id: e.get("fileId").and_then(Value::as_str).map(str::to_owned),
    }
}

/// A job whose manifest names only `asset` (fingerprint `fp`) at `path`.
fn job_for(asset: &str, fp: &str, path: &std::path::Path) -> Job {
    let job = Job::new();
    let m = RenderManifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestAsset {
            asset: asset.into(),
            kind: fin_core::AssetKind::Media,
            fingerprint: fp.into(),
            original: Some(original(path)),
            proxies: Vec::new(),
        }],
        fonts: ManifestFonts::default(),
        pcm: Default::default(),
    };
    std::fs::write(
        job.path("in/manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    job
}

#[test]
fn probe_deep_reports_backend_and_class() {
    let fx = Fixture::new();
    let job = fx.job(None);
    let run = probe(&job, &params(&fx.av1_id, "fp-av1"));
    let r = result(&run);
    assert_eq!(r.asset, fx.av1_id);
    assert_eq!(r.fingerprint, "fp-av1");
    assert_eq!(r.codec, VideoCodec::Av1);
    assert!(r.decodable, "{:?}", r.failure);
    // 6A's software pair until 6B-1 Task 8's probe: dav1d, a `software` backend.
    assert_eq!(r.backend.as_deref(), Some("dav1d"));
    assert_eq!(r.class, Some(VendorClass::Software));
    assert_eq!(r.hw, Some(false));
    assert_eq!(
        r.first_frame,
        Some(FirstFrame {
            width: CLIP_SIZE.width,
            height: CLIP_SIZE.height,
            format: PixelFormat::I420,
        })
    );
    assert_eq!((r.hdr, r.hdr_source, r.failure), (None, None, None));
}

/// SEI messages (type, payload) as one escaped H.264 SEI NAL unit, length-prefixed.
fn h264_sei_sample(messages: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut rbsp = Vec::new();
    for (kind, payload) in messages {
        rbsp.push(*kind as u8);
        rbsp.push(payload.len() as u8);
        rbsp.extend(payload);
    }
    rbsp.push(0x80);
    let mut nal = vec![0x06];
    let mut zeros = 0;
    for b in rbsp {
        if zeros >= 2 && b <= 3 {
            nal.push(3);
            zeros = 0;
        }
        nal.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    let mut out = (nal.len() as u32).to_be_bytes().to_vec();
    out.extend(nal);
    out.extend([0, 0, 0, 3, 0x65, 0x88, 0x80]);
    out
}

#[test]
fn h264_with_sei_reports_hdr_and_why_it_does_not_decode() {
    let fx = Fixture::new();
    // MaxCLL 1000, MaxFALL 400 only (no mastering message), and no `clli` box.
    let sample = h264_sei_sample(&[(144, vec![0x03, 0xE8, 0x01, 0x90])]);
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let t = w
        .add_video(VideoTrackConfig {
            config: CodecConfig::Avc {
                avcc: vec![1, 100, 0, 40, 0xff, 0xe0, 0],
            },
            coded: Size::new(1920, 1080),
            timescale: 30_000,
            rotation: Rotation::R0,
            pixel_aspect: Rational::from_int(1),
            colour: ColourTags::BT709,
            hdr: None,
        })
        .unwrap();
    w.write(t, &sample, 0, 0, 1001, true).unwrap();
    let path = fx.dir.path().join("hdr.mp4");
    std::fs::write(&path, w.finish().unwrap().into_inner()).unwrap();
    let job = job_for("a9", "fp-hdr", &path);
    let r = result(&probe(&job, &params("a9", "fp-hdr")));
    assert_eq!(r.codec, VideoCodec::H264);
    assert_eq!(r.hdr_source, Some(HdrSource::Sei));
    let light = r.hdr.and_then(|h| h.content_light).unwrap();
    assert_eq!((light.max_cll, light.max_fall), (1000, 400));
    // No H.264 decoder on a software-only machine (Linux until 6B-2): the probe says so and
    // still succeeds; on macOS and Windows an OS decoder may take it once Task 8 probes it.
    if !r.decodable {
        let f = r.failure.unwrap();
        assert!(
            ["CODEC_UNAVAILABLE", "DECODE_FAILED", "MEDIA_CORRUPT"].contains(&f.code.as_str()),
            "{f:?}"
        );
        assert_eq!((r.backend, r.class, r.first_frame), (None, None, None));
    }
}

#[test]
fn a_changed_fingerprint_is_asset_changed() {
    let fx = Fixture::new();
    let job = fx.job(None);
    let run = probe(&job, &params(&fx.av1_id, "fp-other"));
    let e = run.error();
    assert_eq!(e.code, "ASSET_CHANGED");
    assert_eq!(e.details["asset"], fx.av1_id.as_str());
}

#[test]
fn an_asset_not_in_the_manifest_or_missing_is_typed() {
    let fx = Fixture::new();
    let run = probe(&fx.job(None), &params("a404", "fp"));
    assert_eq!(run.error().code, "JOB_INPUT_INVALID");
    // The file changed after the manifest was written: the no-follow opener refuses it.
    let job = fx.job(None);
    std::fs::write(&fx.av1, b"rewritten").unwrap();
    let run = probe(&job, &params(&fx.av1_id, "fp-av1"));
    let e = run.error();
    assert_eq!(e.code, "ORIGINALS_MISSING");
    assert_eq!(e.details["reason"], "changed");
}
