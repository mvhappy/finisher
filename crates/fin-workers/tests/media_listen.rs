//! `media.listen` v1 (plan 6C Task 21, Decision 21), driven over the envelope as a host drives it
//! (`fin-workers media`, cold): a project with two audio tracks over synthetic WAV files, measured
//! per track and for the mix, from decoded originals or from a `pcm48` entry `media.pcm` made,
//! with the plan samples checked and missing sources noted.
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::path::{Path, PathBuf};

use fin_codecs::audio::testing::wav::{WavFormat, wav};
use fin_core::{
    AssetAdd, AudioInfo, InsertMode, ItemBody, ItemInsert, MemIds, Op, Project, Rational,
    SCHEMA_VERSION, TrackKind, Tx, apply_tx, new_project,
};
use fin_jobs::HostMsg;
use fin_media::backend::VendorClass;
use fin_media::jobs::{
    JobSnapshot, ListenNote, ListenParams, ListenResult, ListenSpan, MANIFEST_VERSION,
    ManifestAsset, ManifestFonts, ManifestPcm, OriginalFile, PcmLayout, PcmParams, PcmRecipe,
    RenderManifest, listen_plan_samples, pcm_key,
};
use media_support::worker_env;
use serde_json::Value;
use support::{End, Job, Run, expect_of, no_calls, spawn_with};

fn s(n: i64) -> Rational {
    Rational::from_int(n)
}

/// `secs` of a sine at `peak` (linear) and `hz`, at 48 kHz, `channels` the same.
fn tone(path: &Path, hz: f64, peak: f64, secs: usize, channels: u16) {
    let mut pcm = Vec::new();
    for i in 0..secs * 48_000 {
        let v = (peak * (2.0 * std::f64::consts::PI * hz * i as f64 / 48_000.0).sin()) as f32;
        for _ in 0..channels {
            pcm.push(v);
        }
    }
    std::fs::write(path, wav(&pcm, channels, 48_000, WavFormat::F32, None)).unwrap();
}

/// Two WAV files and a project over them: `a1` (stereo 1 kHz, −20 dBFS RMS) on `t2` over
/// `[0, 3)`, `a2` (mono 440 Hz) on a second audio track `t3` over `[1, 5)`.
struct Fixture {
    _dir: tempfile::TempDir,
    paths: [PathBuf; 2],
    project: Project,
    /// The assets' ids as the ops allocated them, and the second audio track's.
    ids: [String; 2],
    t3: String,
}

fn fixture() -> Fixture {
    fixture_with(false)
}

/// The fixture; `shared`: `a1` is played twice on `t2`, `$c1` over `[0, 3)` from source time 3
/// and `$c3` over `[3, 5)` from 0 (one asset, two items, a non-zero source offset first).
fn fixture_with(shared: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let paths = [root.join("one.wav"), root.join("two.wav")];
    tone(&paths[0], 1000.0, 0.1414, 6, 2);
    tone(&paths[1], 440.0, 0.25, 6, 1);
    let audio = |channels| AudioInfo {
        sample_rate: 48_000,
        channels,
    };
    let clip = |id: &str, track: &str, at: i64, asset: &str, secs: i64, source_in: i64| -> Op {
        ItemInsert {
            id: id.into(),
            track: track.into(),
            at: s(at),
            duration: s(secs),
            mode: InsertMode::Overwrite,
            body: ItemBody::Clip {
                asset: asset.into(),
                source_in: s(source_in),
                speed: s(1),
            },
        }
        .into()
    };
    let ops = vec![
        AssetAdd::media(
            "$a1",
            paths[0].to_str().unwrap(),
            s(6),
            None,
            Some(audio(2)),
            "fp-one",
        )
        .into(),
        AssetAdd::media(
            "$a2",
            paths[1].to_str().unwrap(),
            s(6),
            None,
            Some(audio(1)),
            "fp-two",
        )
        .into(),
        fin_core::TrackAdd {
            id: "$t3".into(),
            timeline: "tl1".into(),
            kind: TrackKind::Audio,
            index: None,
            name: Some("A2".into()),
        }
        .into(),
        clip("$c1", "t2", 0, "$a1", 3, if shared { 3 } else { 0 }),
        clip("$c2", "$t3", 1, "$a2", 4, 0),
    ];
    let ops = if shared {
        let mut ops = ops;
        ops.push(clip("$c3", "t2", 3, "$a1", 2, 0));
        ops
    } else {
        ops
    };
    let p = new_project("p1".into(), "Listen".into(), s(30)).unwrap();
    let tx = Tx {
        tx_id: "tx-fixture".into(),
        author: "test".into(),
        branch: "main".into(),
        base_rev: 0,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    };
    let project = apply_tx(&p, &tx, &mut MemIds::after(&p)).unwrap().project;
    let id_of = |path: &Path| {
        let a = project
            .assets
            .values()
            .find(|a| a.path == path.to_str().unwrap());
        a.unwrap().id.clone()
    };
    let ids = [id_of(&paths[0]), id_of(&paths[1])];
    let t3 = project.timelines[0]
        .tracks
        .iter()
        .find(|t| t.name == "A2")
        .unwrap()
        .id
        .clone();
    Fixture {
        _dir: dir,
        ids,
        t3,
        paths,
        project,
    }
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

impl Fixture {
    fn manifest(&self, missing: &[usize]) -> RenderManifest {
        // `missing` names assets by their place: `0` for the first, `1` for the second.
        let asset = |k: usize, fp: &str| ManifestAsset {
            asset: self.ids[k].clone(),
            kind: fin_core::AssetKind::Media,
            fingerprint: fp.into(),
            original: (!missing.contains(&k)).then(|| original(&self.paths[k])),
            proxies: Vec::new(),
        };
        RenderManifest {
            version: MANIFEST_VERSION,
            assets: vec![asset(0, "fp-one"), asset(1, "fp-two")],
            fonts: ManifestFonts::default(),
            pcm: Default::default(),
        }
    }

    fn params(&self, from: Rational, to: Rational) -> ListenParams {
        ListenParams {
            timeline: "tl1".into(),
            from,
            to,
            plan_samples: listen_plan_samples(&self.project, "tl1", from, to).unwrap(),
            project_id: None,
            branch: None,
            rev: None,
        }
    }

    /// A job directory holding the snapshot and `manifest`.
    fn job(&self, manifest: &RenderManifest) -> Job {
        let job = Job::new();
        let snap = JobSnapshot {
            branch: "main".into(),
            rev: 1,
            project: self.project.clone(),
        };
        std::fs::write(
            job.path("in/snapshot.json"),
            serde_json::to_vec(&snap).unwrap(),
        )
        .unwrap();
        std::fs::write(
            job.path("in/manifest.json"),
            serde_json::to_vec(manifest).unwrap(),
        )
        .unwrap();
        job
    }
}

fn listen(job: &Job, p: &ListenParams) -> Run {
    let mut w = spawn_with(&["media"], &worker_env());
    w.hello();
    let start = job.start(
        "media.listen",
        serde_json::to_value(p).unwrap(),
        &["in/snapshot.json", "in/manifest.json", "out/"],
    );
    w.send(&HostMsg::Start(start));
    w.drive(no_calls)
}

fn result(run: &Run) -> ListenResult {
    assert!(matches!(run.end, End::Result(_)), "{:?}", run.end);
    serde_json::from_value(run.result().clone()).unwrap()
}

fn span(from: Rational, to: Rational) -> ListenSpan {
    ListenSpan { from, to }
}

#[test]
fn listen_decodes_when_missing() {
    let fx = fixture();
    let job = fx.job(&fx.manifest(&[]));
    let run = listen(&job, &fx.params(s(0), s(6)));
    let r = result(&run);
    assert_eq!((r.timeline.as_str(), r.from, r.to), ("tl1", s(0), s(6)));
    assert_eq!(
        (r.stats.sources_from_cache, r.stats.sources_decoded),
        (0, 2)
    );
    let tracks: Vec<&str> = r.tracks.iter().map(|t| t.track.as_str()).collect();
    assert_eq!(tracks, ["t2", fx.t3.as_str()]);
    // t2 sounds over [0, 3), t3 over [1, 5): the mix is silent from 5 s.
    assert_eq!(r.mix.silences, [span(s(5), s(6))]);
    assert_eq!(r.tracks[0].stats.silences, [span(s(3), s(6))]);
    assert_eq!(
        r.tracks[1].stats.silences,
        [span(s(0), s(1)), span(s(5), s(6))]
    );
    // The 1 kHz stereo tone at −20 dBFS RMS a channel over half the range: about −16.99 LUFS
    // where it plays (the gate drops the silence), within 0.3 LU.
    let i = r.tracks[0].stats.integrated.unwrap();
    assert!((i - -16.99).abs() <= 0.3, "{i}");
    assert!(r.mix.integrated.unwrap() > i);
    assert!(r.notes.is_empty(), "{:?}", r.notes);
    assert_eq!(r.mix.clipped.samples, 0);
    // Progress was reported, ending at the range's frames.
    assert_eq!(run.progress().last().copied(), Some(288_000.0));
}

/// Runs `media.pcm` for the first asset and answers its entry as a manifest's `pcm` row.
fn make_pcm48(fx: &Fixture, keep: &Path) -> ManifestPcm {
    let job = Job::new();
    std::fs::write(
        job.path("in/manifest.json"),
        serde_json::to_vec(&fx.manifest(&[])).unwrap(),
    )
    .unwrap();
    let key = pcm_key(
        "fp-one",
        PcmRecipe::Pcm48,
        PcmLayout::Stereo,
        VendorClass::Software,
    );
    let p = PcmParams {
        fingerprint: "fp-one".into(),
        recipe: PcmRecipe::Pcm48,
        layout: PcmLayout::Stereo,
        decoder_class: VendorClass::Software,
        key: key.to_string(),
        asset: Some(fx.ids[0].clone()),
        file: None,
        project_id: None,
        branch: None,
    };
    let mut w = spawn_with(&["media"], &worker_env());
    w.hello();
    let start = job.start(
        "media.pcm",
        serde_json::to_value(&p).unwrap(),
        &["in/manifest.json", "out/"],
    );
    w.send(&HostMsg::Start(start));
    let run = w.drive(no_calls);
    assert!(matches!(run.end, End::Result(_)), "{:?}", run.end);
    let dir = keep.join(key.as_str());
    std::fs::rename(job.out().join(key.as_str()), &dir).unwrap();
    ManifestPcm {
        key: key.to_string(),
        dir: dir.to_str().unwrap().into(),
    }
}

#[test]
fn listen_uses_pcm48_when_present() {
    let fx = fixture();
    let keep = tempfile::tempdir().unwrap();
    let keep = std::fs::canonicalize(keep.path()).unwrap();
    let entry = make_pcm48(&fx, &keep);
    let decoded = result(&listen(&fx.job(&fx.manifest(&[])), &fx.params(s(0), s(6))));
    let mut m = fx.manifest(&[]);
    m.pcm.insert(fx.ids[0].clone(), entry.clone());
    let cached = result(&listen(&fx.job(&m), &fx.params(s(0), s(6))));
    assert_eq!(
        (
            cached.stats.sources_from_cache,
            cached.stats.sources_decoded
        ),
        (1, 1)
    );
    // The entry holds the same decode: every statistic is the same.
    assert_eq!(cached.mix, decoded.mix);
    assert_eq!(cached.tracks, decoded.tracks);
    // An entry whose chunk does not verify is not used: the original is decoded instead.
    let chunk = Path::new(&entry.dir).join("chunk-000000.f32");
    let mut bytes = std::fs::read(&chunk).unwrap();
    bytes[100] ^= 0x40;
    std::fs::write(&chunk, bytes).unwrap();
    let mut m = fx.manifest(&[]);
    m.pcm.insert(fx.ids[0].clone(), entry);
    let fell_back = result(&listen(&fx.job(&m), &fx.params(s(0), s(6))));
    assert_eq!(fell_back.mix, decoded.mix);
    assert_eq!(fell_back.stats.sources_decoded, 2);
}

#[test]
fn listen_plan_samples_checked() {
    let fx = fixture();
    let job = fx.job(&fx.manifest(&[]));
    // 25 s: windows at 0, 10 and 20.
    let mut p = fx.params(s(0), s(25));
    assert_eq!(p.plan_samples.len(), 3);
    p.plan_samples[1].blake3 = "0".repeat(64);
    let e = listen(&job, &p).error().clone();
    assert_eq!(e.code, "PLAN_MISMATCH");
    assert_eq!(e.details["reason"], "hash");
    assert_eq!(e.details["at"], "10");
    let mut p = fx.params(s(0), s(25));
    p.plan_samples.pop();
    let e = listen(&fx.job(&fx.manifest(&[])), &p).error().clone();
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("PLAN_MISMATCH", &"noSample".into())
    );
}

#[test]
fn a_missing_original_is_a_note_and_its_items_are_silent() {
    let fx = fixture();
    let job = fx.job(&fx.manifest(&[1]));
    let r = result(&listen(&job, &fx.params(s(0), s(6))));
    assert_eq!(
        r.notes,
        [ListenNote::MissingSource {
            asset: fx.ids[1].clone(),
            reason: "ORIGINALS_MISSING".into()
        }]
    );
    assert_eq!(r.stats.sources_decoded, 1);
    assert_eq!(r.tracks[1].stats.integrated, None);
    assert_eq!(r.tracks[1].stats.silences, [span(s(0), s(6))]);
    assert_eq!(r.mix.silences, [span(s(3), s(6))]);
}

#[test]
fn bad_params_are_refused() {
    let fx = fixture();
    let job = fx.job(&fx.manifest(&[]));
    let mut p = fx.params(s(0), s(6));
    p.to = s(0);
    assert_eq!(listen(&job, &p).error().code, "JOB_PARAMS_INVALID");
}

#[test]
fn a_shared_asset_at_a_source_offset_mixes_from_decode_and_from_pcm48() {
    // 6C Task 21 review B1: one asset under two items, the first from source time 3.
    let fx = fixture_with(true);
    let decoded = result(&listen(&fx.job(&fx.manifest(&[])), &fx.params(s(0), s(6))));
    assert_eq!(decoded.tracks[0].track, "t2");
    // t2 plays [0, 5) without a gap: one silence, from 5.
    assert_eq!(decoded.tracks[0].stats.silences, [span(s(5), s(6))]);
    let i = decoded.tracks[0].stats.integrated.unwrap();
    assert!((i - -16.99).abs() <= 0.3, "{i}");
    let keep = tempfile::tempdir().unwrap();
    let keep = std::fs::canonicalize(keep.path()).unwrap();
    let mut m = fx.manifest(&[]);
    m.pcm.insert(fx.ids[0].clone(), make_pcm48(&fx, &keep));
    let cached = result(&listen(&fx.job(&m), &fx.params(s(0), s(6))));
    assert_eq!(cached.stats.sources_from_cache, 1);
    assert_eq!(cached.tracks, decoded.tracks);
    assert_eq!(cached.mix, decoded.mix);
}

#[test]
fn a_nested_timeline_counts_for_the_track_of_its_compound_clip() {
    let mut fx = fixture();
    // `t3`'s clip moves into a nested timeline `tl2`, which `t3` plays over [1, 5).
    let mut inner = fx.project.timelines[0].clone();
    inner.id = "tl2".into();
    let t3 = fx.project.timelines[0]
        .tracks
        .iter()
        .position(|t| t.id == fx.t3)
        .unwrap();
    let clip = fx.project.timelines[0].tracks[t3].items.remove(0);
    let mut moved = clip.clone();
    moved.start = s(0);
    for t in &mut inner.tracks {
        t.items.clear();
        t.transitions.clear();
    }
    inner.tracks[t3].items.push(moved);
    let nested = fin_core::Item::new(
        "n1",
        s(1),
        s(4),
        fin_core::ItemBody::Nested {
            timeline: "tl2".into(),
            source_in: s(0),
        },
    );
    fx.project.timelines[0].tracks[t3].items.push(nested);
    fx.project.timelines.push(inner);
    let r = result(&listen(&fx.job(&fx.manifest(&[])), &fx.params(s(0), s(6))));
    let tracks: Vec<&str> = r.tracks.iter().map(|t| t.track.as_str()).collect();
    assert_eq!(tracks, ["t2", fx.t3.as_str()]);
    assert_eq!(
        r.tracks[1].stats.silences,
        [span(s(0), s(1)), span(s(5), s(6))]
    );
}

#[test]
fn the_result_names_the_rev_measured() {
    let fx = fixture();
    let r = result(&listen(&fx.job(&fx.manifest(&[])), &fx.params(s(0), s(6))));
    assert_eq!(r.rev, 1);
}
