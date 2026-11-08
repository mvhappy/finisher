//! `library.probe` and `library.hash` over the real `fin-workers` binary (plan 5D Task 6).
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

mod support;

use std::path::Path;

use fin_jobs::{JobError, Retry};
use fin_library::params::{
    FileLine, LibraryHashParams, LibraryProbeParams, PROBED_MAX, ProbedCall,
};
use fin_library::{FileRef, Throttle, hash_file};
use fin_probe::PROBE_VERSION;
use fin_probe::fixtures::write_png;
use serde_json::{Value, json};
use support::{End, Ev, Job, expect_of, identity_of, overlay, rel, run_job, write_file};

/// A root with `n` PNGs `p0.png`…, and the lines the catalog would hold for them (ids 1…).
struct Tree {
    root: tempfile::TempDir,
    lines: Vec<FileLine>,
}

impl Tree {
    fn new(n: usize) -> Tree {
        let root = tempfile::tempdir().unwrap();
        let mut lines = Vec::new();
        for i in 0..n {
            let name = format!("p{i}.png");
            write_png(&root.path().join(&name), 8 + i as u32, 4);
            lines.push(FileLine {
                id: i as i64 + 1,
                rel: rel(&[&name]),
                expect: expect_of(&root.path().join(&name)),
            });
        }
        Tree { root, lines }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn job(&self) -> Job {
        let job = Job::new();
        let lines: Vec<Value> = self
            .lines
            .iter()
            .map(|l| serde_json::to_value(l).unwrap())
            .collect();
        job.write_in("in/files.ndjson", &lines);
        job
    }

    fn items(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.id.to_string()).collect()
    }
}

fn probe_params(t: &Tree, items: Vec<String>) -> LibraryProbeParams {
    LibraryProbeParams {
        library: "lib1".into(),
        root: t.path().to_str().unwrap().into(),
        hydrate: false,
        read_bps: None,
        volume: identity_of(t.path()),
        extensions: None,
        files: "in/files.ndjson".into(),
        items,
    }
}

fn probe_start(job: &Job, p: &LibraryProbeParams) -> fin_jobs::Start {
    job.start(
        "library.probe",
        serde_json::to_value(p).unwrap(),
        &["in/files.ndjson"],
    )
}

fn probed(call: &Value) -> ProbedCall {
    serde_json::from_value(call.clone()).unwrap()
}

#[test]
fn probe_acks_items_after_probed_reply() {
    let t = Tree::new(3);
    let job = t.job();
    let mut p = run_job(probe_start(&job, &probe_params(&t, t.items())));
    let run = p.drive(|method, _| {
        assert_eq!(method, "library.probed");
        Ok(json!({}))
    });
    assert!(run.early.is_empty(), "{:?}", run.early);
    let begin = |i: &str| Ev::Begin(i.into());
    let kinds: Vec<String> = run
        .events
        .iter()
        .map(|e| match e {
            Ev::Begin(i) => format!("begin {i}"),
            Ev::Call(m, _) => format!("call {m}"),
            Ev::Item(i, v) => {
                assert!(v.is_none(), "acks carry no values");
                format!("item {i}")
            }
            other => panic!("{other:?}"),
        })
        .collect();
    let _ = begin;
    assert_eq!(
        kinds,
        [
            "begin 1",
            "begin 2",
            "begin 3",
            "call library.probed",
            "item 1",
            "item 2",
            "item 3"
        ]
    );
    let results = probed(run.calls("library.probed")[0]).results;
    assert_eq!(results.len(), 3);
    for (i, r) in results.iter().enumerate() {
        assert_eq!((r.file, r.state.as_str()), (i as i64 + 1, "present"));
        assert!(r.fingerprint.is_some() && r.probe.is_some(), "{r:?}");
        assert_eq!(r.probe_version, PROBE_VERSION);
    }
    assert_eq!(run.result()["states"], json!({ "present": 3 }));
}

#[test]
fn probed_results_go_in_groups_of_32() {
    let t = Tree::new(PROBED_MAX + 8);
    let job = t.job();
    let mut p = run_job(probe_start(&job, &probe_params(&t, t.items())));
    let run = p.drive(|_, _| Ok(json!({})));
    let sizes: Vec<usize> = run
        .calls("library.probed")
        .iter()
        .map(|c| probed(c).results.len())
        .collect();
    assert_eq!(sizes, [32, 8]);
    assert_eq!(run.items().len(), 40);
    // The 32 are acknowledged before the next file begins.
    let first_ack = run
        .events
        .iter()
        .position(|e| matches!(e, Ev::Item(..)))
        .unwrap();
    let begins_before = run.events[..first_ack]
        .iter()
        .filter(|e| matches!(e, Ev::Begin(_)))
        .count();
    assert_eq!(begins_before, 32);
}

#[test]
fn probe_crash_on_one_file_is_isolatable() {
    let t = Tree::new(3);
    overlay(
        t.path(),
        json!({ "paths": { "p1.png": { "crashOnOpen": true } } }),
    );
    let job = t.job();
    let mut p = run_job(probe_start(&job, &probe_params(&t, t.items())));
    let run = p.drive(|_, _| Ok(json!({})));
    match &run.end {
        End::Died(status) => assert!(!status.success()),
        e => panic!("the worker dies in the poison file, got {e:?}"),
    }
    assert!(
        run.events
            .iter()
            .any(|e| matches!(e, Ev::Begin(i) if i == "2"))
    );
    assert!(
        !run.items().contains(&"2"),
        "the poison file is never acknowledged"
    );
    // The engine's next runs: the in-flight file alone (dies again: POISON_ITEM), then the
    // rest, which need no state from the first run.
    let mut p = run_job(probe_start(&job, &probe_params(&t, vec!["2".into()])));
    let alone = p.drive(|_, _| Ok(json!({})));
    assert!(matches!(alone.end, End::Died(_)));
    let mut p = run_job(probe_start(
        &job,
        &probe_params(&t, vec!["1".into(), "3".into()]),
    ));
    let rest = p.drive(|_, _| Ok(json!({})));
    assert_eq!(rest.items(), ["1", "3"]);
    assert_eq!(rest.result()["states"], json!({ "present": 2 }));
}

#[test]
fn a_changed_file_is_reported_changed_and_never_read() {
    let t = Tree::new(2);
    // p0 grows after the catalog saw it.
    write_file(t.path(), "p0.png", &[0u8; 5000]);
    let job = t.job();
    let mut p = run_job(probe_start(&job, &probe_params(&t, t.items())));
    let run = p.drive(|_, _| Ok(json!({})));
    let results = probed(run.calls("library.probed")[0]).results;
    assert_eq!(results[0].state, "changed");
    assert_eq!(results[0].code.as_deref(), Some("FILE_CHANGED"));
    assert!(results[0].fingerprint.is_none() && results[0].probe.is_none());
    assert_eq!(results[1].state, "present");
    assert_eq!(run.result()["changed"], 1);
}

#[test]
fn the_kinds_table_filters_by_extension() {
    let t = Tree::new(1);
    write_file(t.path(), "notes.TXT", b"hello");
    let mut lines = t.lines.clone();
    lines.push(FileLine {
        id: 9,
        rel: rel(&["notes.TXT"]),
        expect: expect_of(&t.path().join("notes.TXT")),
    });
    let job = Job::new();
    let values: Vec<Value> = lines
        .iter()
        .map(|l| serde_json::to_value(l).unwrap())
        .collect();
    job.write_in("in/files.ndjson", &values);
    let mut prm = probe_params(&t, vec!["1".into(), "9".into()]);
    prm.extensions = Some(vec!["png".into(), "mov".into()]);
    let mut p = run_job(probe_start(&job, &prm));
    let run = p.drive(|_, _| Ok(json!({})));
    let results = probed(run.calls("library.probed")[0]).results;
    assert_eq!(results[0].state, "present");
    assert_eq!(
        (results[1].state.as_str(), results[1].code.as_deref()),
        ("skipped", Some("KIND_FILTER"))
    );
    // Uppercase extensions match case-insensitively.
    prm.extensions = Some(vec!["txt".into()]);
    let mut p = run_job(probe_start(&job, &prm));
    let run = p.drive(|_, _| Ok(json!({})));
    let results = probed(run.calls("library.probed")[0]).results;
    // Read now (its content is no media, so the probe fails, but the fingerprint is kept).
    assert_eq!(results[1].state, "probeFailed");
    assert!(results[1].fingerprint.is_some());
    assert_eq!(results[0].state, "skipped");
}

#[test]
fn cloud_only_files_are_not_opened_without_hydrate() {
    let t = Tree::new(2);
    let log = tempfile::tempdir().unwrap().keep().join("opens.ndjson");
    overlay(
        t.path(),
        json!({ "openLog": log, "paths": { "p0.png": { "placeholder": "dataless" } } }),
    );
    let job = t.job();
    let mut p = run_job(probe_start(&job, &probe_params(&t, t.items())));
    let run = p.drive(|_, _| Ok(json!({})));
    let results = probed(run.calls("library.probed")[0]).results;
    assert_eq!(results[0].state, "cloudOnly");
    assert_eq!(results[0].code.as_deref(), Some("FILE_CLOUD_ONLY"));
    assert_eq!(results[1].state, "present");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("\"refused\":true"), "{text}");
}

#[test]
fn offline_root_is_library_offline_never_retried_for_probe_too() {
    let t = Tree::new(2);
    let job = t.job();
    let prm = probe_params(&t, t.items());
    let root = t.root.path().to_path_buf();
    std::fs::rename(&root, root.with_extension("gone")).unwrap();
    let mut p = run_job(probe_start(&job, &prm));
    let run = p.drive(|_, _| panic!("nothing is reported for an offline root"));
    let e = run.error();
    assert_eq!(
        (e.code.as_str(), e.retry),
        ("LIBRARY_OFFLINE", Retry::Never)
    );
    assert_eq!(e.details["state"]["state"], "unreachable");
    std::fs::rename(root.with_extension("gone"), &root).unwrap();
}

#[test]
fn a_deleted_file_is_missing_while_the_root_is_online() {
    let t = Tree::new(2);
    std::fs::remove_file(t.path().join("p0.png")).unwrap();
    let job = t.job();
    let mut p = run_job(probe_start(&job, &probe_params(&t, t.items())));
    let run = p.drive(|_, _| Ok(json!({})));
    let results = probed(run.calls("library.probed")[0]).results;
    assert_eq!(results[0].state, "missing");
    assert_eq!(results[0].code.as_deref(), Some("FILE_NOT_FOUND"));
    assert_eq!(results[1].state, "present");
}

#[test]
fn cancel_flushes_finished_files_and_drops_the_rest() {
    let t = Tree::new(PROBED_MAX + 8);
    let job = t.job();
    let mut p = run_job(probe_start(&job, &probe_params(&t, t.items())));
    let run = p.drive_cancelling(Some(1), |_, _| Ok(json!({})));
    match &run.end {
        End::Error(e) => assert_eq!(e.code, "JOB_CANCELLED"),
        e => panic!("{e:?}"),
    }
    assert_eq!(run.calls("library.probed").len(), 1);
    assert_eq!(run.items().len(), 32);
}

#[test]
fn an_item_without_a_line_fails_alone() {
    let t = Tree::new(1);
    let job = t.job();
    let mut p = run_job(probe_start(
        &job,
        &probe_params(&t, vec!["1".into(), "77".into(), "x".into()]),
    ));
    let run = p.drive(|_, _| Ok(json!({})));
    let failed: Vec<(&str, &str)> = run
        .events
        .iter()
        .filter_map(|e| match e {
            Ev::ItemFailed(i, c) => Some((i.as_str(), c.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        failed,
        [("x", "JOB_ITEM_INVALID"), ("77", "FILE_NOT_LISTED")]
    );
    assert_eq!(run.items(), ["1"]);
}

#[test]
fn a_host_refusal_of_probed_ends_the_job_without_acks() {
    let t = Tree::new(2);
    let job = t.job();
    let mut p = run_job(probe_start(&job, &probe_params(&t, t.items())));
    let run = p.drive(|_, _| Err(JobError::new("CATALOG_BUSY", "later")));
    assert_eq!(run.error().code, "CATALOG_BUSY");
    assert!(run.items().is_empty());
}

fn hash_params(t: &Tree, items: Vec<String>) -> LibraryHashParams {
    LibraryHashParams {
        library: "lib1".into(),
        root: t.path().to_str().unwrap().into(),
        hydrate: false,
        read_bps: None,
        volume: identity_of(t.path()),
        files: "in/files.ndjson".into(),
        items,
    }
}

fn hash_start(job: &Job, p: &LibraryHashParams) -> fin_jobs::Start {
    job.start(
        "library.hash",
        serde_json::to_value(p).unwrap(),
        &["in/files.ndjson"],
    )
}

#[test]
fn hash_items_carry_hashes() {
    let t = Tree::new(0);
    write_file(t.path(), "empty.bin", b"");
    write_file(
        t.path(),
        "data.bin",
        &(0..=255u8).cycle().take(300_000).collect::<Vec<_>>(),
    );
    let lines = [("empty.bin", 1), ("data.bin", 2)].map(|(n, id)| FileLine {
        id,
        rel: rel(&[n]),
        expect: expect_of(&t.path().join(n)),
    });
    let job = Job::new();
    let values: Vec<Value> = lines
        .iter()
        .map(|l| serde_json::to_value(l).unwrap())
        .collect();
    job.write_in("in/files.ndjson", &values);
    let mut p = run_job(hash_start(
        &job,
        &hash_params(&t, vec!["1".into(), "2".into()]),
    ));
    let run = p.drive(|method, _| panic!("a hash job makes no call, got {method}"));
    let value = |item: &str| {
        run.events
            .iter()
            .find_map(|e| match e {
                Ev::Item(i, Some(v)) if i == item => Some(v.as_str().unwrap().to_owned()),
                _ => None,
            })
            .unwrap()
    };
    // BLAKE3 of the empty input (the published test vector).
    assert_eq!(
        value("1"),
        "blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    let direct = hash_file(
        &FileRef {
            file: 2,
            root: t.path(),
            rel: &lines[1].rel,
            expect: &lines[1].expect,
            hydrate: false,
            display: Path::new("data.bin"),
        },
        &Throttle::default(),
    );
    assert_eq!(Some(value("2")), direct.hash);
    assert_eq!(run.result()["states"], json!({ "present": 2 }));
}

#[test]
fn hash_failures_are_item_errors() {
    let t = Tree::new(2);
    write_file(t.path(), "p0.png", &[1u8; 2000]);
    std::fs::remove_file(t.path().join("p1.png")).unwrap();
    let job = t.job();
    let mut p = run_job(hash_start(&job, &hash_params(&t, t.items())));
    let run = p.drive(|_, _| Ok(json!({})));
    let failed: Vec<(&str, &str)> = run
        .events
        .iter()
        .filter_map(|e| match e {
            Ev::ItemFailed(i, c) => Some((i.as_str(), c.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(failed, [("1", "FILE_CHANGED"), ("2", "FILE_NOT_FOUND")]);
}
