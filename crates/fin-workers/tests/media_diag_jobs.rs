//! The cold `media` kinds' diagnostics (6B-1 plan Task 19): `media.proxy`, `media.thumbs` and
//! `media.probeDeep` put `job.done`'s `media` object in their result with `start.diagnostics` on
//! (and nothing with it off), and their stage timings go out as `render.stats`, the proxy's
//! encoder in `encode` rows. Every payload decodes as its registered kind and holds no path.
//! The proxy and thumbnail runs need a GPU adapter (`FIN_GPU`; skipped with a notice without one,
//! failed with `FIN_REQUIRE_GPU=1`).
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::path::{Path, PathBuf};

use fin_codecs::fixture::{ClipSpec, pattern_clip};
use fin_core::Rational;
use fin_core::events::payloads::{CopyPath, JobMedia, ResolutionBucket};
use fin_core::events::{decode_payload, kind};
use fin_jobs::HostMsg;
use fin_media::Size;
use fin_media::backend::VendorClass;
use fin_media::jobs::{
    MANIFEST_VERSION, ManifestAsset, ManifestFonts, OriginalFile, ProbeDeepParams, ProxyCodec,
    ProxyParams, ProxySourceAudio, ProxyTier, RenderManifest, ThumbsMode, ThumbsParams, proxy_key,
    thumbs_key,
};
use media_support::{gpu_or_skip, worker_env};
use serde_json::Value;
use support::{End, Job, Proc, Run, expect_of, no_calls, spawn_with};

struct Clip {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn clip(frames: u32, size: Size) -> Clip {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap().join("clip.mp4");
    let spec = ClipSpec {
        frames,
        size,
        keyint: 10,
        ..ClipSpec::default()
    };
    std::fs::write(&path, pattern_clip(&spec).unwrap()).unwrap();
    Clip { _dir: dir, path }
}

fn original(p: &Path) -> OriginalFile {
    let e = serde_json::to_value(expect_of(p)).unwrap();
    OriginalFile {
        path: p.to_str().unwrap().into(),
        size: e["size"].as_u64().unwrap(),
        mtime_ns: e["mtimeNs"].as_str().unwrap().into(),
        file_id: e.get("fileId").and_then(Value::as_str).map(str::to_owned),
    }
}

/// A job whose manifest names asset `a1` (fingerprint `fp`) at `path`.
fn job_for(path: &Path) -> Job {
    let job = Job::new();
    let m = RenderManifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestAsset {
            asset: "a1".into(),
            kind: fin_core::AssetKind::Media,
            fingerprint: "fp".into(),
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

fn run(job: &Job, kind: &str, params: Value, grants: &[&str], diagnostics: bool) -> Run {
    let mut w: Proc = spawn_with(&["media"], &worker_env());
    w.hello();
    let mut start = job.start(kind, params, grants);
    start.diagnostics = diagnostics;
    w.send(&HostMsg::Start(start));
    w.drive(no_calls)
}

fn proxy(job: &Job, diagnostics: bool) -> Run {
    let tier = ProxyTier::Scrub;
    let p = ProxyParams {
        fingerprint: "fp".into(),
        tier,
        codec: ProxyCodec::Av1,
        decoder_class: VendorClass::Software,
        audio: ProxySourceAudio::None,
        key: proxy_key(
            "fp",
            tier,
            ProxyCodec::Av1,
            VendorClass::Software,
            ProxySourceAudio::None,
        )
        .to_string(),
        asset: Some("a1".into()),
        file: None,
        project_id: None,
        branch: None,
        display: None,
        intra_ok: false,
    };
    let params = serde_json::to_value(p).unwrap();
    run(
        job,
        "media.proxy",
        params,
        &["in/manifest.json", "out/"],
        diagnostics,
    )
}

/// The result's `media` object, decoded as `job.done`'s.
fn media_of(run: &Run) -> Option<JobMedia> {
    assert!(matches!(run.end, End::Result(_)), "{:?}", run.end);
    run.result()
        .get("media")
        .map(|m| serde_json::from_value(m.clone()).unwrap())
}

/// `(phase, backend, count)` of every `render.stats` sent; each decodes as the kind.
fn stats(run: &Run) -> Vec<(String, String, u64)> {
    let spec = kind("render.stats", 1).unwrap();
    run.diags("render.stats")
        .into_iter()
        .map(|s| {
            decode_payload(spec, s).unwrap_or_else(|e| panic!("{e}: {s}"));
            (
                s["phase"].as_str().unwrap().to_owned(),
                s["backend"].as_str().unwrap().to_owned(),
                s["histogram"]["count"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn count(stats: &[(String, String, u64)], phase: &str, backend: &str) -> u64 {
    stats
        .iter()
        .filter(|(p, b, _)| p == phase && b == backend)
        .map(|(_, _, n)| n)
        .sum()
}

/// Nothing the worker sent names the clip's directory.
fn no_path(run: &Run, clip: &Clip) {
    let dir = clip.path.parent().unwrap().to_str().unwrap();
    let mut sent = run
        .result()
        .get("media")
        .map_or(String::new(), Value::to_string);
    for e in &run.events {
        if let support::Ev::Diag(_, v) = e {
            sent.push_str(&v.to_string());
        }
    }
    assert!(!sent.contains(dir) && !sent.contains("clip.mp4"), "{sent}");
}

#[test]
fn a_proxy_reports_its_media_object_and_encode_stats() {
    if !gpu_or_skip("a_proxy_reports_its_media_object_and_encode_stats") {
        return;
    }
    let c = clip(12, Size::new(1280, 720));
    let run = proxy(&job_for(&c.path), true);
    let m = media_of(&run).expect("a media object with diagnostics on");
    assert_eq!((m.codec.as_str(), m.backend.as_str()), ("av1", "dav1d"));
    assert!(!m.hw && !m.cache_hit);
    assert_eq!(m.frames, 12);
    // 1280×720 into 960×540.
    assert_eq!(m.resolution, ResolutionBucket::Sd);
    assert_eq!(m.copy_path, CopyPath::Copy);
    assert!(m.rt_factor_milli > 0, "{m:?}");
    let s = stats(&run);
    assert_eq!(count(&s, "decode", "dav1d"), 12, "{s:?}");
    // One row per frame and one for the end of the stream, per encoder backend.
    assert_eq!(count(&s, "encode", "svt-av1"), 13, "{s:?}");
    assert!(s.iter().any(|(p, _, _)| p == "composite"), "{s:?}");
    no_path(&run, &c);
}

#[test]
fn diagnostics_off_the_cold_kinds_send_and_build_nothing() {
    if !gpu_or_skip("diagnostics_off_the_cold_kinds_send_and_build_nothing") {
        return;
    }
    let c = clip(4, Size::new(320, 180));
    let job = job_for(&c.path);
    let quiet = |run: &Run| {
        assert!(media_of(run).is_none(), "{:?}", run.result());
        assert!(
            !run.events
                .iter()
                .any(|e| matches!(e, support::Ev::Diag(..)))
        );
    };
    quiet(&proxy(&job, false));
    let probe = serde_json::to_value(probe_params()).unwrap();
    quiet(&run(
        &job,
        "media.probeDeep",
        probe,
        &["in/manifest.json"],
        false,
    ));
}

fn probe_params() -> ProbeDeepParams {
    ProbeDeepParams {
        asset: "a1".into(),
        fingerprint: "fp".into(),
        project_id: None,
        branch: None,
        display: None,
        render_version: None,
        caps_digest: None,
    }
}

#[test]
fn a_deep_probe_reports_its_first_frame() {
    let c = clip(3, Size::new(1920, 1080));
    let params = serde_json::to_value(probe_params()).unwrap();
    let run = run(
        &job_for(&c.path),
        "media.probeDeep",
        params,
        &["in/manifest.json"],
        true,
    );
    let m = media_of(&run).expect("a media object");
    assert_eq!((m.codec.as_str(), m.backend.as_str()), ("av1", "dav1d"));
    assert_eq!((m.frames, m.resolution), (1, ResolutionBucket::Hd1080));
    assert_eq!(m.rt_factor_milli, 0);
    assert_eq!(count(&stats(&run), "decode", "dav1d"), 1);
    no_path(&run, &c);
}

#[test]
fn thumbnails_report_their_media_object() {
    if !gpu_or_skip("thumbnails_report_their_media_object") {
        return;
    }
    let c = clip(30, Size::new(640, 360));
    let mode = ThumbsMode::Thumbs {
        times: vec![Rational::from_int(0), Rational::new(1, 2).unwrap()],
        max_side: 160,
    };
    let p = ThumbsParams {
        fingerprint: "fp".into(),
        asset: "a1".into(),
        key: thumbs_key("fp", &mode, VendorClass::Software).to_string(),
        mode,
        decoder_class: VendorClass::Software,
        only: None,
        project_id: None,
        branch: None,
        display: None,
    };
    let params = serde_json::to_value(p).unwrap();
    let run = run(
        &job_for(&c.path),
        "media.thumbs",
        params,
        &["in/manifest.json", "out/"],
        true,
    );
    let m = media_of(&run).expect("a media object");
    assert_eq!((m.codec.as_str(), m.backend.as_str()), ("av1", "dav1d"));
    assert_eq!((m.frames, m.resolution), (2, ResolutionBucket::Sd));
    assert!(!m.cache_hit);
    let s = stats(&run);
    assert_eq!(count(&s, "decode", "dav1d"), 2, "{s:?}");
    assert!(s.iter().all(|(p, _, _)| p != "encode"), "{s:?}");
    no_path(&run, &c);
}
