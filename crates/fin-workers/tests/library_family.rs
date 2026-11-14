//! The `library` family as a whole (plan 5D Task 6): its hello, the name tables, the dataless
//! policy and the rule that it starts no process.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod support;

use std::path::Path;
#[cfg(target_os = "linux")]
use std::time::Duration;

use fin_core::events::names_jobs::{JOB_KINDS, WORKER_FAMILIES};
use fin_jobs::{Role, envelope_id, kinds};
use fin_library::ExcludeSpec;
use fin_library::params::{LibraryScanParams, PARAMS, ScanMode};
use serde_json::json;
use support::{Job, run_job, spawn};
#[cfg(target_os = "linux")]
use support::{overlay, write_file};

#[test]
fn hello_lists_the_library_kinds() {
    let mut p = spawn("library");
    let h = p.hello();
    assert_eq!(
        (h.v, h.minor, h.role, h.family.as_str()),
        (1, 0, Role::Worker, "library")
    );
    let kinds_served: Vec<&str> = h.kinds.iter().map(|k| k.kind.as_ref()).collect();
    // The test binary also serves `test.stage`, the stage framework's test kind (Task 11).
    assert_eq!(
        kinds_served,
        [
            "library.scan",
            "library.probe",
            "library.hash",
            "test.stage"
        ]
    );
    assert_eq!(h.envelope.as_deref(), Some(envelope_id()));
    assert!(!h.warm);
    // The drift test: the family and every kind it serves are in fin-core's tables and have
    // their params type.
    assert!(WORKER_FAMILIES.get("library").is_some());
    for k in &h.kinds {
        assert!(JOB_KINDS.get(&k.kind).is_some(), "{}", k.kind);
        assert!(
            kinds::find(
                &[PARAMS, fin_library::params_test::PARAMS],
                &k.kind,
                k.version
            )
            .is_some(),
            "{}",
            k.kind
        );
    }
}

#[test]
fn params_validate_through_the_registry() {
    let scan = kinds::find(&[PARAMS], "library.scan", 1).unwrap();
    let ok = json!({
        "library": "l", "root": "/r", "mode": "full", "gen": 3,
        "excludes": { "caseSensitive": true }
    });
    let filled = (scan.validate)(&ok).unwrap();
    assert_eq!(filled["gen"], 3);
    assert!(filled["excludes"]["exclude"].as_array().unwrap().is_empty());
    let mut unknown = ok.clone();
    unknown["surprise"] = json!(1);
    let errors = (scan.validate)(&unknown).unwrap_err();
    assert!(errors[0].contains("surprise"), "{errors:?}");
    let mut bad_mode = ok;
    bad_mode["mode"] = json!("sideways");
    assert!((scan.validate)(&bad_mode).is_err());
}

#[test]
fn family_denies_dataless_before_hello() {
    // `init` runs between the self-sandbox entry and `hello`; a job reports the policy the
    // OS then holds: off on macOS, nothing to turn off elsewhere.
    let root = tempfile::tempdir().unwrap();
    let job = Job::new();
    let p = LibraryScanParams {
        library: "l".into(),
        root: root.path().to_str().unwrap().into(),
        mode: ScanMode::Full,
        generation: 1,
        dirs: None,
        excludes: ExcludeSpec {
            exclude: vec![],
            include: None,
            case_sensitive: true,
        },
        threads: Some(1),
        volume: None,
        timing: None,
    };
    let mut proc = run_job(job.start("library.scan", serde_json::to_value(&p).unwrap(), &["out/"]));
    let run = proc.drive(|_, _| Ok(json!({})));
    let want = if cfg!(target_os = "macos") {
        "off"
    } else {
        "notApplicable"
    };
    assert_eq!(run.result()["datalessPolicy"], want);
}

/// Every `.rs` file below `dir`.
fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn family_starts_no_process() {
    // Decision 1: `fin-library` and the family create no process. The clippy gate forbids the
    // path-based reads; this one forbids `std::process::Command` in the same crates' sources.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/library"),
        &mut files,
    );
    sources(&crates.join("fin-library/src"), &mut files);
    assert!(files.len() > 20, "{} files", files.len());
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap();
            assert!(
                !code.contains("Command") && !code.contains("process::exit"),
                "{}:{} starts or ends a process: {line}",
                f.display(),
                i + 1
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_running_scan_has_no_child_processes() {
    let root = tempfile::tempdir().unwrap();
    for d in 0..4 {
        write_file(root.path(), &format!("d{d}/a.mov"), b"x");
    }
    overlay(
        root.path(),
        json!({ "paths": { "d0": { "readDelayMs": 150 }, "d1": { "readDelayMs": 150 },
                           "d2": { "readDelayMs": 150 }, "d3": { "readDelayMs": 150 } } }),
    );
    let job = Job::new();
    let p = LibraryScanParams {
        library: "l".into(),
        root: root.path().to_str().unwrap().into(),
        mode: ScanMode::Full,
        generation: 1,
        dirs: None,
        excludes: ExcludeSpec {
            exclude: vec![],
            include: None,
            case_sensitive: true,
        },
        threads: Some(1),
        volume: None,
        timing: None,
    };
    let mut proc = run_job(job.start("library.scan", serde_json::to_value(&p).unwrap(), &["out/"]));
    let pid = proc.pid();
    let watcher = std::thread::spawn(move || {
        let mut seen = 0;
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(20));
            let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
                break;
            };
            for t in tasks.flatten() {
                let children =
                    std::fs::read_to_string(t.path().join("children")).unwrap_or_default();
                assert!(children.trim().is_empty(), "children of {pid}: {children}");
                seen += 1;
            }
        }
        seen
    });
    proc.drive(|_, _| Ok(json!({}))).result();
    assert!(watcher.join().unwrap() > 0, "the worker was observed");
}
