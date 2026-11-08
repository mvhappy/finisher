//! Probe and hash of library files (plan 5D Task 5): one handle through the safe opener,
//! the fingerprint kept across a failed probe, the budget and cloud-only rules.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

mod support;

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fin_jobs::ReadBudget;
use fin_jobs::budget::BudgetClock;
use fin_library::{CancelToken, Expect, FileRef, Throttle, hash_file, probe_file};
use fin_probe::PROBE_VERSION;
use support::{expect_of, rel};

/// A clock whose `sleep` advances `now`.
#[derive(Default)]
struct Fake(AtomicU64);

impl BudgetClock for Fake {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::SeqCst))
    }

    fn sleep(&self, d: Duration) {
        self.0.fetch_add(d.as_millis() as u64, Ordering::SeqCst);
    }
}

struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    fn new() -> Self {
        Tree {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.dir.path().join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    /// A [`FileRef`] of `name` with the catalog's current facts about it.
    fn with<T>(&self, name: &str, hydrate: bool, f: impl FnOnce(&FileRef<'_>) -> T) -> T {
        let path = self.dir.path().join(name);
        let r = rel(&[name.as_bytes()]);
        let expect = expect_of(&path);
        f(&FileRef {
            file: 7,
            root: self.dir.path(),
            rel: &r,
            expect: &expect,
            hydrate,
            display: &path,
        })
    }
}

fn png(path: &Path) {
    fin_probe::fixtures::write_png(path, 64, 36);
}

#[test]
fn probe_file_is_probe_of_the_file() {
    let t = Tree::new();
    png(&t.dir.path().join("a.png"));
    let o = t.with("a.png", false, |f| probe_file(f, &Throttle::default()));
    let want = fin_probe::probe(&t.dir.path().join("a.png")).unwrap();
    assert_eq!(o.state, "present");
    assert_eq!(o.code, None);
    assert_eq!((o.file, o.probe_version), (7, PROBE_VERSION));
    assert_eq!(o.fingerprint.as_deref(), Some(want.fingerprint.as_str()));
    assert_eq!(o.probe, Some(want));
}

#[test]
fn probe_through_the_safe_opener_with_probe_with() {
    let t = Tree::new();
    png(&t.dir.path().join("a.png"));
    let want = fin_probe::probe(&t.dir.path().join("a.png")).unwrap();
    // Task 13's shape: `asset.probe` of a library file opens with the catalogued row.
    let got = t.with("a.png", false, |f| {
        fin_probe::probe_with(f.display, |_| f.open().map_err(std::io::Error::other))
    });
    assert_eq!(got.unwrap(), want);
}

#[test]
fn probe_failure_keeps_the_fingerprint() {
    let t = Tree::new();
    let p = t.put("x.png", b"\0\x9f\x92\x96 not an image at all");
    let o = t.with("x.png", false, |f| probe_file(f, &Throttle::default()));
    assert_eq!(o.state, "probeFailed");
    assert_eq!(o.code.as_deref(), Some("UNSUPPORTED_CONTAINER"));
    assert_eq!(o.probe, None);
    assert_eq!(o.fingerprint, Some(fin_probe::fingerprint(&p).unwrap()));
}

#[test]
fn hash_is_whole_file_blake3() {
    let t = Tree::new();
    // Over the 192 KiB the fingerprint samples: the hash still reads every byte.
    let bytes: Vec<u8> = (0..(900 * 1024 + 13))
        .map(|i| (i * 31 % 251) as u8)
        .collect();
    t.put("big.bin", &bytes);
    let o = t.with("big.bin", false, |f| hash_file(f, &Throttle::default()));
    assert_eq!(o.state, "present");
    assert_eq!(
        o.hash,
        Some(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
    );
    t.put("empty.bin", b"");
    let o = t.with("empty.bin", false, |f| hash_file(f, &Throttle::default()));
    assert_eq!(
        o.hash,
        Some(format!("blake3:{}", blake3::hash(b"").to_hex()))
    );
}

#[test]
fn budget_limits_probe_reads() {
    let t = Tree::new();
    // An SVG is read whole: a megabyte of comment inside the root element.
    let pad = "x".repeat(1024 * 1024 - 100);
    t.put(
        "big.svg",
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"9\" height=\"9\"><!--{pad}--></svg>"
        )
        .as_bytes(),
    );
    let rate = 100_000u64;
    let clock = Arc::new(Fake::default());
    let throttle = Throttle::new(Some(ReadBudget::with_clock(rate, clock.clone())), None);
    let o = t.with("big.svg", false, |f| probe_file(f, &throttle));
    assert_eq!(o.state, "present", "{o:?}");
    // The probe reads the whole file and the fingerprint's three samples: at the budget's
    // rate that takes (bytes − the one tenth-second bucket) / rate of the clock's time.
    let size = fs::metadata(t.dir.path().join("big.svg")).unwrap().len();
    let read = size + 3 * 64 * 1024;
    let floor_ms = (read - rate / 10) * 1000 / rate;
    let spent = clock.now().as_millis() as u64;
    assert!(
        spent >= floor_ms,
        "slept {spent} ms, at least {floor_ms} expected"
    );
    assert!(
        spent < floor_ms * 12 / 10,
        "slept {spent} ms for {floor_ms}"
    );

    // The hash is paid for too.
    let clock = Arc::new(Fake::default());
    let throttle = Throttle::new(Some(ReadBudget::with_clock(rate, clock.clone())), None);
    let o = t.with("big.svg", false, |f| hash_file(f, &throttle));
    assert_eq!(o.state, "present");
    assert!(clock.now().as_millis() as u64 >= (size - rate / 10) * 1000 / rate);
}

#[test]
fn cancel_stops_a_read() {
    let t = Tree::new();
    t.put("big.bin", &vec![7u8; 600 * 1024]);
    let cancel = CancelToken::new();
    cancel.cancel();
    let throttle = Throttle::new(None, Some(cancel));
    let o = t.with("big.bin", false, |f| probe_file(f, &throttle));
    assert_eq!(
        (o.state.as_str(), o.fingerprint, o.probe),
        ("cancelled", None, None)
    );
    let h = t.with("big.bin", false, |f| hash_file(f, &throttle));
    assert_eq!((h.state.as_str(), h.hash), ("cancelled", None));
}

#[test]
fn cloud_only_refused_unless_hydrate() {
    let t = Tree::new();
    png(&t.dir.path().join("cloud.png"));
    t.put(
        ".fin-fake.json",
        br#"{"paths": {"cloud.png": {"placeholder": "dataless"}}}"#,
    );
    let o = t.with("cloud.png", false, |f| probe_file(f, &Throttle::default()));
    assert_eq!(o.state, "cloudOnly");
    assert_eq!(o.code.as_deref(), Some("FILE_CLOUD_ONLY"));
    assert_eq!((o.fingerprint, o.probe), (None, None));
    let h = t.with("cloud.png", false, |f| hash_file(f, &Throttle::default()));
    assert_eq!((h.state.as_str(), h.hash), ("cloudOnly", None));
    // With `hydrate` the library asked for the download.
    let o = t.with("cloud.png", true, |f| probe_file(f, &Throttle::default()));
    assert_eq!(o.state, "present", "{o:?}");
    assert!(o.probe.is_some());

    // A Nextcloud stub: the file's data is not here, whatever `hydrate` says.
    t.put("n.png.nextcloud", b"");
    let r = rel(&[b"n.png"]);
    let expect = fin_library::Expect {
        size: 10,
        mtime_ns: 0,
        file_id: None,
        dev: None,
    };
    let f = FileRef {
        file: 1,
        root: t.dir.path(),
        rel: &r,
        expect: &expect,
        hydrate: false,
        display: Path::new("n.png"),
    };
    let o = probe_file(&f, &Throttle::default());
    assert_eq!(o.state, "cloudOnly");
}

#[test]
fn wrong_device_is_changed_not_probed() {
    // `Expect.dev` is the catalogued entry's device: a handle on another one is not the file
    // the catalog read, so nothing is read from it.
    let t = Tree::new();
    png(&t.dir.path().join("a.png"));
    let path = t.dir.path().join("a.png");
    let r = rel(&[b"a.png"]);
    let moved = Expect {
        dev: Some("0:0".into()),
        ..expect_of(&path)
    };
    let f = FileRef {
        file: 4,
        root: t.dir.path(),
        rel: &r,
        expect: &moved,
        hydrate: false,
        display: &path,
    };
    let o = probe_file(&f, &Throttle::default());
    assert_eq!(
        (o.state.as_str(), o.code.as_deref()),
        ("changed", Some("FILE_CHANGED"))
    );
    assert_eq!((o.fingerprint, o.probe), (None, None));
    let h = hash_file(&f, &Throttle::default());
    assert_eq!((h.state.as_str(), h.hash), ("changed", None));
    // Without a device the handle must be on the root's mount, which it is.
    let none = Expect {
        dev: None,
        ..moved.clone()
    };
    let f = FileRef { expect: &none, ..f };
    assert_eq!(probe_file(&f, &Throttle::default()).state, "present");
}

#[test]
fn changed_identity_is_changed_not_probed() {
    let t = Tree::new();
    png(&t.dir.path().join("a.png"));
    let path = t.dir.path().join("a.png");
    let r = rel(&[b"a.png"]);
    let stale = expect_of(&path);
    // The file is replaced after the catalog read it.
    fin_probe::fixtures::write_png(&path, 640, 360);
    let f = FileRef {
        file: 3,
        root: t.dir.path(),
        rel: &r,
        expect: &stale,
        hydrate: false,
        display: &path,
    };
    let o = probe_file(&f, &Throttle::default());
    assert_eq!(o.state, "changed");
    assert_eq!(o.code.as_deref(), Some("FILE_CHANGED"));
    assert_eq!((o.fingerprint, o.probe), (None, None));
    let h = hash_file(&f, &Throttle::default());
    assert_eq!((h.state.as_str(), h.hash), ("changed", None));

    // Gone entirely.
    fs::remove_file(&path).unwrap();
    let o = probe_file(&f, &Throttle::default());
    assert_eq!(o.state, "missing");
    assert_eq!(o.code.as_deref(), Some("FILE_NOT_FOUND"));
}

#[test]
fn raw_non_utf8_names_probe_by_their_chain() {
    let t = Tree::new();
    let dir: &[u8] = b"d\xff";
    let name: &[u8] = b"f\xfe.png";
    let dp = t.dir.path().join(std::ffi::OsStr::from_bytes(dir));
    fs::create_dir(&dp).unwrap();
    let path = dp.join(std::ffi::OsStr::from_bytes(name));
    png(&path);
    let r = rel(&[dir, name]);
    let expect = expect_of(&path);
    let f = FileRef {
        file: 9,
        root: t.dir.path(),
        rel: &r,
        expect: &expect,
        hydrate: false,
        display: Path::new("d\u{fffd}/f\u{fffd}.png"),
    };
    let o = probe_file(&f, &Throttle::default());
    assert_eq!(o.state, "present", "{o:?}");
    assert!(o.probe.is_some());
}
