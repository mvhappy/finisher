//! The ACL broker (plan 7C Task 10, Decision 18). The crate is a napi `cdylib`, so this
//! includes the napi-free broker by path.
//!
//! On every OS, against [`FileFake`] (a DACL per path kept in a file, changed by an unlocked
//! read-modify-write as `SetEntriesInAclW` + `SetNamedSecurityInfoW` are): the log, its replay
//! and compaction; a crash at each named point (a child of this binary aborted by the
//! `test-hooks` hook) completed at the next open; two concurrent grants on one directory both
//! kept (the S8 lost-grant race, with its unserialised control); the sweep of owners gone;
//! protected roots refused before anything is logged. On Windows, the real DACL read back
//! with `acl::sddl`, and `PipeServer.allow(<container SID>)` per project, with another
//! project's container shown refused by `AccessCheck` on a token of a process in it (no
//! escape attempt: owner decision 2026-11-08).

#![allow(dead_code, clippy::disallowed_methods)]

#[path = "../src/acl_broker/mod.rs"]
mod acl_broker;

#[cfg(windows)]
#[path = "../src/pipe_loop.rs"]
mod pipe_loop;
#[cfg(windows)]
#[path = "../src/pipe_sec.rs"]
mod pipe_sec;
#[cfg(windows)]
#[path = "../src/pipe_win.rs"]
mod pipe_win;
#[cfg(windows)]
#[path = "pipe_server/support.rs"]
#[allow(unused_imports)]
mod support;

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use acl_broker::{Apply, Broker, BrokerError, Grant, Options, STATIC_OWNER, WAL_FILE};

const MODIFY: u32 = 0x0013_01BF;
const READ: u32 = 0x0012_0089;
const READ_EXECUTE: u32 = 0x0012_00A9;
const SID_A: &str = "S-1-15-3-1024-1-2-3-4-5-6-7-8";
const SID_B: &str = "S-1-15-3-1024-9-9-9-9-9-9-9-9";

/// One ACE of the fake DACL.
type Aces = BTreeMap<String, Vec<(String, u32, bool)>>;

/// A DACL per path in `<dir>/fake-acl.json`, so a crashed child's grants are seen by the next
/// process. Each change reads the file, waits `delay`, then writes it back, unlocked: a
/// read-modify-write like the Win32 one.
#[derive(Clone)]
struct FileFake {
    file: PathBuf,
    delay: Duration,
    fail_grant: bool,
}

impl FileFake {
    fn new(dir: &Path) -> FileFake {
        FileFake {
            file: dir.join("fake-acl.json"),
            delay: Duration::ZERO,
            fail_grant: false,
        }
    }

    fn read(&self) -> Aces {
        std::fs::read_to_string(&self.file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn write(&self, a: &Aces) {
        std::fs::write(&self.file, serde_json::to_string(a).unwrap()).unwrap();
    }

    fn sids(&self, p: &Path) -> Vec<String> {
        let mut v: Vec<String> = self
            .read()
            .get(&key(p))
            .map(|a| a.iter().map(|(s, ..)| s.clone()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }
}

fn key(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl Apply for FileFake {
    fn grant(&self, path: &Path, sid: &str, mask: u32, inherit: bool) -> io::Result<()> {
        if self.fail_grant {
            return Err(io::Error::other("injected"));
        }
        std::fs::symlink_metadata(path)?;
        let mut all = self.read();
        std::thread::sleep(self.delay);
        let aces = all.entry(key(path)).or_default();
        match aces.iter_mut().find(|(s, ..)| s == sid) {
            Some(a) => a.1 |= mask,
            None => aces.push((sid.to_owned(), mask, inherit)),
        }
        self.write(&all);
        Ok(())
    }

    fn revoke(&self, path: &Path, sid: &str) -> io::Result<usize> {
        let mut all = self.read();
        std::thread::sleep(self.delay);
        let aces = all.entry(key(path)).or_default();
        let before = aces.len();
        aces.retain(|(s, ..)| s != sid);
        let n = before - aces.len();
        self.write(&all);
        if std::fs::symlink_metadata(path).is_err() {
            return Err(io::ErrorKind::NotFound.into());
        }
        Ok(n)
    }
}

/// A server data directory with one session's directories and one project.
struct Tree {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    project: PathBuf,
}

impl Tree {
    fn new() -> Tree {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let data = root.join("data");
        for d in [
            "agents/s1/tmp",
            "agents/s1/provider",
            "agents/s1/settings",
            "agents/s1/git",
            "agents/s1/blobs",
            "auth",
            "run",
            "jobs/j1",
            "plugins/store/p1",
        ] {
            std::fs::create_dir_all(data.join(d)).unwrap();
        }
        std::fs::write(data.join("agents/s1/credential.json"), "{}").unwrap();
        std::fs::write(data.join("auth/local.key"), "k").unwrap();
        let project = root.join("p");
        for d in [".finisher", "assets", "workspace/s1", "sources"] {
            std::fs::create_dir_all(project.join(d)).unwrap();
        }
        std::fs::write(project.join("finisher.project.json"), "{}").unwrap();
        Tree {
            _tmp: tmp,
            root,
            data,
            project,
        }
    }

    fn fake(&self) -> FileFake {
        FileFake::new(&self.root)
    }

    fn wal(&self) -> PathBuf {
        self.data.join("run").join(WAL_FILE)
    }

    fn wal_text(&self) -> String {
        std::fs::read_to_string(self.wal()).unwrap_or_default()
    }

    fn open(&self, fake: FileFake, live: &[&str]) -> Broker<FileFake> {
        self.open_with(fake, live, Options::default())
    }

    fn open_with(&self, fake: FileFake, live: &[&str], opts: Options) -> Broker<FileFake> {
        let live: HashSet<String> = live.iter().map(|s| s.to_string()).collect();
        Broker::open(&self.data, fake, opts, &|o| live.contains(o)).expect("the broker opens")
    }

    fn tmp(&self) -> PathBuf {
        self.data.join("agents/s1/tmp")
    }
}

fn grant(path: &Path, sid: &str, mask: u32, owner: &str) -> Grant {
    Grant {
        path: path.to_path_buf(),
        sid: sid.into(),
        mask,
        inherit: true,
        owner: owner.into(),
    }
}

fn owners(b: &Broker<FileFake>) -> Vec<String> {
    let mut v: Vec<String> = b.grants().into_iter().map(|g| g.owner).collect();
    v.sort();
    v
}

#[test]
fn a_grant_is_logged_applied_and_replayed() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open(fake.clone(), &[]);
    b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
        .unwrap();
    b.grant(&grant(&t.project, SID_B, READ_EXECUTE, STATIC_OWNER))
        .unwrap();
    assert_eq!(fake.sids(&t.tmp()), [SID_A]);
    assert_eq!(fake.sids(&t.project), [SID_B]);
    // grant, done, grant, done.
    assert_eq!(t.wal_text().lines().count(), 4, "{}", t.wal_text());
    drop(b);
    let b = t.open(fake.clone(), &["session:s1"]);
    assert_eq!(owners(&b), ["session:s1", STATIC_OWNER]);
    // Revoked by owner: the ACE goes, the log says so after a reopen.
    assert_eq!(b.revoke("session:s1", None).unwrap(), 1);
    assert!(fake.sids(&t.tmp()).is_empty());
    drop(b);
    assert_eq!(owners(&t.open(fake, &["session:s1"])), [STATIC_OWNER]);
}

#[test]
fn the_log_stays_bounded() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open_with(fake.clone(), &[], Options { compact_slack: 4 });
    b.grant(&grant(&t.project, SID_B, READ_EXECUTE, STATIC_OWNER))
        .unwrap();
    for _ in 0..50 {
        b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
            .unwrap();
        b.revoke("session:s1", None).unwrap();
    }
    let lines = t.wal_text().lines().count();
    assert_eq!(lines, b.wal_lines());
    // 2 per live grant, the slack, and what the last operation appended since.
    assert!(lines <= 2 + 4 + 4, "{lines} lines:\n{}", t.wal_text());
    drop(b);
    assert_eq!(owners(&t.open(fake, &[])), [STATIC_OWNER]);
}

#[test]
fn a_torn_last_line_is_dropped_and_a_damaged_one_refused() {
    let t = Tree::new();
    let fake = t.fake();
    drop(t.open(fake.clone(), &[]));
    let g = serde_json::json!({"op": "grant", "id": 1, "path": t.tmp(), "sid": SID_A, "mask": MODIFY, "inherit": true, "owner": STATIC_OWNER});
    let done = r#"{"op":"done","id":1}"#;
    std::fs::write(t.wal(), format!("{g}\n{done}\n{{\"op\":\"gra")).unwrap();
    let b = t.open(fake.clone(), &[]);
    assert_eq!(owners(&b), [STATIC_OWNER]);
    drop(b);
    // Rewritten at open: the torn tail is gone.
    assert_eq!(t.wal_text().lines().count(), 2, "{}", t.wal_text());
    std::fs::write(t.wal(), format!("{g}\nnot json\n{done}\n")).unwrap();
    let r = Broker::open(&t.data, fake, Options::default(), &|_| true);
    assert!(
        matches!(r, Err(BrokerError::Io(_))),
        "a damaged log is refused"
    );
}

#[test]
fn two_concurrent_grants_on_one_directory_both_survive() {
    let t = Tree::new();
    let mut fake = t.fake();
    fake.delay = Duration::from_millis(60);
    // The control: unserialised, the fake loses one of two grants (the S8 race).
    let barrier = Arc::new(Barrier::new(2));
    let lost: Vec<_> = [SID_A, SID_B]
        .into_iter()
        .map(|sid| {
            let (f, b, p) = (fake.clone(), barrier.clone(), t.tmp());
            std::thread::spawn(move || {
                b.wait();
                f.grant(&p, sid, MODIFY, true).unwrap();
            })
        })
        .collect();
    lost.into_iter().for_each(|h| h.join().unwrap());
    assert_eq!(fake.sids(&t.tmp()).len(), 1, "the fake models the race");
    std::fs::remove_file(&fake.file).unwrap();
    // Through the broker, both are kept; also a grant on a child of a directory being granted.
    let b = Arc::new(t.open(fake.clone(), &[]));
    let barrier = Arc::new(Barrier::new(2));
    let both: Vec<_> = [SID_A, SID_B]
        .into_iter()
        .map(|sid| {
            let (b, w, p) = (b.clone(), barrier.clone(), t.tmp());
            std::thread::spawn(move || {
                w.wait();
                b.grant(&grant(&p, sid, MODIFY, &format!("session:{sid}")))
                    .unwrap();
            })
        })
        .collect();
    both.into_iter().for_each(|h| h.join().unwrap());
    assert_eq!(fake.sids(&t.tmp()), [SID_A, SID_B]);
    assert_eq!(b.grants().len(), 2);
}

/// Records whether two operations on related paths (one inside the other) ever overlapped:
/// inheritance rewrites the descendants' DACLs, so a grant on a parent races one on a child.
#[derive(Default)]
struct Overlap {
    active: Mutex<Vec<PathBuf>>,
    overlapped: std::sync::atomic::AtomicBool,
}

impl Overlap {
    fn run(&self, path: &Path) {
        {
            let mut a = self.active.lock().unwrap();
            if a.iter().any(|q| q.starts_with(path) || path.starts_with(q)) {
                self.overlapped
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            a.push(path.to_path_buf());
        }
        std::thread::sleep(Duration::from_millis(60));
        let mut a = self.active.lock().unwrap();
        let i = a.iter().position(|q| q == path).unwrap();
        a.remove(i);
    }
}

impl Apply for Arc<Overlap> {
    fn grant(&self, path: &Path, _sid: &str, _mask: u32, _inherit: bool) -> io::Result<()> {
        self.run(path);
        Ok(())
    }

    fn revoke(&self, path: &Path, _sid: &str) -> io::Result<usize> {
        self.run(path);
        Ok(1)
    }
}

#[test]
fn a_directory_and_one_inside_it_are_never_changed_at_once() {
    let t = Tree::new();
    let fake = Arc::new(Overlap::default());
    let b = Arc::new(Broker::open(&t.data, fake.clone(), Options::default(), &|_| true).unwrap());
    let inner = t.project.join("workspace/s1");
    let barrier = Arc::new(Barrier::new(3));
    let hs: Vec<_> = [
        (t.project.clone(), READ_EXECUTE, STATIC_OWNER),
        (inner.clone(), MODIFY, "session:s1"),
        (t.tmp(), MODIFY, "session:s1"),
    ]
    .into_iter()
    .map(|(p, mask, owner)| {
        let (b, w) = (b.clone(), barrier.clone());
        std::thread::spawn(move || {
            w.wait();
            b.grant(&grant(&p, SID_A, mask, owner)).unwrap();
        })
    })
    .collect();
    hs.into_iter().for_each(|h| h.join().unwrap());
    assert!(
        !fake.overlapped.load(std::sync::atomic::Ordering::SeqCst),
        "serialised"
    );
    assert_eq!(b.grants().len(), 3);
}

#[test]
fn the_sweep_revokes_the_grants_of_owners_gone() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open(fake.clone(), &[]);
    b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:a"))
        .unwrap();
    b.grant(&grant(&t.data.join("jobs/j1"), SID_B, MODIFY, "job:b"))
        .unwrap();
    b.grant(&grant(&t.project, SID_B, READ_EXECUTE, STATIC_OWNER))
        .unwrap();
    let live: HashSet<String> = ["session:a".to_string()].into();
    assert_eq!(b.sweep(&live).unwrap(), 1);
    assert_eq!(owners(&b), ["session:a", STATIC_OWNER]);
    assert!(fake.sids(&t.data.join("jobs/j1")).is_empty());
    assert_eq!(fake.sids(&t.tmp()), [SID_A]);
    // At start nothing is live but the static grants.
    assert_eq!(b.sweep(&HashSet::new()).unwrap(), 1);
    assert_eq!(owners(&b), [STATIC_OWNER]);
}

#[test]
fn protected_roots_are_refused_before_the_log() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open(fake.clone(), &[]);
    let wal = t.wal_text();
    let d = &t.data;
    let refused: Vec<(Grant, &str)> = vec![
        (grant(d, SID_A, READ, "session:s1"), "PATH_NOT_ALLOWED"),
        (
            grant(&t.root, SID_A, READ, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&d.join("auth/local.key"), SID_A, READ, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&d.join("run"), SID_A, READ, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&d.join("agents"), SID_A, READ, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&d.join("agents/s1"), SID_A, READ, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&d.join("agents/s1/blobs"), SID_A, READ, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&d.join("agents/s1/settings"), SID_A, MODIFY, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(
                &d.join("agents/s1/credential.json"),
                SID_A,
                MODIFY,
                "session:s1",
            ),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&d.join("jobs"), SID_A, MODIFY, "job:j1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&d.join("plugins/store/p1"), SID_A, MODIFY, "call:c1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&t.project, SID_A, MODIFY, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&t.project.join(".finisher"), SID_A, MODIFY, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(&t.project.join("assets"), SID_A, MODIFY, "session:s1"),
            "PATH_NOT_ALLOWED",
        ),
        (
            grant(
                &t.project.join("finisher.project.json"),
                SID_A,
                MODIFY,
                "session:s1",
            ),
            "PATH_NOT_ALLOWED",
        ),
        // WRITE_DAC; GENERIC_ALL.
        (
            grant(&t.tmp(), SID_A, MODIFY | 0x0004_0000, "session:s1"),
            "INVALID_ARGS",
        ),
        (
            grant(&t.tmp(), SID_A, 0x1000_0000, "session:s1"),
            "INVALID_ARGS",
        ),
        (
            grant(&t.tmp(), "S-1-x", MODIFY, "session:s1"),
            "INVALID_ARGS",
        ),
        (grant(&t.tmp(), SID_A, MODIFY, "session s1"), "INVALID_ARGS"),
        (
            grant(&t.tmp().join(".."), SID_A, MODIFY, "session:s1"),
            "INVALID_ARGS",
        ),
        (
            grant(&t.tmp().join("missing"), SID_A, MODIFY, "session:s1"),
            "NOT_FOUND",
        ),
    ];
    for (g, code) in &refused {
        let e = b
            .grant(g)
            .expect_err(&format!("{} is refused", g.path.display()));
        assert_eq!(e.code(), *code, "{}: {e}", g.path.display());
    }
    // A link into server data is refused by its canonical form.
    #[cfg(unix)]
    {
        let link = t.project.join("workspace/s1/link");
        std::os::unix::fs::symlink(d.join("auth"), &link).unwrap();
        let e = b
            .grant(&grant(&link, SID_A, READ, "session:s1"))
            .unwrap_err();
        assert_eq!(e.code(), "PATH_NOT_ALLOWED", "{e}");
    }
    assert_eq!(t.wal_text(), wal, "nothing was logged");
    assert!(
        fake.read().values().all(Vec::is_empty),
        "nothing was applied"
    );
    // What the profiles receive is granted.
    for g in [
        grant(&t.tmp(), SID_A, MODIFY, "session:s1"),
        grant(&d.join("agents/s1/provider"), SID_A, MODIFY, "session:s1"),
        grant(&d.join("agents/s1/settings"), SID_A, READ, "session:s1"),
        grant(
            &d.join("agents/s1/credential.json"),
            SID_A,
            READ,
            "session:s1",
        ),
        grant(&d.join("jobs/j1"), SID_A, MODIFY, "job:j1"),
        grant(&d.join("plugins/store/p1"), SID_A, READ_EXECUTE, "call:c1"),
        grant(&t.project, SID_B, READ_EXECUTE, STATIC_OWNER),
        grant(&t.project.join("workspace/s1"), SID_A, MODIFY, "session:s1"),
        grant(&t.project.join("sources"), SID_A, READ, "session:s1"),
    ] {
        b.grant(&g)
            .unwrap_or_else(|e| panic!("{}: {e}", g.path.display()));
    }
}

#[test]
fn one_owner_per_pair_and_a_covered_grant_is_skipped() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open(fake, &[]);
    b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
        .unwrap();
    let lines = b.wal_lines();
    b.grant(&grant(&t.tmp(), SID_A, READ, "session:s1"))
        .unwrap();
    assert_eq!(b.wal_lines(), lines, "already covered: nothing logged");
    let e = b
        .grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s2"))
        .unwrap_err();
    assert_eq!(e.code(), "INVALID_OP", "{e}");
}

#[test]
fn a_failed_grant_is_rolled_back() {
    let t = Tree::new();
    let mut fake = t.fake();
    fake.fail_grant = true;
    let b = t.open(fake.clone(), &[]);
    let e = b
        .grant(&grant(&t.tmp(), SID_A, MODIFY, STATIC_OWNER))
        .unwrap_err();
    assert_eq!(e.code(), "IO_ERROR");
    assert!(b.grants().is_empty());
    drop(b);
    // The rolled-back grant is not completed at the next open, even for a live owner.
    fake.fail_grant = false;
    let b = t.open(fake.clone(), &[]);
    assert!(b.grants().is_empty(), "{:?}", b.grants());
    assert!(fake.sids(&t.tmp()).is_empty());
}

/// The crash points: a child of this binary runs one operation with `FIN_TEST_ACL_CRASH`
/// set and is aborted there; the parent opens the broker again.
#[cfg(feature = "test-hooks")]
mod crash {
    use super::*;

    /// Runs `op` (`grant` or `revoke`) in a child aborted at `point`.
    fn child(t: &Tree, op: &str, point: &str) {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "crash::crash_child",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("FIN_TEST_ACL_CRASH", format!("{op}:{point}"))
            .env(
                "FIN_ACL_CHILD",
                format!("{op}\n{}\n{}", t.data.display(), t.root.display()),
            )
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "the child was aborted at {op}:{point}: {out:?}"
        );
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("CRASH_CHILD_DONE"),
            "the child stopped before the end"
        );
    }

    fn pending(t: &Tree) -> usize {
        let text = t.wal_text();
        let ops = text.lines().filter(|l| !l.contains("\"done\"")).count();
        ops - text.lines().filter(|l| l.contains("\"done\"")).count()
    }

    #[test]
    fn a_crash_after_the_grant_is_logged_is_completed() {
        for live in [false, true] {
            let t = Tree::new();
            let fake = t.fake();
            child(&t, "grant", "afterAppend");
            assert_eq!(pending(&t), 1, "logged, not done:\n{}", t.wal_text());
            assert!(fake.sids(&t.tmp()).is_empty(), "never applied");
            let b = t.open(fake.clone(), if live { &["session:s1"] } else { &[] });
            assert_eq!(pending(&t), 0, "{}", t.wal_text());
            if live {
                assert_eq!(owners(&b), ["session:s1"]);
                assert_eq!(fake.sids(&t.tmp()), [SID_A]);
            } else {
                assert!(b.grants().is_empty());
                assert!(fake.sids(&t.tmp()).is_empty());
            }
        }
    }

    #[test]
    fn a_crash_after_the_grant_is_applied_is_completed() {
        for live in [false, true] {
            let t = Tree::new();
            let fake = t.fake();
            child(&t, "grant", "afterApply");
            assert_eq!(pending(&t), 1);
            assert_eq!(fake.sids(&t.tmp()), [SID_A], "applied, not marked done");
            let b = t.open(fake.clone(), if live { &["session:s1"] } else { &[] });
            assert_eq!(pending(&t), 0);
            assert_eq!(b.grants().len(), usize::from(live));
            assert_eq!(
                fake.sids(&t.tmp()).len(),
                usize::from(live),
                "a dead owner's grant is revoked"
            );
        }
    }

    #[test]
    fn a_crash_during_a_revoke_is_completed() {
        for point in ["afterAppend", "afterApply"] {
            let t = Tree::new();
            let fake = t.fake();
            drop({
                let b = t.open(fake.clone(), &[]);
                b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
                    .unwrap();
                b
            });
            child(&t, "revoke", point);
            assert_eq!(
                fake.sids(&t.tmp()).len(),
                usize::from(point == "afterAppend")
            );
            // Completed even though the owner is live: the revoke was asked for.
            let b = t.open(fake.clone(), &["session:s1"]);
            assert!(b.grants().is_empty(), "{point}");
            assert!(fake.sids(&t.tmp()).is_empty(), "{point}");
            assert_eq!(pending(&t), 0);
        }
    }

    /// The child of the crash tests (`FIN_ACL_CHILD`: the op, server data, the fake's dir).
    #[test]
    #[ignore = "run by the crash tests"]
    fn crash_child() {
        let Ok(spec) = std::env::var("FIN_ACL_CHILD") else {
            return;
        };
        let mut it = spec.split('\n');
        let (op, data, root) = (
            it.next().unwrap(),
            PathBuf::from(it.next().unwrap()),
            PathBuf::from(it.next().unwrap()),
        );
        let b = Broker::open(&data, FileFake::new(&root), Options::default(), &|_| true).unwrap();
        let tmp = data.join("agents/s1/tmp");
        match op {
            "grant" => b.grant(&grant(&tmp, SID_A, MODIFY, "session:s1")).unwrap(),
            _ => {
                b.revoke("session:s1", None).unwrap();
            }
        }
        println!("CRASH_CHILD_DONE");
    }
}

#[cfg(windows)]
#[path = "acl_broker/windows.rs"]
mod windows;
