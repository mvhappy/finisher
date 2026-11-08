//! The walker over the real Unix source, and the test overlay (plan 5D Task 4): a
//! `.fin-fake.json` at a real root marks paths as placeholders, automounts and mounts, delays
//! and fails reads, and logs every `open_for_read` of a listed path outside the root.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

mod support;
mod walk_util;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fin_library::overlay::{self, FILE};
use fin_library::source::NativeSource;
use fin_library::{OpenError, Placeholder, Source, WalkMode, open_for_read};
use serde_json::{Value, json};
use support::expect_of;
use walk_util::*;

fn write(p: &Path, bytes: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

/// `root/{a/{x/1.mov, y/2.mov}, b/3.mov, cloud/{c.mov, deep/d.mov}, net/, top.mov}` plus
/// [`ODD_NAME`] in `b` and a symlink.
fn real_tree(root: &Path) {
    for f in [
        "a/x/1.mov",
        "a/y/2.mov",
        "b/3.mov",
        "cloud/c.mov",
        "cloud/deep/d.mov",
        "top.mov",
    ] {
        write(&root.join(f), b"data");
    }
    fs::create_dir_all(root.join("net")).unwrap();
    use std::os::unix::ffi::OsStrExt;
    write(
        &root.join("b").join(std::ffi::OsStr::from_bytes(ODD_NAME)),
        b"x",
    );
    std::os::unix::fs::symlink(root.join("a"), root.join("link")).unwrap();
}

/// A name a byte-preserving walk must keep as listed: non-UTF-8 on Linux; on macOS (APFS and
/// HFS+ refuse invalid UTF-8 with `EILSEQ`) an NFD name, which APFS stores and lists as given.
#[cfg(target_os = "linux")]
const ODD_NAME: &[u8] = b"caf\xe9.mov";
#[cfg(not(target_os = "linux"))]
const ODD_NAME: &[u8] = b"cafe\xcc\x81.mov";

fn source(root: &Path) -> Arc<dyn Source> {
    overlay::wrap(root, Arc::new(NativeSource::new(root))).unwrap()
}

fn set_overlay(root: &Path, spec: &Value) {
    fs::write(root.join(FILE), spec.to_string()).unwrap();
}

fn log_lines(log: &Path) -> Vec<Value> {
    match fs::read_to_string(log) {
        Ok(t) => t
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn real_walk_lists_raw_names_and_skips_links() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    real_tree(&root);
    let (r, batches) = run(source(&root), full(1, None), &opts(8));
    let s = r.unwrap();
    assert_eq!(
        done_paths(&batches),
        ["", "a", "b", "cloud", "net", "a/x", "a/y", "cloud/deep"]
    );
    assert_eq!(
        s.identity.enumeration,
        if cfg!(target_os = "linux") {
            "getdents64"
        } else {
            "getattrlistbulk"
        }
    );
    let es = entries(&batches);
    let odd = es.iter().find(|e| e.name == ODD_NAME).unwrap();
    assert_eq!(odd.name_lossy, cfg!(target_os = "linux"));
    if !odd.name_lossy {
        assert_eq!(odd.name_nfc, "caf\u{e9}.mov");
    }
    let link = es.iter().find(|e| e.name == b"link").unwrap();
    assert_eq!(link.entry_type, fin_library::EntryType::Symlink);
    // Every DirDone carries the directory's mtime.
    assert!(dones(&batches).iter().all(|d| d.mtime_ns.is_some()));
}

/// The real source walks to the same batches at every thread count, on a tree wider than the
/// walker's held-descriptor cap (64) and deeper than a few levels.
#[test]
fn real_walk_is_deterministic_across_thread_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    for t in 0..90 {
        let mut d = root.join(format!("t{t:02}"));
        for k in 0..(t % 7) {
            d = d.join(format!("d{k}"));
        }
        fs::create_dir_all(&d).unwrap();
        for f in 0..3 {
            write(&d.join(format!("f{f}.mov")), b"x");
        }
    }
    let mut seen = Vec::new();
    for threads in [1, 3, 8, 64] {
        let (r, batches) = run(source(&root), full(1, None), &opts(threads));
        let s = r.unwrap();
        assert_eq!((s.incomplete, s.timeouts), (0, 0), "threads {threads}");
        seen.push(serde_json::to_string(&batches).unwrap());
    }
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "batches differ by thread count"
    );
}

#[test]
fn real_poll_relists_only_the_changed_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    real_tree(&root);
    let (r, batches) = run(source(&root), full(1, None), &opts(4));
    r.unwrap();
    let known = known_from(&batches, 1);
    let (r, batches) = run(
        source(&root),
        WalkMode::Poll {
            known: known.clone(),
        },
        &opts(4),
    );
    assert_eq!(r.unwrap().listed, 0);
    assert!(batches.is_empty());

    // A new file in a/y moves a/y's mtime (set explicitly: coarse clocks).
    write(&root.join("a/y/new.mov"), b"n");
    let later = SystemTime::now() + Duration::from_secs(5);
    fs::File::open(root.join("a/y"))
        .unwrap()
        .set_modified(later)
        .unwrap();
    let (r, batches) = run(source(&root), WalkMode::Poll { known }, &opts(4));
    assert_eq!(r.unwrap().listed, 1);
    assert_eq!(done_paths(&batches), ["a/y"]);
    assert!(entries(&batches).iter().any(|e| e.name == b"new.mov"));
}

fn overlay_spec(log: &Path) -> Value {
    json!({
        "openLog": log,
        "paths": {
            "cloud": { "placeholder": "dataless" },
            "b/3.mov": { "placeholder": "recallOnData" },
            "net": { "automount": true },
            "a/x/1.mov": {}
        }
    })
}

#[test]
fn fake_placeholders_open_counter_is_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let log = tmp.path().join("opens.ndjson");
    real_tree(&root);
    set_overlay(&root, &overlay_spec(&log));

    let (r, batches) = run(source(&root), full(1, None), &opts(4));
    r.unwrap();
    assert_eq!(
        done_paths(&batches),
        [
            "",
            "cloud!CLOUD_ONLY",
            "net!AUTOMOUNT",
            "a",
            "b",
            "a/x",
            "a/y"
        ]
    );
    let es = entries(&batches);
    let stub = es.iter().find(|e| e.name == b"3.mov").unwrap();
    assert_eq!(stub.placeholder, Some(Placeholder::RecallOnData));
    let cloud = es.iter().find(|e| e.name == b"cloud").unwrap();
    assert_eq!(cloud.placeholder, Some(Placeholder::Dataless));
    assert!(
        es.iter()
            .all(|e| e.name != FILE.as_bytes() || e.excluded.is_some())
    );

    let known = known_from(&batches, 1);
    for mode in [
        full(2, Some(known.clone())),
        WalkMode::Dirs {
            dirs: vec![rel(""), rel("b")],
            known: Some(known.clone()),
        },
        WalkMode::Poll {
            known: known.clone(),
        },
    ] {
        run(source(&root), mode, &opts(4)).0.unwrap();
    }
    // The root changed: a poll re-lists it and meets the placeholders again.
    write(&root.join("added.mov"), b"a");
    fs::File::open(&root)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    let (r, b) = run(source(&root), WalkMode::Poll { known }, &opts(4));
    r.unwrap();
    assert!(done_paths(&b).contains(&"cloud!CLOUD_ONLY".to_owned()));
    assert_eq!(
        log_lines(&log),
        Vec::<Value>::new(),
        "a walk touched a placeholder"
    );

    // Every open_for_read of a listed path is logged; placeholders are refused.
    let refused = open_for_read(
        &root,
        &rel("b/3.mov"),
        &expect_of(&root.join("b/3.mov")),
        false,
    );
    assert!(matches!(refused, Err(OpenError::CloudOnly)), "{refused:?}");
    let below = open_for_read(
        &root,
        &rel("cloud/deep/d.mov"),
        &expect_of(&root.join("cloud/deep/d.mov")),
        false,
    );
    assert!(matches!(below, Err(OpenError::CloudOnly)), "{below:?}");
    open_for_read(
        &root,
        &rel("b/3.mov"),
        &expect_of(&root.join("b/3.mov")),
        true,
    )
    .unwrap();
    open_for_read(
        &root,
        &rel("a/x/1.mov"),
        &expect_of(&root.join("a/x/1.mov")),
        false,
    )
    .unwrap();
    open_for_read(
        &root,
        &rel("top.mov"),
        &expect_of(&root.join("top.mov")),
        false,
    )
    .unwrap();
    assert_eq!(
        log_lines(&log),
        [
            json!({ "op": "open", "path": "b/3.mov", "hydrate": false, "refused": true }),
            json!({ "op": "open", "path": "cloud/deep/d.mov", "hydrate": false, "refused": true }),
            json!({ "op": "open", "path": "b/3.mov", "hydrate": true, "refused": false }),
            json!({ "op": "open", "path": "a/x/1.mov", "hydrate": false, "refused": false }),
        ]
    );
}

#[test]
fn overlay_unmounts_delays_and_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let log = tmp.path().join("opens.ndjson");
    real_tree(&root);
    set_overlay(
        &root,
        &json!({ "openLog": log, "paths": { "a": { "mountRoot": true } } }),
    );
    let (r, batches) = run(source(&root), full(1, None), &opts(4));
    r.unwrap();
    let known = known_from(&batches, 1);
    assert!(known.get(&rel("a")).unwrap().mount_root);

    // `a` unmounts; `b` fails; `cloud` is slow per entry but progresses; the root spins up.
    set_overlay(
        &root,
        &json!({
            "openLog": log,
            "statDelayMs": 150,
            "paths": {
                "a": { "unmounted": true },
                "b": { "error": "io" },
                "cloud": { "entryDelayMs": 120 },
                "net": { "openError": "permissionDenied" }
            }
        }),
    );
    let mut o = opts(4);
    o.timing = timing(1_000, 300);
    let (r, batches) = run(source(&root), full(2, Some(known)), &o);
    let s = r.unwrap();
    assert_eq!(
        done_paths(&batches),
        [
            "",
            "a!ROOT_UNMOUNTED",
            "b!IO",
            "cloud",
            "net!PERMISSION_DENIED",
            "cloud/deep"
        ]
    );
    assert_eq!(
        s.timeouts, 0,
        "a slow listing that progresses is not timed out"
    );

    // Past the first-stat deadline: offline.
    set_overlay(&root, &json!({ "openLog": log, "statDelayMs": 1_500 }));
    o.timing = timing(300, 300);
    let (r, _) = run(source(&root), full(3, None), &o);
    assert_eq!(offline(&r), Some(fin_library::RootState::Timeout), "{r:?}");

    // A log inside the root is refused.
    set_overlay(&root, &json!({ "openLog": root.join("log") }));
    assert!(overlay::wrap(&root, Arc::new(NativeSource::new(&root))).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn descriptors_stay_bounded_on_wide_trees() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    for i in 0..1_500 {
        fs::create_dir_all(root.join(format!("d{i:04}/sub"))).unwrap();
    }
    let fds = || fs::read_dir("/proc/self/fd").unwrap().count();
    let base = fds();
    let mut most = 0;
    let mut o = opts(8);
    o.batch_entries = 64;
    let r = fin_library::walk(
        source(&root),
        full(1, None),
        &o,
        &mut |_| {
            most = most.max(fds());
            Ok(())
        },
        &fin_library::CancelToken::new(),
    );
    assert_eq!(r.unwrap().listed, 3_001);
    // 64 kept handles and 8 running reads at most, plus what other tests hold meanwhile.
    assert!(
        most < base + 64 + 8 + 100,
        "{most} descriptors (from {base})"
    );
}
