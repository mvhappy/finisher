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
    /// Paths whose revoke fails (shared by the clones, so a test can change it later).
    fail_revoke: Arc<Mutex<Vec<PathBuf>>>,
}

impl FileFake {
    fn new(dir: &Path) -> FileFake {
        FileFake {
            file: dir.join("fake-acl.json"),
            delay: Duration::ZERO,
            fail_grant: false,
            fail_revoke: Arc::default(),
        }
    }

    fn fail_revoke_on(&self, paths: &[&Path]) {
        *self.fail_revoke.lock().unwrap() = paths.iter().map(|p| p.to_path_buf()).collect();
    }

    /// Removes every ACE of `p` behind the broker's back (a file replaced by rename).
    fn wipe(&self, p: &Path) {
        let mut all = self.read();
        all.remove(&key(p));
        self.write(&all);
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
        if self.fail_revoke.lock().unwrap().iter().any(|p| p == path) {
            return Err(io::Error::other("injected revoke failure"));
        }
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

    fn holds(&self, path: &Path, sid: &str, mask: u32, _inherit: bool) -> io::Result<bool> {
        Ok(self
            .read()
            .get(&key(path))
            .is_some_and(|a| a.iter().any(|(s, m, _)| s == sid && m & mask == mask)))
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
        open_retrying(&self.data, fake, opts, &|o| live.contains(o)).expect("the broker opens")
    }

    fn tmp(&self) -> PathBuf {
        self.data.join("agents/s1/tmp")
    }
}

/// [`Broker::open`], retried while the lock of a broker this process just dropped is still
/// held: on Unix the lock (`flock`) belongs to the open file, which a child forked by another
/// test at that moment holds until its `exec` closes it. A server never reopens its broker.
fn open_retrying<A: Apply + Clone>(
    data: &Path,
    apply: A,
    opts: Options,
    live: &dyn Fn(&str) -> bool,
) -> Result<Broker<A>, BrokerError> {
    for _ in 0..200 {
        match Broker::open(data, apply.clone(), opts.clone(), live) {
            Err(BrokerError::Io(m)) if m.contains("another ACL broker") => {
                std::thread::sleep(Duration::from_millis(10));
            }
            r => return r,
        }
    }
    Broker::open(data, apply, opts, live)
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

/// `<p>/..`, its `..` kept as written.
fn dotdot(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(std::path::MAIN_SEPARATOR_STR);
    s.push("..");
    PathBuf::from(s)
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
    // A static record is not carried across a restart (the next launch grants it again); its
    // ACE stays.
    let b = t.open(fake.clone(), &["session:s1"]);
    assert_eq!(owners(&b), ["session:s1"]);
    assert_eq!(fake.sids(&t.project), [SID_B]);
    assert!(!t.wal_text().contains(STATIC_OWNER), "{}", t.wal_text());
    // Revoked by owner: the ACE goes, the log says so after a reopen.
    assert_eq!(b.revoke("session:s1", None).unwrap(), 1);
    assert!(fake.sids(&t.tmp()).is_empty());
    drop(b);
    assert!(owners(&t.open(fake, &["session:s1"])).is_empty());
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
    assert!(b.warnings().is_empty(), "{:?}", b.warnings());
    drop(b);
    assert!(owners(&t.open(fake, &[])).is_empty());
}

#[test]
fn a_torn_last_line_is_dropped_and_a_damaged_one_refused() {
    let t = Tree::new();
    let fake = t.fake();
    drop(t.open(fake.clone(), &[]));
    let g = serde_json::json!({"op": "grant", "id": 1, "path": t.tmp(), "sid": SID_A, "mask": MODIFY, "inherit": true, "owner": "session:s1"});
    let done = r#"{"op":"done","id":1}"#;
    std::fs::write(t.wal(), format!("{g}\n{done}\n{{\"op\":\"gra")).unwrap();
    let b = t.open(fake.clone(), &["session:s1"]);
    assert_eq!(owners(&b), ["session:s1"]);
    drop(b);
    // Rewritten at open: the torn tail is gone.
    assert_eq!(t.wal_text().lines().count(), 2, "{}", t.wal_text());
    std::fs::write(t.wal(), format!("{g}\nnot json\n{done}\n")).unwrap();
    let r = open_retrying(&t.data, fake, Options::default(), &|_| true);
    assert!(
        matches!(&r, Err(BrokerError::Io(m)) if m.contains("damaged")),
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
    // Through the broker, both are kept.
    let b = Arc::new(t.open(fake.clone(), &[]));
    let barrier = Arc::new(Barrier::new(2));
    let job = t.data.join("jobs/j1");
    let both: Vec<_> = [SID_A, SID_B]
        .into_iter()
        .enumerate()
        .map(|(n, sid)| {
            let (b, w, p) = (b.clone(), barrier.clone(), job.clone());
            std::thread::spawn(move || {
                w.wait();
                b.grant(&grant(&p, sid, MODIFY, &format!("job:{n}")))
                    .unwrap();
            })
        })
        .collect();
    both.into_iter().for_each(|h| h.join().unwrap());
    assert_eq!(fake.sids(&job), [SID_A, SID_B]);
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
    b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
        .unwrap();
    b.grant(&grant(&t.data.join("jobs/j1"), SID_B, MODIFY, "job:b"))
        .unwrap();
    b.grant(&grant(&t.project, SID_B, READ_EXECUTE, STATIC_OWNER))
        .unwrap();
    let live: HashSet<String> = ["session:s1".to_string()].into();
    assert_eq!(b.sweep(&live).revoked, 1);
    assert_eq!(owners(&b), ["session:s1", STATIC_OWNER]);
    assert!(fake.sids(&t.data.join("jobs/j1")).is_empty());
    assert_eq!(fake.sids(&t.tmp()), [SID_A]);
    // At start nothing is live but the static grants.
    assert_eq!(b.sweep(&HashSet::new()).revoked, 1);
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
        // Spelled out: `join("..")` on a `\\?\` path (Windows) pops a component instead.
        (
            grant(&dotdot(&t.tmp()), SID_A, MODIFY, "session:s1"),
            "INVALID_ARGS",
        ),
        // Another session's files, whatever the mask.
        (
            grant(&t.tmp(), SID_A, READ, "session:s2"),
            "PATH_NOT_ALLOWED",
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
    let job = t.data.join("jobs/j1");
    b.grant(&grant(&job, SID_A, MODIFY, "job:a")).unwrap();
    let e = b.grant(&grant(&job, SID_A, MODIFY, "job:b")).unwrap_err();
    assert_eq!(e.code(), "INVALID_OP", "{e}");
}

#[test]
fn a_failed_grant_is_rolled_back() {
    let t = Tree::new();
    let mut fake = t.fake();
    fake.fail_grant = true;
    let b = t.open(fake.clone(), &[]);
    let e = b
        .grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
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

#[test]
fn a_pending_revoke_never_wipes_a_newer_grant_of_its_pair() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open(fake.clone(), &[]);
    let g = grant(&t.tmp(), SID_A, MODIFY, "session:s1");
    b.grant(&g).unwrap();
    fake.fail_revoke_on(&[&t.tmp()]);
    assert_eq!(b.revoke("session:s1", None).unwrap_err().code(), "IO_ERROR");
    assert_eq!(
        fake.sids(&t.tmp()),
        [SID_A],
        "the failed revoke left the ACE"
    );
    assert_eq!(b.pending(), 1, "and stays pending");
    // The session grants its pair again: the pending revoke is completed first, then granted.
    fake.fail_revoke_on(&[]);
    b.grant(&g).unwrap();
    assert_eq!(b.pending(), 0);
    let live: HashSet<String> = ["session:s1".to_string()].into();
    assert!(b.sweep(&live).failed.is_empty());
    assert_eq!(
        fake.sids(&t.tmp()),
        [SID_A],
        "the sweep left the newer grant"
    );
    drop(b);
    let b = t.open(fake.clone(), &["session:s1"]);
    assert_eq!(owners(&b), ["session:s1"]);
    assert_eq!(fake.sids(&t.tmp()), [SID_A], "and so did the replay");
}

#[test]
fn a_replayed_revoke_followed_by_a_grant_of_its_pair_grants_it_again() {
    // The log of a broker that did not complete its pending revoke before granting again.
    let t = Tree::new();
    let fake = t.fake();
    drop(t.open(fake.clone(), &[]));
    let tmp = t.tmp();
    let lines = [
        serde_json::json!({"op": "grant", "id": 1, "path": tmp, "sid": SID_A, "mask": MODIFY, "inherit": true, "owner": "session:s1"}),
        serde_json::json!({"op": "done", "id": 1}),
        serde_json::json!({"op": "revoke", "id": 2, "path": tmp, "sid": SID_A, "owner": "session:s1"}),
        serde_json::json!({"op": "grant", "id": 3, "path": tmp, "sid": SID_A, "mask": MODIFY, "inherit": true, "owner": "session:s1"}),
        serde_json::json!({"op": "done", "id": 3}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(t.wal(), text).unwrap();
    FileFake::grant(&fake, &tmp, SID_A, MODIFY, true).unwrap();
    let b = t.open(fake.clone(), &["session:s1"]);
    assert_eq!(owners(&b), ["session:s1"]);
    assert_eq!(b.pending(), 0);
    assert_eq!(
        fake.sids(&tmp),
        [SID_A],
        "the revoke did not leave the grant without its ACE"
    );
}

#[test]
fn a_revoke_that_fails_stays_pending_and_the_others_go_on() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open_with(fake.clone(), &[], Options { compact_slack: 0 });
    let job = t.data.join("jobs/j1");
    b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
        .unwrap();
    b.grant(&grant(
        &t.project.join("workspace/s1"),
        SID_A,
        MODIFY,
        "session:s1",
    ))
    .unwrap();
    b.grant(&grant(&job, SID_B, MODIFY, "job:j1")).unwrap();
    // `revoke` tries every grant of the owner.
    fake.fail_revoke_on(&[&t.tmp()]);
    assert!(b.revoke("session:s1", None).is_err());
    assert!(fake.sids(&t.project.join("workspace/s1")).is_empty());
    assert_eq!(b.pending(), 1);
    // The sweep retries it, fails again, and still revokes the job's.
    let r = b.sweep(&HashSet::new());
    assert_eq!((r.revoked, r.failed.len()), (1, 1), "{r:?}");
    assert!(fake.sids(&job).is_empty());
    assert_eq!(fake.sids(&t.tmp()), [SID_A]);
    // Compaction (slack 0: at every operation) keeps it; so does a restart.
    drop(b);
    let b = t.open_with(fake.clone(), &[], Options { compact_slack: 0 });
    assert_eq!(b.pending(), 1, "{}", t.wal_text());
    assert!(!b.warnings().is_empty(), "the failure at open is a warning");
    fake.fail_revoke_on(&[]);
    let r = b.sweep(&HashSet::new());
    assert!(r.failed.is_empty(), "{r:?}");
    assert!(fake.sids(&t.tmp()).is_empty());
    assert_eq!(b.pending(), 0);
}

#[test]
fn a_covered_static_grant_is_applied_again_when_its_ace_is_gone() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open(fake.clone(), &[]);
    let g = grant(&t.project, SID_B, READ_EXECUTE, STATIC_OWNER);
    b.grant(&g).unwrap();
    let lines = b.wal_lines();
    b.grant(&g).unwrap();
    assert_eq!(b.wal_lines(), lines, "held: skipped");
    // Replaced by rename: the explicit ACE is gone, the record is not.
    fake.wipe(&t.project);
    b.grant(&g).unwrap();
    assert_eq!(fake.sids(&t.project), [SID_B], "granted again");
}

#[test]
fn a_path_is_checked_locked_recorded_and_applied_in_its_resolved_form() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open(fake.clone(), &[]);
    // A spelling with a redundant separator and, on Unix, a link: one record, the resolved path.
    let mut spelled = t.tmp().into_os_string();
    spelled.push(std::path::MAIN_SEPARATOR_STR);
    b.grant(&grant(&PathBuf::from(spelled), SID_A, MODIFY, "session:s1"))
        .unwrap();
    #[cfg(unix)]
    {
        let link = t.root.join("tmp-link");
        std::os::unix::fs::symlink(t.tmp(), &link).unwrap();
        b.grant(&grant(&link, SID_A, MODIFY, "session:s1")).unwrap();
        // Through a link into another session's files: refused by the resolved form.
        std::fs::create_dir_all(t.data.join("agents/s2/tmp")).unwrap();
        let other = t.root.join("other-link");
        std::os::unix::fs::symlink(t.data.join("agents/s2/tmp"), &other).unwrap();
        let e = b
            .grant(&grant(&other, SID_A, MODIFY, "session:s1"))
            .unwrap_err();
        assert_eq!(e.code(), "PATH_NOT_ALLOWED", "{e}");
    }
    let paths: Vec<PathBuf> = b.grants().into_iter().map(|g| g.path).collect();
    assert_eq!(paths, [t.tmp()]);
    assert_eq!(fake.sids(&t.tmp()), [SID_A]);
    assert_eq!(b.revoke("session:s1", None).unwrap(), 1);
}

#[cfg(unix)]
#[test]
fn a_path_that_cannot_be_resolved_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let t = Tree::new();
    let b = t.open(t.fake(), &[]);
    let locked = t.project.join("workspace/s1/locked");
    std::fs::create_dir_all(locked.join("inner")).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let r = b.grant(&grant(&locked.join("inner"), SID_A, READ, "session:s1"));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Root reads through 0o000 (a CI container): nothing to refuse there.
    if let Err(e) = r {
        assert_eq!(e.code(), "PATH_NOT_ALLOWED", "{e}");
    }
}

#[test]
fn a_failed_rewrite_of_the_log_is_a_warning() {
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open_with(fake.clone(), &[], Options { compact_slack: 0 });
    // The temp file cannot be made: a directory holds its name.
    std::fs::create_dir_all(t.data.join("run/acl-broker.wal.tmp/x")).unwrap();
    for _ in 0..3 {
        b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
            .unwrap();
        b.revoke("session:s1", None).unwrap();
    }
    assert!(!b.warnings().is_empty());
    // The log was kept, and appended to, in place.
    assert_eq!(t.wal_text().lines().count(), b.wal_lines());
    b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
        .unwrap();
    drop(b);
    std::fs::remove_dir_all(t.data.join("run/acl-broker.wal.tmp")).unwrap();
    assert_eq!(owners(&t.open(fake, &["session:s1"])), ["session:s1"]);
}

#[test]
fn one_broker_per_server_data() {
    let t = Tree::new();
    let b = t.open(t.fake(), &[]);
    let second = Broker::open(&t.data, t.fake(), Options::default(), &|_| true);
    assert!(
        matches!(second, Err(BrokerError::Io(_))),
        "refused while one is open"
    );
    drop(b);
    drop(t.open(t.fake(), &[]));
}

/// The real grant tables (`fin_sandbox::windows::grants`), built from realistic inputs with
/// this OS's toolchain list, pass the broker's protect rules (Task 10 review B1): a table that
/// names what the rules refuse fails here, not at a launch.
#[test]
fn the_real_grant_tables_pass_the_protect_rules() {
    use fin_core::events::payloads::agent::{AgentMode, Harness};
    use fin_sandbox::windows::grants::{self, Scope};
    use fin_sandbox::{AgentInput, HookInput, inputs::FinEndpoint, inputs::HookFin};
    let t = Tree::new();
    let fake = t.fake();
    let b = t.open(fake, &[]);
    let d = &t.data;
    for p in ["agents/s1/localappdata", "providers/codex/home", "hooks/c1"] {
        std::fs::create_dir_all(d.join(p)).unwrap();
    }
    std::fs::write(d.join("providers/codex/home/auth.json"), "{}").unwrap();
    std::fs::write(d.join("hooks/c1.cred"), "{}").unwrap();
    std::fs::create_dir_all(t.project.join("workspace/shared")).unwrap();
    let hook_file = t.root.join("hooks/h.cmd");
    std::fs::create_dir_all(hook_file.parent().unwrap()).unwrap();
    std::fs::write(&hook_file, "x").unwrap();
    let lib = t.root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let home = t.root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cli = t.root.join("cli");
    std::fs::create_dir_all(&cli).unwrap();
    let mut toolchains: Vec<PathBuf> = system_toolchains();
    toolchains.push(cli.clone());
    let agent = AgentInput {
        session_id: "s1".into(),
        mode: AgentMode::Draft,
        harness: Harness::Codex,
        project_dir: t.project.clone(),
        workspace_dir: t.project.join("workspace/s1"),
        shared_dir: Some(t.project.join("workspace/shared")),
        library_roots: vec![lib],
        session_dir: d.join("agents/s1"),
        sign_in_home: None,
        sign_in_store: Some(d.join("providers/codex/home/auth.json")),
        toolchains: toolchains.clone(),
        server_data: d.clone(),
        home: home.clone(),
        fin: None,
    };
    let hook = HookInput {
        project_dir: t.project.clone(),
        hook_file,
        toolchains,
        tmp: d.join("hooks/c1"),
        server_data: d.clone(),
        home,
        fin: Some(HookFin {
            credential: d.join("hooks/c1.cred"),
            endpoint: FinEndpoint::Pipe,
        }),
        timeout_ms: None,
    };
    let tables = [
        (
            "session:s1",
            grants::agent_with_extra(&agent, &[cli]).unwrap(),
        ),
        ("call:c1", grants::hook(&hook).unwrap()),
    ];
    let mut checked = 0;
    for (owner, table) in tables {
        assert!(!table.is_empty());
        for e in table {
            let sid = match &e.trustee {
                grants::Trustee::Sid(s) => s.clone(),
                grants::Trustee::Container(_) => SID_B.into(),
            };
            let owner = if e.scope == Scope::Static {
                STATIC_OWNER
            } else {
                owner
            };
            b.grant(&Grant {
                path: e.path.clone(),
                sid,
                mask: e.mask,
                inherit: true,
                owner: owner.into(),
            })
            .unwrap_or_else(|err| panic!("{} for {owner}: {err}", e.path.display()));
            checked += 1;
        }
    }
    assert!(checked >= 15, "{checked}");
}

/// This OS's base toolchain roots, as `finisherd` passes them (`sandboxToolchainBase`).
fn system_toolchains() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        fin_sandbox::data::toolchain_base_resolved(fin_sandbox::Os::current())
    }
    #[cfg(windows)]
    {
        fin_sandbox::data::toolchain_base_present()
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
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

    #[test]
    fn a_crash_while_the_log_is_rewritten_loses_nothing() {
        for point in ["beforeRename", "afterRename"] {
            let t = Tree::new();
            let fake = t.fake();
            drop({
                let b = t.open(fake.clone(), &[]);
                b.grant(&grant(&t.tmp(), SID_A, MODIFY, "session:s1"))
                    .unwrap();
                b
            });
            // The child's open rewrites the log and is aborted around the rename.
            child(&t, "compact", point);
            let tmp = t.data.join("run/acl-broker.wal.tmp");
            assert_eq!(tmp.exists(), point == "beforeRename", "{point}");
            let b = t.open(fake.clone(), &["session:s1"]);
            assert_eq!(owners(&b), ["session:s1"], "{point}: {}", t.wal_text());
            assert_eq!(b.pending(), 0, "{point}");
            assert_eq!(fake.sids(&t.tmp()), [SID_A], "{point}");
            assert!(
                !tmp.exists(),
                "{point}: the stale temp file was replaced and renamed"
            );
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
        let b = open_retrying(&data, FileFake::new(&root), Options::default(), &|_| true).unwrap();
        let tmp = data.join("agents/s1/tmp");
        match op {
            "grant" => b.grant(&grant(&tmp, SID_A, MODIFY, "session:s1")).unwrap(),
            // The open above rewrote the log.
            "compact" => {}
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

// The AppContainer children of `windows::pipe_allow_admits_the_projects_container_and_no_other`,
// at the crate root: the container runs this binary as `<exe> <test> --exact`, so a test in a
// module would not be found and the child would exit 0 without running.

/// In the container: opens the pipe named by `FIN_PIPE_PROBE` as `fin` does (its own
/// project's: the allowed case).
#[test]
#[ignore = "the AppContainer child of pipe_allow_admits_the_projects_container_and_no_other"]
#[cfg(windows)]
fn appcontainer_child() {
    let Ok(name) = std::env::var("FIN_PIPE_PROBE") else {
        return;
    };
    let code = match support::open(&name, support::CLIENT_ACCESS) {
        Ok(mut c) => {
            let _ = std::io::Write::write_all(&mut c, b"hi\n");
            support::CHILD_CONNECTED
        }
        Err(e) if e.raw_os_error() == Some(5) => support::CHILD_DENIED,
        Err(_) => support::CHILD_OTHER,
    };
    std::process::exit(code as i32);
}

/// In the container: waits to be killed, so the parent can read its token. Opens nothing.
#[test]
#[ignore = "the waiting AppContainer child of pipe_allow_admits_the_projects_container_and_no_other"]
#[cfg(windows)]
fn waiting_child() {
    if std::env::var_os("FIN_ACL_WAIT").is_some() {
        std::thread::sleep(Duration::from_secs(60));
    }
}
