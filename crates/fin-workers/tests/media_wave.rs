//! `media.waveform` v1 (plan 6C Task 19, Decisions 7, 19 and 20), driven over the envelope as a
//! host drives it (`fin-workers media`, cold): `peaks.bin` equal to an independent decode →
//! canonical → resample → `fin_audio::peaks`, the same bytes whether the worker reads the
//! source's `pcm48` entry or decodes the original, a valid `wave` entry, and the typed refusals.
//! Fixtures are synthetic WAV files written at test time.
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::path::{Path, PathBuf};

use fin_audio::channels::{Canonical, canonical};
use fin_audio::{Peaks, Resampler, Stereo};
use fin_codecs::audio::testing::wav::{WavFormat, wav};
use fin_codecs::audio::{AudioLimits, AudioReader};
use fin_jobs::HostMsg;
use fin_media::backend::VendorClass;
use fin_media::cache::{EntryMeta, META_FILE};
use fin_media::jobs::{
    MANIFEST_VERSION, ManifestAsset, ManifestFonts, OriginalFile, PEAKS_FILE, PcmLayout, PcmParams,
    PcmRecipe, RenderManifest, WaveFrom, WaveParams, WaveResult, pcm_key, wave_key,
};
use media_support::worker_env;
use serde_json::{Value, json};
use support::{End, Job, Proc, Run, expect_of, no_calls, spawn_with};

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
    };
    std::fs::write(
        job.path("in/manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    job
}

fn pcm_params(fp: &str, layout: PcmLayout) -> PcmParams {
    PcmParams {
        fingerprint: fp.into(),
        recipe: PcmRecipe::Pcm48,
        layout,
        decoder_class: VendorClass::Software,
        key: pcm_key(fp, PcmRecipe::Pcm48, layout, VendorClass::Software).to_string(),
        asset: Some("a1".into()),
        file: None,
        project_id: None,
        branch: None,
    }
}

fn wave_params(fp: &str) -> WaveParams {
    WaveParams {
        fingerprint: fp.into(),
        decoder_class: VendorClass::Software,
        key: wave_key(fp, VendorClass::Software).to_string(),
        asset: "a1".into(),
        pcm_key: None,
        project_id: None,
        branch: None,
    }
}

fn start(job: &Job, kind: &str, params: Value, grants: &[&str]) -> Run {
    let mut w: Proc = spawn_with(&["media"], &worker_env());
    w.hello();
    w.send(&HostMsg::Start(job.start(kind, params, grants)));
    w.drive(no_calls)
}

/// Runs `media.waveform`; `pcm` is the `pcm48` entry directory `in/pcm.json` names, if any.
fn run_wave(job: &Job, p: &WaveParams, pcm: Option<&Path>) -> Run {
    let mut grants = vec!["in/manifest.json", "out/"];
    if let (Some(dir), Some(key)) = (pcm, &p.pcm_key) {
        std::fs::write(
            job.path("in/pcm.json"),
            // The host writes the canonical directory: the checked open refuses any link.
            serde_json::to_vec(&json!({ "key": key, "dir": canonical_dir(dir) })).unwrap(),
        )
        .unwrap();
        grants.push("in/pcm.json");
    }
    start(
        job,
        "media.waveform",
        serde_json::to_value(p).unwrap(),
        &grants,
    )
}

fn canonical_dir(dir: &Path) -> String {
    // A missing entry stays as named (its parent is canonical).
    let name = dir.file_name().unwrap();
    let parent = std::fs::canonicalize(dir.parent().unwrap()).unwrap();
    parent.join(name).to_str().unwrap().to_owned()
}

fn outcome(run: &Run, fp: &str) -> Result<WaveResult, String> {
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

/// The peaks computed independently of the worker: the reader, the canonical form, one
/// resampler per channel to 48 kHz, a mono source as dual mono.
fn reference(path: &Path) -> Peaks {
    let mut r =
        AudioReader::open(std::fs::File::open(path).unwrap(), &AudioLimits::default()).unwrap();
    let rate = r.info().sample_rate;
    let mut rs = [
        Resampler::new(rate, 48_000).unwrap(),
        Resampler::new(rate, 48_000).unwrap(),
    ];
    let (mut l, mut rt) = (Vec::new(), Vec::new());
    while let Some(d) = r.next().unwrap() {
        match canonical(&d.interleaved, &d.layout).unwrap() {
            Canonical::Mono(m) => {
                rs[0].process(&m, &mut l);
                rs[1].process(&m, &mut rt);
            }
            Canonical::Stereo(s) => {
                rs[0].process(&s.l, &mut l);
                rs[1].process(&s.r, &mut rt);
            }
        }
    }
    rs[0].finish(&mut l);
    rs[1].finish(&mut rt);
    fin_audio::peaks(&Stereo { l, r: rt })
}

fn peaks_bytes(job: &Job, key: &str) -> Vec<u8> {
    std::fs::read(job.out().join(key).join(PEAKS_FILE)).unwrap()
}

#[test]
fn wave_levels_header() {
    // 44.1 kHz: the resampler runs; 700,000 frames → 761,905 at 48 kHz, so the peaks are built
    // from two blocks and must equal those of the whole signal.
    let s = source(700_000, 44_100, 2);
    let job = job_for("fp", Some(&s.path));
    let p = wave_params("fp");
    let r = outcome(&run_wave(&job, &p, None), "fp").unwrap();
    assert_eq!(r.key, p.key);
    assert_eq!((r.levels, r.from), (5, WaveFrom::Decode));
    let bytes = peaks_bytes(&job, &p.key);
    assert_eq!(r.bytes, bytes.len() as u64);
    assert_eq!(&bytes[..8], b"FINWAVE1");
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 5);
    assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 2);
    let frames = fin_audio::resampled_len(700_000, 44_100, 48_000);
    assert_eq!(
        u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
        frames
    );
    assert_eq!(r.frames, frames);
    let expected = reference(&s.path);
    assert_eq!(Peaks::parse(&bytes).unwrap(), expected);
    assert_eq!(bytes, expected.to_bytes());
    let levels: Vec<u32> = expected.levels.iter().map(|l| l.samples_per_peak).collect();
    assert_eq!(levels, [256, 1_024, 4_096, 16_384, 65_536]);
    assert_eq!(levels, fin_media::jobs::WAVE_LEVELS);
}

#[test]
fn wave_entry_meta_valid() {
    let s = source(48_000, 48_000, 2);
    let job = job_for("fp", Some(&s.path));
    let p = wave_params("fp");
    let r = outcome(&run_wave(&job, &p, None), "fp").unwrap();
    let m = meta(&job, &p.key);
    assert_eq!(m.check().unwrap().as_str(), p.key);
    assert_eq!((m.recipe.as_str(), m.recipe_version), ("wave", 1));
    assert_eq!(
        m.params,
        json!({ "audioVersion": fin_audio::AUDIO_VERSION, "rate": 48000 })
    );
    assert_eq!(m.fingerprint, "fp");
    assert_eq!(m.decoder_class, VendorClass::Software);
    assert_eq!(m.files.len(), 1);
    assert_eq!(m.files[0].name, PEAKS_FILE);
    assert_eq!(m.files[0].bytes, r.bytes);
    let b = peaks_bytes(&job, &p.key);
    assert_eq!(blake3::hash(&b).to_hex().as_str(), m.files[0].blake3);
    let mut names: Vec<String> = std::fs::read_dir(job.out().join(&p.key))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["meta.json", "peaks.bin"]);
}

#[test]
fn wave_from_pcm48_entry_matches_direct() {
    for (frames, rate, channels) in [(300_000usize, 44_100u32, 2u16), (200_000, 48_000, 1)] {
        let s = source(frames, rate, channels);
        // The source's pcm48 entry, made by media.pcm (a mono source stays one channel).
        let layout = if channels == 1 {
            PcmLayout::Mono
        } else {
            PcmLayout::Stereo
        };
        let pcm_job = job_for("fp", Some(&s.path));
        let pp = pcm_params("fp", layout);
        let run = start(
            &pcm_job,
            "media.pcm",
            serde_json::to_value(&pp).unwrap(),
            &["in/manifest.json", "out/"],
        );
        assert!(matches!(run.end, End::Result(_)), "{:?}", run.end);
        let entry = pcm_job.out().join(&pp.key);
        // Direct: decoded from the original.
        let direct = job_for("fp", Some(&s.path));
        let p = wave_params("fp");
        let d = outcome(&run_wave(&direct, &p, None), "fp").unwrap();
        assert_eq!(d.from, WaveFrom::Decode);
        // From the entry: the original is not even read (it is gone).
        let from_entry = job_for("fp", None);
        let q = WaveParams {
            pcm_key: Some(pp.key.clone()),
            ..p.clone()
        };
        let e = outcome(&run_wave(&from_entry, &q, Some(&entry)), "fp").unwrap();
        assert_eq!(e.from, WaveFrom::Pcm48);
        assert_eq!((e.frames, e.bytes), (d.frames, d.bytes));
        assert_eq!(
            peaks_bytes(&from_entry, &p.key),
            peaks_bytes(&direct, &p.key),
            "{channels} channels"
        );
        assert_eq!(peaks_bytes(&direct, &p.key), reference(&s.path).to_bytes());
    }
}

#[test]
fn wave_entry_not_usable_falls_back_to_the_decode() {
    let s = source(60_000, 48_000, 2);
    let layout = PcmLayout::Stereo;
    let pcm_job = job_for("fp", Some(&s.path));
    let pp = pcm_params("fp", layout);
    start(
        &pcm_job,
        "media.pcm",
        serde_json::to_value(&pp).unwrap(),
        &["in/manifest.json", "out/"],
    );
    let entry = pcm_job.out().join(&pp.key);
    let p = WaveParams {
        pcm_key: Some(pp.key.clone()),
        ..wave_params("fp")
    };
    // A chunk that is not its listed digest: the entry is refused, the original decoded.
    let chunk = entry.join("chunk-000000.f32");
    let mut b = std::fs::read(&chunk).unwrap();
    b[0] ^= 0xff;
    std::fs::write(&chunk, b).unwrap();
    let job = job_for("fp", Some(&s.path));
    let r = outcome(&run_wave(&job, &p, Some(&entry)), "fp").unwrap();
    assert_eq!(r.from, WaveFrom::Decode);
    assert_eq!(peaks_bytes(&job, &p.key), reference(&s.path).to_bytes());
    // An entry that is gone, with the original gone too: the original's absence is typed.
    let job = job_for("fp", None);
    let gone = pcm_job.out().join("0".repeat(64));
    assert_eq!(
        outcome(&run_wave(&job, &p, Some(&gone)), "fp").unwrap_err(),
        "ORIGINALS_MISSING"
    );
}

#[test]
fn wave_key_mismatch_writes_nothing() {
    let s = source(4_800, 48_000, 2);
    let job = job_for("fp", Some(&s.path));
    // The host planned another decoder class than the one that decodes a WAV.
    let p = WaveParams {
        decoder_class: VendorClass::Os,
        key: wave_key("fp", VendorClass::Os).to_string(),
        ..wave_params("fp")
    };
    assert_eq!(
        outcome(&run_wave(&job, &p, None), "fp").unwrap_err(),
        "PLAN_MISMATCH"
    );
    assert_eq!(std::fs::read_dir(job.out()).unwrap().count(), 0);
}

#[test]
fn wave_missing_original_and_changed_asset_typed() {
    let p = wave_params("fp");
    let job = job_for("fp", None);
    assert_eq!(
        outcome(&run_wave(&job, &p, None), "fp").unwrap_err(),
        "ORIGINALS_MISSING"
    );
    let s = source(4_800, 48_000, 2);
    let job = job_for("fp-new", Some(&s.path));
    assert_eq!(
        outcome(&run_wave(&job, &p, None), "fp").unwrap_err(),
        "ASSET_CHANGED"
    );
}
