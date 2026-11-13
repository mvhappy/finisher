//! `media.proxy` v1 (plan 6B-1 Task 14), driven over the envelope as a host drives it
//! (`fin-workers media`, cold): one proxy frame per presented original frame (Decision 7, read
//! back by frame id for CFR, VFR, edit-listed and rotated clips), tier sizes (Decision 6), the
//! GOP rules read back from the proxy's sync samples (Decision 8), HDR to SDR (Decision 9),
//! rotation kept out of the pixels, the cache key the worker recomputes (Decision 11), library
//! files, and the typed refusals. Needs a GPU adapter for the scale pass (`FIN_GPU`; skipped
//! with a notice without one, failed with `FIN_REQUIRE_GPU=1`).
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::fs::File;
use std::path::{Path, PathBuf};

use fin_codecs::fixture::{ClipSpec, pattern_clip, timing};
use fin_codecs::{FrameReader, ProbeResult};
use fin_container::{Demuxer, Limits};
use fin_core::Rational;
use fin_jobs::HostMsg;
use fin_media::backend::VendorClass;
use fin_media::cache::{EntryMeta, META_FILE};
use fin_media::colour::{ChromaSiting, Matrix, Primaries, Range, Transfer};
use fin_media::jobs::{
    MANIFEST_VERSION, ManifestAsset, ManifestFonts, OriginalFile, ProxyCodec, ProxyLibraryFile,
    ProxyParams, ProxyResult, ProxyTier, RenderManifest, ToneMapped, proxy_key,
};
use fin_media::table_bin::{self, TABLE_FILE};
use fin_media::testing::read_id_y;
use fin_media::{ColourTags, Rotation, Size};
use media_support::{gpu_or_skip, worker_env};
use serde_json::{Value, json};
use support::{End, Job, Proc, Run, expect_of, no_calls, rel, spawn_with};

const PROXY_FILE: &str = "proxy.mp4";

fn original(p: &Path) -> OriginalFile {
    let e = serde_json::to_value(expect_of(p)).unwrap();
    OriginalFile {
        path: p.to_str().unwrap().into(),
        size: e["size"].as_u64().unwrap(),
        mtime_ns: e["mtimeNs"].as_str().unwrap().into(),
        file_id: e.get("fileId").and_then(Value::as_str).map(str::to_owned),
    }
}

/// A clip written into a fresh directory.
struct Clip {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn clip(spec: &ClipSpec) -> Clip {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap().join("clip.mp4");
    std::fs::write(&path, pattern_clip(spec).unwrap()).unwrap();
    Clip { _dir: dir, path }
}

/// A job whose manifest names only asset `a1` (fingerprint `fp`) at `path`.
fn job_for(fp: &str, path: &Path) -> Job {
    let job = Job::new();
    let m = RenderManifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestAsset {
            asset: "a1".into(),
            kind: fin_core::AssetKind::Media,
            fingerprint: fp.into(),
            original: Some(original(path)),
        }],
        fonts: ManifestFonts::default(),
    };
    std::fs::write(
        job.path("in/manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    job
}

fn params(fp: &str, tier: ProxyTier) -> ProxyParams {
    ProxyParams {
        fingerprint: fp.into(),
        tier,
        codec: ProxyCodec::Av1,
        decoder_class: VendorClass::Software,
        key: proxy_key(fp, tier, ProxyCodec::Av1, VendorClass::Software).to_string(),
        asset: Some("a1".into()),
        file: None,
        project_id: None,
        branch: None,
        display: None,
        intra_ok: false,
    }
}

fn run(job: &Job, p: &ProxyParams) -> Run {
    let mut w: Proc = spawn_with(&["media"], &worker_env());
    w.hello();
    let grants: &[&str] = if p.asset.is_some() {
        &["in/manifest.json", "out/"]
    } else {
        &["out/"]
    };
    let start = job.start("media.proxy", serde_json::to_value(p).unwrap(), grants);
    w.send(&HostMsg::Start(start));
    w.drive(no_calls)
}

/// The item's result, or its error code.
fn outcome(run: &Run, fp: &str) -> Result<ProxyResult, String> {
    assert!(matches!(run.end, End::Result(_)), "{:?}", run.end);
    for e in &run.events {
        match e {
            support::Ev::Item(i, Some(v)) if i == fp => {
                return Ok(serde_json::from_value(v.clone()).unwrap());
            }
            support::Ev::ItemFailed(i, code) if i == fp => return Err(code.clone()),
            _ => {}
        }
    }
    panic!("no outcome for {fp}: {:?}", run.events)
}

/// The proxy's directory, `out/<key>/`.
fn entry(job: &Job, key: &str) -> PathBuf {
    job.out().join(key)
}

fn meta(job: &Job, key: &str) -> EntryMeta {
    serde_json::from_slice(&std::fs::read(entry(job, key).join(META_FILE)).unwrap()).unwrap()
}

/// Every frame id of `path` in presentation order (decoded with dav1d).
fn ids(path: &Path) -> Vec<Option<u32>> {
    let mut r = FrameReader::open(
        File::open(path).unwrap(),
        &Limits::default(),
        &ProbeResult::software_only(),
    )
    .unwrap();
    (0..r.table().len())
        .map(|i| {
            let f = r.frame(i).unwrap();
            let p = &f.planes[0];
            let y: Vec<u8> = (0..f.size.height)
                .flat_map(|row| p.row(row, 1).to_vec())
                .collect();
            read_id_y(&y, f.size.width, f.size.height)
        })
        .collect()
}

fn demux(path: &Path) -> Demuxer<File> {
    Demuxer::open(File::open(path).unwrap(), &Limits::default()).unwrap()
}

/// Makes a proxy of `spec` at `tier`: the clip, the job and the result.
fn made(spec: &ClipSpec, tier: ProxyTier) -> (Clip, Job, ProxyResult) {
    let c = clip(spec);
    let job = job_for("fp", &c.path);
    let r = outcome(&run(&job, &params("fp", tier)), "fp").expect("a proxy");
    (c, job, r)
}

fn spec(frames: u32, size: Size) -> ClipSpec {
    ClipSpec {
        frames,
        size,
        keyint: 10,
        ..ClipSpec::default()
    }
}

#[test]
fn proxy_frames_match_original_ids() {
    if !gpu_or_skip("proxy_frames_match_original_ids") {
        return;
    }
    let (n, d) = timing(Rational::from_int(30));
    let tick = d as u64;
    let _ = n;
    let cases = [
        ("cfr", spec(24, Size::new(1280, 720))),
        (
            "vfr",
            ClipSpec {
                vfr: true,
                ..spec(20, Size::new(320, 180))
            },
        ),
        (
            // Frames 5–14, then 0–9 again: one media frame presented twice.
            "edits",
            ClipSpec {
                edits: Some(vec![(10 * tick, Some(5 * tick)), (10 * tick, Some(0))]),
                ..spec(20, Size::new(320, 180))
            },
        ),
        (
            "rotated",
            ClipSpec {
                rotation: Rotation::R90,
                ..spec(16, Size::new(320, 180))
            },
        ),
    ];
    for (name, s) in cases {
        let (c, job, r) = made(&s, ProxyTier::Scrub);
        let want = ids(&c.path);
        let table = demux(&c.path).video().unwrap().table.clone();
        assert_eq!(want.len(), table.len(), "{name}");
        assert!(want.iter().all(Option::is_some), "{name}: {want:?}");
        let dir = entry(&job, &r.key);
        let got = ids(&dir.join(PROXY_FILE));
        assert_eq!(got, want, "{name}: proxy frame i shows original frame i");
        assert_eq!(r.frames, table.len() as u64, "{name}");
        // CFR at the original's nominal rate, one sample per presented frame.
        let proxy = demux(&dir.join(PROXY_FILE));
        let pv = proxy.video().unwrap();
        assert_eq!(pv.table.len(), table.len(), "{name}");
        let info = proxy.info().video.clone().unwrap();
        assert!(!info.vfr, "{name}: the proxy is CFR");
        assert_eq!(info.nominal_rate, Rational::from_int(30), "{name}");
        // `table.bin` is the original's sample table.
        let bin = std::fs::read(dir.join(TABLE_FILE)).unwrap();
        assert_eq!(table_bin::decode(&bin).unwrap(), table, "{name}");
        // `meta.json` lists the files and keeps the original's MediaInfo.
        let m = meta(&job, &r.key);
        m.check().unwrap();
        assert_eq!(m.media.as_ref(), Some(demux(&c.path).info()), "{name}");
        let names: Vec<_> = m.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, [PROXY_FILE, TABLE_FILE], "{name}");
        assert_eq!(r.bytes, m.bytes().unwrap(), "{name}");
    }
}

#[test]
fn tier_sizes_and_no_upscale() {
    if !gpu_or_skip("tier_sizes_and_no_upscale") {
        return;
    }
    let s = spec(4, Size::new(1280, 720));
    for (tier, want) in [
        (ProxyTier::Scrub, Size::new(960, 540)),
        // Never upscaled: 1280 × 720 stays its size for the 1080 tier.
        (ProxyTier::Preview, Size::new(1280, 720)),
    ] {
        let (_c, job, r) = made(&s, tier);
        assert_eq!(r.size, want, "{tier:?}");
        let proxy = demux(&entry(&job, &r.key).join(PROXY_FILE));
        let v = proxy.info().video.clone().unwrap();
        assert_eq!(v.coded, want, "{tier:?}");
        assert_eq!(meta(&job, &r.key).recipe, tier.recipe().name);
    }
}

/// Indices of the proxy's keyframes, after checking it has no reordering (B-frames).
fn keyframes(path: &Path) -> Vec<usize> {
    let d = demux(path);
    let t = &d.video().unwrap().table;
    for (i, f) in t.frames().iter().enumerate() {
        assert_eq!(
            f.decode_index as usize, i,
            "no B-frames: decode order is presentation order"
        );
    }
    (t.frames().iter().enumerate())
        .filter(|(_, f)| f.keyframe)
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn gop_rules_540_and_1080() {
    if !gpu_or_skip("gop_rules_540_and_1080") {
        return;
    }
    // 30 fps: 540 a keyframe every 7 frames (≤ 0.25 s, no intra-only measurement: Task 8);
    // 1080 every 15 (≤ 0.5 s, SVT-AV1's closed GOP of 15).
    let s = spec(40, Size::new(320, 180));
    for (tier, every) in [(ProxyTier::Scrub, 7), (ProxyTier::Preview, 15)] {
        let (_c, job, r) = made(&s, tier);
        let keys = keyframes(&entry(&job, &r.key).join(PROXY_FILE));
        let want: Vec<usize> = (0..40).step_by(every).collect();
        assert_eq!(keys, want, "{tier:?}");
    }
}

#[test]
fn hdr_source_gives_sdr_proxy() {
    if !gpu_or_skip("hdr_source_gives_sdr_proxy") {
        return;
    }
    let pq = ColourTags {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
        matrix: Matrix::Bt2020Ncl,
        range: Range::Limited,
        siting: ChromaSiting::Left,
    };
    let s = ClipSpec {
        colour: pq,
        ..spec(6, Size::new(320, 180))
    };
    let (c, job, r) = made(&s, ProxyTier::Scrub);
    assert_eq!(r.tone_mapped, Some(ToneMapped::Shader));
    let m = meta(&job, &r.key);
    assert_eq!(m.tone_mapped, Some(ToneMapped::Shader));
    // The original's HDR tags are kept in the entry; the proxy is BT.709 SDR.
    assert_eq!(
        m.media.unwrap().video.unwrap().colour.transfer,
        Transfer::Pq
    );
    let path = entry(&job, &r.key).join(PROXY_FILE);
    let v = demux(&path).info().video.clone().unwrap();
    // `colr` (nclx) carries no siting: compare the rest.
    let tags = |c: ColourTags| (c.primaries, c.transfer, c.matrix, c.range);
    assert_eq!(tags(v.colour), tags(ColourTags::BT709));
    assert!(v.hdr.is_none());
    assert_eq!(ids(&path), ids(&c.path));
    // An SDR source is not tone-mapped.
    let (_c, _job, sdr) = made(&spec(2, Size::new(320, 180)), ProxyTier::Scrub);
    assert_eq!(sdr.tone_mapped, None);
}

#[test]
fn rotation_not_baked() {
    if !gpu_or_skip("rotation_not_baked") {
        return;
    }
    let s = ClipSpec {
        rotation: Rotation::R90,
        ..spec(3, Size::new(320, 180))
    };
    let (_c, job, r) = made(&s, ProxyTier::Scrub);
    // The coded orientation and size ratio: 320 × 180, not 180 × 320.
    assert_eq!(r.size, Size::new(320, 180));
    let v = demux(&entry(&job, &r.key).join(PROXY_FILE))
        .info()
        .video
        .clone()
        .unwrap();
    assert_eq!((v.coded, v.rotation), (Size::new(320, 180), Rotation::R0));
    // The original's rotation is in the entry, for the compositor to apply.
    let m = meta(&job, &r.key).media.unwrap().video.unwrap();
    assert_eq!(m.rotation, Rotation::R90);
}

#[test]
fn cache_key_mismatch_is_plan_mismatch() {
    let c = clip(&spec(2, Size::new(320, 180)));
    let job = job_for("fp", &c.path);
    // The host expected an OS decoder; dav1d (software) decodes it here.
    let mut p = params("fp", ProxyTier::Scrub);
    p.decoder_class = VendorClass::Os;
    p.key = proxy_key("fp", ProxyTier::Scrub, ProxyCodec::Av1, VendorClass::Os).to_string();
    assert_eq!(outcome(&run(&job, &p), "fp").unwrap_err(), "PLAN_MISMATCH");
    // A key that is not the params' at all.
    let mut p = params("fp", ProxyTier::Scrub);
    p.key = proxy_key(
        "other",
        ProxyTier::Scrub,
        ProxyCodec::Av1,
        VendorClass::Software,
    )
    .to_string();
    assert_eq!(outcome(&run(&job, &p), "fp").unwrap_err(), "PLAN_MISMATCH");
    // Nothing was written for either.
    assert_eq!(std::fs::read_dir(job.out()).unwrap().count(), 0);
}

#[test]
fn h264_without_a_hardware_encoder_is_codec_unavailable() {
    // Linux has no H.264 encoder until 6B-2 (D13: Finisher ships none of its own).
    if cfg!(not(target_os = "linux")) {
        return;
    }
    let c = clip(&spec(2, Size::new(320, 180)));
    let job = job_for("fp", &c.path);
    let mut p = params("fp", ProxyTier::Scrub);
    p.codec = ProxyCodec::H264;
    p.key = proxy_key(
        "fp",
        ProxyTier::Scrub,
        ProxyCodec::H264,
        VendorClass::Software,
    )
    .to_string();
    assert_eq!(
        outcome(&run(&job, &p), "fp").unwrap_err(),
        "CODEC_UNAVAILABLE"
    );
}

#[test]
fn a_changed_asset_or_file_is_refused() {
    let c = clip(&spec(2, Size::new(320, 180)));
    // The manifest's fingerprint is not the one asked for.
    let job = job_for("fp-new", &c.path);
    assert_eq!(
        outcome(&run(&job, &params("fp", ProxyTier::Scrub)), "fp").unwrap_err(),
        "ASSET_CHANGED"
    );
    // The file changed after the manifest was written.
    let job = job_for("fp", &c.path);
    std::fs::write(&c.path, b"something else").unwrap();
    assert_eq!(
        outcome(&run(&job, &params("fp", ProxyTier::Scrub)), "fp").unwrap_err(),
        "ORIGINALS_MISSING"
    );
}

fn library_params(c: &Clip, size_off: u64) -> ProxyParams {
    let root = c.path.parent().unwrap();
    let mut expect = serde_json::to_value(expect_of(&c.path)).unwrap();
    expect["size"] = json!(expect["size"].as_u64().unwrap() + size_off);
    ProxyParams {
        asset: None,
        file: Some(ProxyLibraryFile {
            library: "l1".into(),
            root: root.to_str().unwrap().into(),
            rel: serde_json::to_value(rel(&["clip.mp4"])).unwrap(),
            expect,
            hydrate: false,
        }),
        ..params("fp", ProxyTier::Scrub)
    }
}

#[test]
fn library_files_open_through_the_catalogue_s_expectations() {
    let c = clip(&spec(3, Size::new(320, 180)));
    let job = Job::new();
    // A file that is not what the catalogue says is FILE_CHANGED (the stage rescans it).
    assert_eq!(
        outcome(&run(&job, &library_params(&c, 1)), "fp").unwrap_err(),
        "FILE_CHANGED"
    );
    if !gpu_or_skip("library_files_open_through_the_catalogue_s_expectations") {
        return;
    }
    let r = outcome(&run(&job, &library_params(&c, 0)), "fp").unwrap();
    assert_eq!(r.frames, 3);
    assert_eq!(ids(&entry(&job, &r.key).join(PROXY_FILE)), ids(&c.path));
}

#[test]
fn progress_counts_frames() {
    if !gpu_or_skip("progress_counts_frames") {
        return;
    }
    let c = clip(&spec(12, Size::new(320, 180)));
    let job = job_for("fp", &c.path);
    let run = run(&job, &params("fp", ProxyTier::Scrub));
    outcome(&run, "fp").unwrap();
    let p = run.progress();
    assert!(!p.is_empty());
    assert!(p.windows(2).all(|w| w[0] <= w[1]), "{p:?}");
    assert_eq!(p.last().copied(), Some(12.0));
}

#[test]
fn dolby_vision_profile_5_is_codec_unavailable() {
    use fin_media::{DolbyVision, HdrInfo};
    let dv = |profile| HdrInfo {
        mastering: None,
        content_light: None,
        dolby_vision: Some(DolbyVision {
            profile,
            level: 6,
            rpu: true,
            enhancement_layer: false,
            base_layer: true,
            compatibility: if profile == 5 { 0 } else { 1 },
        }),
    };
    let c = clip(&ClipSpec {
        hdr: Some(dv(5)),
        ..spec(2, Size::new(320, 180))
    });
    let job = job_for("fp", &c.path);
    assert_eq!(
        outcome(&run(&job, &params("fp", ProxyTier::Scrub)), "fp").unwrap_err(),
        "CODEC_UNAVAILABLE"
    );
    // Profile 8 proxies its base layer.
    if !gpu_or_skip("dolby_vision_profile_5_is_codec_unavailable") {
        return;
    }
    let c = clip(&ClipSpec {
        hdr: Some(dv(8)),
        ..spec(2, Size::new(320, 180))
    });
    let job = job_for("fp", &c.path);
    outcome(&run(&job, &params("fp", ProxyTier::Scrub)), "fp").unwrap();
}

/// Review of Task 14: an output the worker cannot write names it relative to the job directory;
/// no absolute path of the host reaches the job's error (each output file in turn).
#[test]
fn refused_outputs_name_no_absolute_path() {
    if !gpu_or_skip("refused_outputs_name_no_absolute_path") {
        return;
    }
    let c = clip(&spec(2, Size::new(320, 180)));
    for blocked in [PROXY_FILE, TABLE_FILE, META_FILE] {
        let job = job_for("fp", &c.path);
        let p = params("fp", ProxyTier::Scrub);
        // A directory (not empty) where the output file goes: it cannot be made.
        let dir = entry(&job, &p.key).join(blocked);
        std::fs::create_dir_all(dir.join("x")).unwrap();
        let run = run(&job, &p);
        let root = std::fs::canonicalize(job.dir.path()).unwrap();
        let text = format!("{:?} {:?}", run.end, run.events);
        assert!(
            matches!(run.end, End::Error(_)) || text.contains("ItemFailed"),
            "{blocked}: {text}"
        );
        for p in [root.as_path(), job.dir.path()] {
            assert!(!text.contains(&*p.to_string_lossy()), "{blocked}: {text}");
        }
    }
}
