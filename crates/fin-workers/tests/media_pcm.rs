//! `media.pcm` v1 (plan 6C Task 18, Decisions 7, 8, 20, 23), driven over the envelope as a host
//! drives it (`fin-workers media`, cold): the chunks and `meta.json` of `pcm48` and `pcm16m`
//! entries, the feed equal to an independent decode → canonical → resample, mono kept as one
//! channel, the key the worker recomputes, a worker death mid-entry retried byte for byte, and
//! the typed refusals. Fixtures are synthetic WAV files written at test time.
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::path::{Path, PathBuf};

use fin_audio::channels::{Canonical, canonical};
use fin_audio::{Resampler, resampled_len};
use fin_codecs::audio::testing::wav::{WavFormat, wav};
use fin_codecs::audio::{AudioLimits, AudioReader};
use fin_jobs::HostMsg;
use fin_media::backend::VendorClass;
use fin_media::cache::{EntryMeta, META_FILE};
use fin_media::jobs::{
    MANIFEST_VERSION, ManifestAsset, ManifestFonts, OriginalFile, PcmLayout, PcmParams, PcmRecipe,
    PcmResult, ProxyLibraryFile, RenderManifest, chunk_name, pcm_key,
};
use media_support::worker_env;
use serde_json::{Value, json};
use support::{End, Job, Proc, Run, expect_of, no_calls, rel, spawn_with};

/// A WAV written into a fresh (canonical) directory.
struct Source {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

/// `frames` frames at `rate` of a 440 Hz tone (left) and a 1 kHz tone (right), 16-bit.
fn source(frames: usize, rate: u32, channels: u16) -> Source {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap().join("sound.wav");
    let mut pcm = Vec::with_capacity(frames * channels as usize);
    for i in 0..frames {
        let t = i as f64 / f64::from(rate);
        pcm.push((0.5 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as f32);
        if channels == 2 {
            pcm.push((0.25 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as f32);
        }
    }
    std::fs::write(&path, wav(&pcm, channels, rate, WavFormat::Pcm16, None)).unwrap();
    Source { _dir: dir, path }
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

/// A job whose manifest names asset `a1` (fingerprint `fp`) at `path` (`None`: missing here).
fn job_for(fp: &str, path: Option<&Path>) -> Job {
    let job = Job::new();
    let m = RenderManifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestAsset {
            asset: "a1".into(),
            kind: fin_core::AssetKind::Media,
            fingerprint: fp.into(),
            original: path.map(original),
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

fn params(fp: &str, recipe: PcmRecipe, layout: PcmLayout) -> PcmParams {
    PcmParams {
        fingerprint: fp.into(),
        recipe,
        layout,
        decoder_class: VendorClass::Software,
        key: pcm_key(fp, recipe, layout, VendorClass::Software).to_string(),
        asset: Some("a1".into()),
        file: None,
        project_id: None,
        branch: None,
    }
}

fn run_env(job: &Job, p: &PcmParams, env: &[(&str, String)]) -> Run {
    let mut all = worker_env();
    all.extend(env.iter().map(|(k, v)| (*k, v.clone())));
    let mut w: Proc = spawn_with(&["media"], &all);
    w.hello();
    let grants: &[&str] = if p.asset.is_some() {
        &["in/manifest.json", "out/"]
    } else {
        &["out/"]
    };
    let start = job.start("media.pcm", serde_json::to_value(p).unwrap(), grants);
    w.send(&HostMsg::Start(start));
    w.drive(no_calls)
}

fn run(job: &Job, p: &PcmParams) -> Run {
    run_env(job, p, &[])
}

/// The item's result, or its error code.
fn outcome(run: &Run, fp: &str) -> Result<PcmResult, String> {
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

fn meta(job: &Job, key: &str) -> EntryMeta {
    serde_json::from_slice(&std::fs::read(job.out().join(key).join(META_FILE)).unwrap()).unwrap()
}

/// Every chunk of the entry, concatenated, as `f32`.
fn samples(job: &Job, key: &str, m: &EntryMeta) -> Vec<f32> {
    let mut out = Vec::new();
    for f in &m.files {
        let b = std::fs::read(job.out().join(key).join(&f.name)).unwrap();
        out.extend(b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)));
    }
    out
}

/// The feed computed independently of the worker: the reader, the canonical form and one
/// resampler per channel (folded to mono for `pcm16m`), interleaved.
fn reference(path: &Path, recipe: PcmRecipe) -> Vec<f32> {
    let mut r =
        AudioReader::open(std::fs::File::open(path).unwrap(), &AudioLimits::default()).unwrap();
    let rate = r.info().sample_rate;
    let mono = recipe == PcmRecipe::Pcm16m || r.info().channels == 1;
    let n = if mono { 1 } else { 2 };
    let mut rs: Vec<Resampler> = (0..n)
        .map(|_| Resampler::new(rate, recipe.rate()).unwrap())
        .collect();
    let mut ch: Vec<Vec<f32>> = vec![Vec::new(); n];
    let mut fold = Vec::new();
    while let Some(d) = r.next().unwrap() {
        let c = canonical(&d.interleaved, &d.layout).unwrap();
        match (&c, mono) {
            (Canonical::Mono(m), true) => rs[0].process(m, &mut ch[0]),
            (Canonical::Stereo(_), true) => {
                fin_audio::channels::fold_mono(&c, &mut fold);
                rs[0].process(&fold, &mut ch[0]);
            }
            (Canonical::Stereo(s), false) => {
                rs[0].process(&s.l, &mut ch[0]);
                rs[1].process(&s.r, &mut ch[1]);
            }
            (Canonical::Mono(_), false) => unreachable!(),
        }
    }
    for (r, c) in rs.iter_mut().zip(ch.iter_mut()) {
        r.finish(c);
    }
    if n == 1 {
        return ch.remove(0);
    }
    ch[0]
        .iter()
        .zip(&ch[1])
        .flat_map(|(l, r)| [*l, *r])
        .collect()
}

#[test]
fn pcm48_chunks_and_meta() {
    // 10 s at 48 kHz stereo: 3 × 131,072 frames + 86,784.
    let s = source(480_000, 48_000, 2);
    let job = job_for("fp", Some(&s.path));
    let p = params("fp", PcmRecipe::Pcm48, PcmLayout::Stereo);
    let r = outcome(&run(&job, &p), "fp").unwrap();
    assert_eq!(r.key, p.key);
    assert_eq!(
        (r.frames, r.chunks, r.layout),
        (480_000, 4, PcmLayout::Stereo)
    );
    assert_eq!((r.source_rate, r.source_channels), (48_000, 2));
    assert_eq!(
        (r.decoder.backend.as_str(), r.decoder.class),
        ("symphonia", VendorClass::Software)
    );
    let m = meta(&job, &p.key);
    assert_eq!(m.check().unwrap().as_str(), p.key);
    assert_eq!((m.recipe.as_str(), m.recipe_version), ("pcm48", 1));
    assert_eq!(
        m.params,
        json!({ "audioVersion": fin_audio::AUDIO_VERSION, "rate": 48000, "layout": "stereo" })
    );
    let sizes: Vec<u64> = m.files.iter().map(|f| f.bytes).collect();
    assert_eq!(sizes, [1 << 20, 1 << 20, 1 << 20, 86_784 * 8]);
    assert_eq!(r.bytes, sizes.iter().sum::<u64>());
    for (i, f) in m.files.iter().enumerate() {
        assert_eq!(f.name, chunk_name(i));
        let b = std::fs::read(job.out().join(&p.key).join(&f.name)).unwrap();
        assert_eq!(blake3::hash(&b).to_hex().as_str(), f.blake3);
    }
    // The entry directory holds exactly the listed files and meta.json.
    let mut names: Vec<String> = std::fs::read_dir(job.out().join(&p.key))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names.len(), 5);
    assert!(fin_media::jobs::PcmEntry::of(&m).is_ok());
}

#[test]
fn pcm48_equals_decoded_source() {
    // 44.1 kHz: the resampler runs.
    let s = source(100_000, 44_100, 2);
    let job = job_for("fp", Some(&s.path));
    let p = params("fp", PcmRecipe::Pcm48, PcmLayout::Stereo);
    let r = outcome(&run(&job, &p), "fp").unwrap();
    assert_eq!(r.frames, resampled_len(100_000, 44_100, 48_000));
    let got = samples(&job, &p.key, &meta(&job, &p.key));
    assert_eq!(got, reference(&s.path, PcmRecipe::Pcm48));
}

#[test]
fn pcm16m_is_mono_16k() {
    // 20 s at 44.1 kHz stereo → 320,000 frames at 16 kHz: 262,144 + 57,856.
    let s = source(882_000, 44_100, 2);
    let job = job_for("fp", Some(&s.path));
    let p = params("fp", PcmRecipe::Pcm16m, PcmLayout::Mono);
    let r = outcome(&run(&job, &p), "fp").unwrap();
    assert_eq!(r.frames, resampled_len(882_000, 44_100, 16_000));
    assert_eq!(
        (r.frames, r.chunks, r.layout),
        (320_000, 2, PcmLayout::Mono)
    );
    let m = meta(&job, &p.key);
    assert_eq!(m.params["rate"], 16_000);
    assert_eq!(m.files[0].bytes, 262_144 * 4);
    assert_eq!(
        samples(&job, &p.key, &m),
        reference(&s.path, PcmRecipe::Pcm16m)
    );
}

#[test]
fn mono_source_stays_one_channel() {
    let s = source(480_000, 48_000, 1);
    let job = job_for("fp", Some(&s.path));
    let p = params("fp", PcmRecipe::Pcm48, PcmLayout::Mono);
    let r = outcome(&run(&job, &p), "fp").unwrap();
    assert_eq!(
        (r.layout, r.frames, r.chunks),
        (PcmLayout::Mono, 480_000, 2)
    );
    let m = meta(&job, &p.key);
    assert_eq!(m.params["layout"], "mono");
    assert_eq!(m.files[0].bytes, 262_144 * 4);
    assert_eq!(m.files[1].bytes, (480_000 - 262_144) * 4);
    // Identity rate, one channel: the decoded samples, never upmixed.
    assert_eq!(
        samples(&job, &p.key, &m),
        reference(&s.path, PcmRecipe::Pcm48)
    );
}

#[test]
fn key_mismatch_writes_nothing() {
    let s = source(4_800, 48_000, 2);
    // The host planned another decoder class, or a mono layout for a stereo source.
    for p in [
        PcmParams {
            decoder_class: VendorClass::Os,
            key: pcm_key("fp", PcmRecipe::Pcm48, PcmLayout::Stereo, VendorClass::Os).to_string(),
            ..params("fp", PcmRecipe::Pcm48, PcmLayout::Stereo)
        },
        params("fp", PcmRecipe::Pcm48, PcmLayout::Mono),
    ] {
        let job = job_for("fp", Some(&s.path));
        assert_eq!(outcome(&run(&job, &p), "fp").unwrap_err(), "PLAN_MISMATCH");
        assert_eq!(std::fs::read_dir(job.out()).unwrap().count(), 0);
    }
}

#[test]
fn worker_death_retry_is_byte_identical() {
    let s = source(480_000, 48_000, 2);
    let p = params("fp", PcmRecipe::Pcm48, PcmLayout::Stereo);
    // An uninterrupted run.
    let clean = job_for("fp", Some(&s.path));
    outcome(&run(&clean, &p), "fp").unwrap();
    // The worker dies once chunk 2 is written: two complete chunks, no meta.json, no item.
    let job = job_for("fp", Some(&s.path));
    let died = run_env(&job, &p, &[("FIN_TEST_PCM_ABORT_AFTER", "2".into())]);
    assert!(matches!(died.end, End::Died(_)), "{:?}", died.end);
    assert!(died.items().is_empty());
    let dir = job.out().join(&p.key);
    assert!(dir.join(chunk_name(1)).exists());
    assert!(!dir.join(chunk_name(2)).exists() && !dir.join(META_FILE).exists());
    // 5C empties `out/` before the next attempt, which starts from sample 0.
    std::fs::remove_dir_all(job.out()).unwrap();
    std::fs::create_dir(job.out()).unwrap();
    outcome(&run(&job, &p), "fp").unwrap();
    let (a, b) = (meta(&clean, &p.key), meta(&job, &p.key));
    assert_eq!(a.files, b.files);
    for f in &a.files {
        assert_eq!(
            std::fs::read(clean.out().join(&p.key).join(&f.name)).unwrap(),
            std::fs::read(dir.join(&f.name)).unwrap()
        );
    }
    // meta.json is byte-identical but for its creation time.
    let text = |j: &Job| {
        let mut v: Value =
            serde_json::from_slice(&std::fs::read(j.out().join(&p.key).join(META_FILE)).unwrap())
                .unwrap();
        v["createdAt"] = json!(0);
        serde_json::to_vec_pretty(&v).unwrap()
    };
    assert_eq!(text(&clean), text(&job));
}

#[test]
fn missing_original_typed() {
    // Not on this machine, or gone since the manifest was written.
    let job = job_for("fp", None);
    let p = params("fp", PcmRecipe::Pcm48, PcmLayout::Stereo);
    assert_eq!(
        outcome(&run(&job, &p), "fp").unwrap_err(),
        "ORIGINALS_MISSING"
    );
    let s = source(4_800, 48_000, 2);
    let job = job_for("fp", Some(&s.path));
    std::fs::remove_file(&s.path).unwrap();
    assert_eq!(
        outcome(&run(&job, &p), "fp").unwrap_err(),
        "ORIGINALS_MISSING"
    );
    // Another fingerprint than the manifest's is the asset having changed.
    let s = source(4_800, 48_000, 2);
    let job = job_for("fp-new", Some(&s.path));
    assert_eq!(outcome(&run(&job, &p), "fp").unwrap_err(), "ASSET_CHANGED");
    // A file that is no audio container is typed too.
    std::fs::write(&s.path, b"not audio at all, just bytes").unwrap();
    let job = job_for("fp", Some(&s.path));
    assert_eq!(
        outcome(&run(&job, &p), "fp").unwrap_err(),
        "UNSUPPORTED_CONTAINER"
    );
}

#[test]
fn library_file_through_the_catalogue() {
    let s = source(48_000, 48_000, 2);
    let root = s.path.parent().unwrap();
    let p = PcmParams {
        asset: None,
        file: Some(ProxyLibraryFile {
            library: "l1".into(),
            root: root.to_str().unwrap().into(),
            rel: serde_json::to_value(rel(&["sound.wav"])).unwrap(),
            expect: serde_json::to_value(expect_of(&s.path)).unwrap(),
            hydrate: false,
        }),
        ..params("fp", PcmRecipe::Pcm16m, PcmLayout::Mono)
    };
    let job = Job::new();
    let r = outcome(&run(&job, &p), "fp").unwrap();
    assert_eq!(r.frames, 16_000);
    // A file that is not what the catalogue says is FILE_CHANGED (the stage rescans it).
    let mut q = p.clone();
    q.file.as_mut().unwrap().expect["size"] = json!(1);
    assert_eq!(
        outcome(&run(&Job::new(), &q), "fp").unwrap_err(),
        "FILE_CHANGED"
    );
}
