//! `library.scan` over the real `fin-workers` binary (plan 5D Task 6): batches through
//! calls, progress, offline roots, modes and cancel.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod support;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use fin_jobs::{JobError, Retry};
use fin_library::batchfile;
use fin_library::params::{BatchCall, LibraryScanParams, ScanMode};
use fin_library::{ExcludeSpec, ScanBatch, Timing};
use serde_json::{Value, json};
use support::{End, Ev, Job, identity_of, overlay, rel, run_job, write_file};

fn params(root: &Path, mode: ScanMode) -> LibraryScanParams {
    LibraryScanParams {
        library: "lib1".into(),
        root: root.to_str().unwrap().into(),
        mode,
        generation: 1,
        dirs: None,
        excludes: ExcludeSpec {
            exclude: vec![],
            include: None,
            case_sensitive: true,
        },
        threads: Some(4),
        volume: None,
        timing: Some(Timing {
            first_stat: Duration::from_secs(30),
            entry: Duration::from_secs(10),
            flush: Duration::from_millis(5),
        }),
    }
}

fn start(job: &Job, p: &LibraryScanParams, with_dirs: bool) -> fin_jobs::Start {
    let grants: &[&str] = if with_dirs {
        &["out/", "in/dirs.ndjson"]
    } else {
        &["out/"]
    };
    job.start("library.scan", serde_json::to_value(p).unwrap(), grants)
}

/// Reads and removes nothing: decodes the batch file a `library.batch` call names, which
/// holds one line, one `ScanBatch`.
fn read_batch(job: &Job, call: &Value) -> ScanBatch {
    let c: BatchCall = serde_json::from_value(call.clone()).unwrap();
    assert!(!c.file.contains('/'), "a plain name");
    let f = std::fs::File::open(job.out().join(&c.file)).expect("the file exists at the call");
    let mut lines = batchfile::decode(std::io::BufReader::new(f)).unwrap();
    assert_eq!(lines.len(), 1);
    let b = lines.remove(0);
    assert_eq!(b.seq, c.seq);
    b
}

fn tree(root: &Path, dirs: usize, files: usize) {
    for d in 0..dirs {
        for f in 0..files {
            write_file(root, &format!("d{d}/f{f:04}.mov"), b"x");
        }
    }
}

#[test]
fn scan_streams_batches_through_calls_and_waits_for_replies() {
    let root = tempfile::tempdir().unwrap();
    // 5 directories of 1,000 files: 5,006 entries with the root's, more than one batch.
    tree(root.path(), 5, 1000);
    let job = Job::new();
    let mut p = run_job(start(&job, &params(root.path(), ScanMode::Full), false));
    let mut batches = Vec::new();
    let run = p.drive(|method, call| {
        assert_eq!(method, "library.batch");
        batches.push(read_batch(&job, call));
        // The worker must wait here: nothing it sends after the call may arrive before the
        // reply (checked by `early`).
        std::thread::sleep(Duration::from_millis(150));
        Ok(json!({}))
    });
    assert!(
        run.early.is_empty(),
        "sent before the reply: {:?}",
        run.early
    );
    let result = run.result();
    assert!(batches.len() >= 2, "{} batches", batches.len());
    let seqs: Vec<u64> = batches.iter().map(|b| b.seq).collect();
    assert_eq!(seqs, (0..batches.len() as u64).collect::<Vec<_>>());
    assert!(batches.iter().all(|b| b.entries.len() <= 4096));
    let files: BTreeSet<String> = batches
        .iter()
        .flat_map(|b| &b.entries)
        .filter(|e| e.name_nfc.ends_with(".mov"))
        .map(|e| format!("{}/{}", e.dir.display(), e.name_nfc))
        .collect();
    assert_eq!(files.len(), 5000);
    let done: Vec<String> = batches
        .iter()
        .flat_map(|b| &b.dirs_done)
        .filter(|d| d.complete)
        .map(|d| d.dir.display())
        .collect();
    assert_eq!(done.len(), 6, "the root and five directories: {done:?}");
    assert_eq!(result["listed"], 6);
    assert_eq!(result["entries"], 5005);
    assert_eq!(result["batches"], batches.len());
    // The reply means the indexer applied the file: the worker removes it.
    let left: Vec<_> = std::fs::read_dir(job.out()).unwrap().collect();
    assert!(left.is_empty(), "batch files left behind: {left:?}");
    assert!(p.wait().success());
}

#[test]
fn scan_reports_progress_per_directory() {
    let root = tempfile::tempdir().unwrap();
    tree(root.path(), 5, 3);
    let mut paths = serde_json::Map::new();
    for d in 0..5 {
        paths.insert(format!("d{d}"), json!({ "readDelayMs": 40 }));
    }
    overlay(root.path(), json!({ "paths": paths }));
    let job = Job::new();
    let mut p = run_job(start(&job, &params(root.path(), ScanMode::Full), false));
    let run = p.drive(|_, _| Ok(json!({})));
    run.result();
    let progress = run.progress();
    assert!(progress.len() >= 5, "progress {progress:?}");
    assert!(progress.windows(2).all(|w| w[0] <= w[1]), "{progress:?}");
    assert_eq!(
        *progress.last().unwrap(),
        6.0,
        "the root and five directories"
    );
}

#[test]
fn offline_root_is_library_offline_never_retried() {
    let root = tempfile::tempdir().unwrap();
    tree(root.path(), 1, 2);
    overlay(root.path(), json!({ "statError": "io" }));
    let job = Job::new();
    let mut p = run_job(start(&job, &params(root.path(), ScanMode::Full), false));
    let run = p.drive(|_, _| panic!("no batch of an offline root"));
    let e = run.error();
    assert_eq!(e.code, "LIBRARY_OFFLINE");
    assert_eq!(e.retry, Retry::Never);
    assert_eq!(e.details["state"]["state"], "unreachable");
    assert!(run.calls("library.batch").is_empty());
}

#[test]
fn a_changed_volume_is_offline_with_its_state() {
    let root = tempfile::tempdir().unwrap();
    tree(root.path(), 1, 2);
    let mut stored = identity_of(root.path());
    stored.key = "someone-else".into();
    stored.root_file_id = Some(vec![9; 8]);
    stored.sample = vec![];
    let mut prm = params(root.path(), ScanMode::Full);
    prm.volume = Some(stored);
    let job = Job::new();
    let mut p = run_job(start(&job, &prm, false));
    let run = p.drive(|_, _| panic!("no batch"));
    let e = run.error();
    assert_eq!(e.code, "LIBRARY_OFFLINE");
    assert_eq!(e.details["state"]["state"], "volumeChanged");
}

#[test]
fn dirs_mode_lists_only_the_given_directories() {
    let root = tempfile::tempdir().unwrap();
    tree(root.path(), 3, 2);
    let mut prm = params(root.path(), ScanMode::Dirs);
    prm.dirs = Some(vec![rel(&["d1"])]);
    let job = Job::new();
    let mut p = run_job(start(&job, &prm, false));
    let mut seen = Vec::new();
    let run = p.drive(|_, call| {
        seen.push(read_batch(&job, call));
        Ok(json!({}))
    });
    let result = run.result();
    assert_eq!(result["listed"], 1);
    let names: Vec<_> = seen
        .iter()
        .flat_map(|b| &b.entries)
        .map(|e| e.name_nfc.clone())
        .collect();
    assert_eq!(names, ["f0000.mov", "f0001.mov"]);
}

#[test]
fn poll_lists_only_changed_directories() {
    let root = tempfile::tempdir().unwrap();
    tree(root.path(), 3, 2);
    // A full scan first, to learn the directories as the catalog would keep them.
    let job = Job::new();
    let mut p = run_job(start(&job, &params(root.path(), ScanMode::Full), false));
    let mut dirs = Vec::new();
    p.drive(|_, call| {
        dirs.extend(read_batch(&job, call).dirs_done);
        Ok(json!({}))
    })
    .result();
    // The lines the indexer writes: `{dir, mtimeNs, listed}`.
    let known: Vec<Value> = dirs
        .iter()
        .map(|d| json!({ "dir": d.dir, "mtimeNs": d.mtime_ns.map(|t| t.to_string()), "listed": true }))
        .collect();
    // A new file in d2 moves its mtime.
    std::thread::sleep(Duration::from_millis(20));
    write_file(root.path(), "d2/new.mov", b"y");
    let job = Job::new();
    job.write_in("in/dirs.ndjson", &known);
    let mut p = run_job(start(&job, &params(root.path(), ScanMode::Poll), true));
    let mut names = Vec::new();
    let run = p.drive(|_, call| {
        names.extend(
            read_batch(&job, call)
                .entries
                .into_iter()
                .map(|e| e.name_nfc),
        );
        Ok(json!({}))
    });
    let result = run.result();
    assert_eq!(result["listed"], 1, "{result}");
    assert_eq!(result["unchanged"], 3);
    names.sort();
    assert_eq!(names, ["f0000.mov", "f0001.mov", "new.mov"]);
}

#[test]
fn a_resumed_full_scan_skips_listings_of_its_generation() {
    let root = tempfile::tempdir().unwrap();
    tree(root.path(), 3, 2);
    let known: Vec<Value> = (0..3)
        .map(|d| json!({ "dir": rel(&[&format!("d{d}")]), "mtimeNs": null, "listed": true }))
        .collect();
    let job = Job::new();
    job.write_in("in/dirs.ndjson", &known);
    let mut p = run_job(start(&job, &params(root.path(), ScanMode::Full), true));
    let run = p.drive(|_, _| Ok(json!({})));
    let result = run.result();
    assert_eq!(result["skipped"], 3, "{result}");
    assert_eq!(result["listed"], 1, "only the root is listed again");
}

#[test]
fn bad_params_are_refused_before_any_read() {
    let root = tempfile::tempdir().unwrap();
    let job = Job::new();
    let mut prm = params(root.path(), ScanMode::Dirs);
    let mut p = run_job(start(&job, &prm, false));
    let e = p.drive(|_, _| panic!("no call")).error().clone();
    assert_eq!(
        e.code, "JOB_PARAMS_INVALID",
        "dirs without directories: {e}"
    );
    prm.mode = ScanMode::Poll;
    prm.dirs = None;
    let mut p = run_job(start(&job, &prm, false));
    let e = p.drive(|_, _| panic!("no call")).error().clone();
    assert_eq!(
        e.code, "JOB_PARAMS_INVALID",
        "poll without known directories: {e}"
    );
    prm.mode = ScanMode::Full;
    prm.root = "relative/root".into();
    let mut p = run_job(start(&job, &prm, false));
    let e = p.drive(|_, _| panic!("no call")).error().clone();
    assert_eq!(e.code, "JOB_PARAMS_INVALID", "a relative root: {e}");
}

#[test]
fn a_refused_batch_fails_the_job_with_the_hosts_error() {
    let root = tempfile::tempdir().unwrap();
    tree(root.path(), 1, 2);
    let job = Job::new();
    let mut p = run_job(start(&job, &params(root.path(), ScanMode::Full), false));
    let run = p.drive(|_, _| Err(JobError::new("BATCH_INVALID", "schema")));
    assert_eq!(run.error().code, "BATCH_INVALID");
    assert_eq!(
        run.calls("library.batch").len(),
        1,
        "the walk stops at once"
    );
}

#[test]
fn cancel_stops_a_slow_scan() {
    let root = tempfile::tempdir().unwrap();
    tree(root.path(), 4, 2);
    overlay(
        root.path(),
        json!({ "paths": { "d0": { "readDelayMs": 400 }, "d1": { "readDelayMs": 400 } } }),
    );
    let job = Job::new();
    let mut p = run_job(start(&job, &params(root.path(), ScanMode::Full), false));
    std::thread::sleep(Duration::from_millis(100));
    p.cancel("j1");
    let run = p.drive(|_, _| Ok(json!({})));
    match &run.end {
        End::Error(e) => assert_eq!(e.code, "JOB_CANCELLED"),
        e => panic!("cancelled, got {e:?}"),
    }
    assert!(!run.events.iter().any(|e| matches!(e, Ev::Checkpoint(_))));
    assert!(p.wait().success());
}

#[test]
fn dir_done_counts_every_entry_and_follows_them() {
    let root = tempfile::tempdir().unwrap();
    // Two kept files and three the default rules exclude: dot-file, Thumbs.db, a conflict
    // copy. All five are emitted, so the listing's `entries` is 5 (plan 5D Task 7b).
    for n in [
        "a.mov",
        "b.mov",
        ".hidden",
        "Thumbs.db",
        "c (conflicted copy).mov",
    ] {
        write_file(root.path(), &format!("d/{n}"), b"x");
    }
    write_file(root.path(), "e/only.mov", b"x");
    let job = Job::new();
    let mut p = run_job(start(&job, &params(root.path(), ScanMode::Full), false));
    let mut batches = Vec::new();
    p.drive(|_, call| {
        batches.push(read_batch(&job, call));
        Ok(json!({}))
    })
    .result();
    let d = rel(&["d"]);
    let in_d = |b: &ScanBatch| b.entries.iter().filter(|e| e.dir == d).count();
    let total: usize = batches.iter().map(in_d).sum();
    assert_eq!(total, 5);
    let excluded = batches
        .iter()
        .flat_map(|b| &b.entries)
        .filter(|e| e.dir == d && e.excluded.is_some())
        .count();
    assert!(excluded >= 2, "{excluded} excluded");
    let done = batches
        .iter()
        .flat_map(|b| &b.dirs_done)
        .find(|x| x.dir == d)
        .unwrap();
    assert_eq!(done.entries as usize, total);
    // Every entry of a directory comes before its DirDone, in file order.
    let mut closed = BTreeSet::new();
    for b in &batches {
        for e in &b.entries {
            assert!(
                !closed.contains(&e.dir.display()),
                "entry after its DirDone"
            );
        }
        for x in &b.dirs_done {
            closed.insert(x.dir.display());
        }
    }
}
