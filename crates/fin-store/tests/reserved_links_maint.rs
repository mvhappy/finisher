//! Maintenance never reaches through a link in place of `.finisher/` or a directory reserved in
//! it (sub-project 4 spec §3.6.1 "Never followed"): compaction writes and deletes, the lease
//! (acquisition, takeover, heartbeat). The scan and export cases are in `reserved_links.rs`.
#![cfg(unix)]

mod integration_support;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fin_store::lease::record;
use fin_store::lease::{FakeClock, Lease, LeaseState, Params};
use fin_store::storage_kind::StorageKind;
use fin_store::{MAIN, Store, fail_link_checks_for_test};
use integration_support::*;
use tempfile::TempDir;

const GRACE: Duration = Duration::from_secs(5);

/// Moves `path` (made empty first if missing) to an isolated directory and links it back;
/// returns the moved directory.
fn link_out(path: &Path) -> (TempDir, PathBuf) {
    fs::create_dir_all(path).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let moved = outside.path().join("moved");
    fs::rename(path, &moved).unwrap();
    symlink(&moved, path).unwrap();
    (outside, moved)
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// A holder with `n` exported one-record segments and a short compaction grace.
fn holder(e: &Env, n: usize) -> Store {
    let mut a = e.create("a");
    a.set_compaction_grace_for_test(GRACE);
    for i in 0..n {
        let id = format!("t{i}");
        commit(&mut a, &id, vec![asset(&format!("/m/{id}.mov"))]);
        assert_eq!(e.flush(&mut a), 1);
    }
    a
}

#[test]
fn compaction_never_writes_through_a_linked_snapshot_dir() {
    let e = env();
    let mut a = holder(&e, 4);
    let (_out, moved) = link_out(&e.dir.join(".finisher/snapshots").join(MAIN));
    let r = a.compact(true).unwrap();
    assert!(r.manifests.is_empty(), "{r:?}");
    assert!(
        names(&moved).is_empty(),
        "a snapshot landed beyond the link"
    );
    assert!(names(&e.dir.join(".finisher/compaction")).is_empty());
}

#[test]
fn compaction_never_deletes_through_a_linked_segments_dir() {
    let e = env();
    let mut a = holder(&e, 4);
    a.compact(true).unwrap();
    let before = segments(&e.dir).len();
    assert_eq!(before, 5, "four segments and their pack");
    e.advance(GRACE.as_secs() + 1);
    // After the first delete, the branch directory is moved out and linked back.
    let moved: Arc<Mutex<Option<(TempDir, PathBuf)>>> = Arc::default();
    let branch = e.dir.join(".finisher/segments").join(MAIN);
    let slot = moved.clone();
    a.on_compaction_delete_for_test(Box::new(move |_| {
        let mut slot = slot.lock().unwrap();
        if slot.is_none() {
            *slot = Some(link_out(&branch));
        }
    }));
    let r = a.compact(false).unwrap();
    assert!(r.finished.is_empty(), "{r:?}");
    let moved = moved.lock().unwrap().take().expect("one delete happened");
    assert_eq!(names(&moved.1).len(), before - 1, "deleted beyond the link");
    assert_eq!(names(&e.dir.join(".finisher/compaction")).len(), 1);
}

#[test]
fn the_store_never_takes_the_lease_through_a_linked_finisher() {
    let e = env();
    let a = holder(&e, 1);
    a.close().unwrap();
    let (_out, moved) = link_out(&e.dir.join(".finisher"));
    let files = names(&moved);
    let lease = fs::read(moved.join(record::FILE)).unwrap();
    let mut b = e.open("b");
    assert_eq!(held_epoch(&b), None);
    // Long past staleness: still not acquired, and a confirmed takeover is refused.
    e.advance(60);
    e.poll(&mut b);
    assert_eq!(held_epoch(&b), None);
    let refused = fin_core::CoreError::from(b.takeover(true).unwrap_err());
    assert_eq!(refused.code, "LEASE_TAKEOVER_REFUSED", "{refused:?}");
    assert_eq!(names(&moved), files, "nothing written beyond the link");
    assert_eq!(fs::read(moved.join(record::FILE)).unwrap(), lease);
}

fn lease_at(dir: &Path, clock: &Arc<FakeClock>) -> Lease {
    let p = Params {
        clock: clock.clone(),
        poll: Duration::from_millis(2),
        ..Params::for_kind(StorageKind::Local)
    };
    Lease::acquire(dir, "servera", "holder a", p).unwrap()
}

fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for {what}");
}

fn heartbeat_in(dir: &Path) -> u64 {
    record::decode(&fs::read(dir.join(record::FILE)).unwrap())
        .unwrap()
        .heartbeat
}

#[test]
fn a_heartbeat_loses_the_lease_on_a_linked_finisher() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".finisher");
    fs::create_dir(&dir).unwrap();
    let c = FakeClock::new();
    let a = lease_at(&dir, &c);
    let (_out, moved) = link_out(&dir);
    c.advance(Duration::from_secs(5));
    eventually("the lease is lost", || {
        matches!(a.state(), LeaseState::Lost { .. })
    });
    let LeaseState::Lost { reason } = a.state() else {
        unreachable!()
    };
    assert!(reason.contains("link"), "{reason}");
    assert_eq!(heartbeat_in(&moved), 0, "no heartbeat beyond the link");
}

#[test]
fn a_failed_link_check_is_a_failed_beat_not_a_lost_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".finisher");
    fs::create_dir(&dir).unwrap();
    let c = FakeClock::new();
    let a = lease_at(&dir, &c);
    fail_link_checks_for_test(&dir, true);
    c.advance(Duration::from_secs(5));
    eventually("the failed beat", || a.failed_heartbeats() >= 1);
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    fail_link_checks_for_test(&dir, false);
    c.advance(Duration::from_secs(1));
    eventually("the retry", || a.heartbeats() >= 1);
    assert_eq!(a.check(), Ok(1));
    assert!(heartbeat_in(&dir) >= 1);
}
