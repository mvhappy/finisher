//! A clean close releases the lease (`Lease::release`, spec §3.5 "Release"): the holder's
//! last write marks `lease.json` released, and another server takes it at once, at a fresh
//! epoch through a claim. Only the current holder can release; a crash (a drop) releases
//! nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fin_store::lease::record::{self, LeaseRecord};
use fin_store::lease::{Contender, FakeClock, Lease, LeaseState, Params};
use fin_store::storage_kind::StorageKind;
use tempfile::TempDir;

const A: &str = "servera";
const B: &str = "serverb";
const C: &str = "serverc";

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn folder() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".finisher");
    fs::create_dir(&dir).unwrap();
    (tmp, dir)
}

fn params(kind: StorageKind, clock: &Arc<FakeClock>) -> Params {
    Params {
        clock: clock.clone(),
        poll: Duration::from_millis(2),
        ..Params::for_kind(kind)
    }
}

fn local(clock: &Arc<FakeClock>) -> Params {
    params(StorageKind::Local, clock)
}

fn contender(dir: &Path, id: &str, p: Params) -> Contender {
    Contender::new(dir, id, &format!("holder {id}"), p)
}

fn acquire(dir: &Path, id: &str, p: Params) -> Lease {
    Lease::acquire(dir, id, &format!("holder {id}"), p).unwrap()
}

fn read_record(dir: &Path) -> LeaseRecord {
    record::decode(&fs::read(dir.join("lease.json")).unwrap()).expect("a valid record")
}

fn held_elsewhere(r: Result<Lease, LeaseState>) {
    match r {
        Err(LeaseState::HeldElsewhere { .. }) => {}
        Err(other) => panic!("expected HeldElsewhere, got {other:?}"),
        Ok(l) => panic!("expected HeldElsewhere, acquired epoch {}", l.epoch()),
    }
}

#[test]
fn a_released_lease_is_taken_at_once() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let mut b = contender(&dir, B, local(&c));
    held_elsewhere(b.try_acquire());
    assert!(a.release(), "released");
    let r = read_record(&dir);
    assert!(r.released, "{r:?}");
    assert_eq!((r.server_id.as_str(), r.epoch), (A, 1));
    // No waiting out staleness: the next attempt takes it, at a fresh epoch.
    let l = b.try_acquire().unwrap();
    assert_eq!(l.epoch(), 2);
    assert_eq!(l.previous().record.as_deref(), Some(A));
    assert!(
        l.previous().other_writer_since(B, 0),
        "the ordering rule still applies"
    );
    let r = read_record(&dir);
    assert!(!r.released);
    assert_eq!((r.server_id.as_str(), r.epoch), (B, 2));
    // A held record serialises as before: no `released` field.
    let bytes = fs::read(dir.join("lease.json")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("released"));
}

#[test]
fn a_released_lease_is_taken_at_once_on_a_sync_folder_too() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let sync = || {
        params(
            StorageKind::Sync(fin_store::conflict_names::Provider::Dropbox),
            &c,
        )
    };
    let a = acquire(&dir, A, sync());
    assert!(a.release());
    assert_eq!(contender(&dir, B, sync()).try_acquire().unwrap().epoch(), 2);
}

#[test]
fn a_new_process_of_the_same_server_retakes_its_released_lease() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    assert!(acquire(&dir, A, local(&c)).release());
    fin_store::lease::forget_process_for_test(&dir);
    assert_eq!(acquire(&dir, A, local(&c)).epoch(), 2);
}

#[test]
fn a_dropped_lease_releases_nothing() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    drop(acquire(&dir, A, local(&c)));
    assert!(!read_record(&dir).released);
    let mut b = contender(&dir, B, local(&c));
    held_elsewhere(b.try_acquire());
    c.advance(s(44));
    held_elsewhere(b.try_acquire());
}

#[test]
fn a_lost_lease_never_releases() {
    // Another server's record over ours: the release writes nothing.
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let theirs = LeaseRecord::new(C, 7, 0, "holder c", 0);
    fin_store::fsio::write_in_place(&dir.join("lease.json"), &record::encode(&theirs)).unwrap();
    assert!(!a.release());
    assert_eq!(read_record(&dir), theirs, "untouched");
}

#[test]
fn a_newer_claim_blocks_the_release() {
    // An acquisition in progress (a young claim above our epoch) makes us lose, not release.
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let before = fs::read(dir.join("lease.json")).unwrap();
    fs::write(dir.join(fin_store::lease::claim::claim_name(2)), C).unwrap();
    assert!(!a.release());
    assert_eq!(fs::read(dir.join("lease.json")).unwrap(), before);
}

#[test]
fn a_self_fenced_lease_never_releases() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    a.fail_heartbeats(u32::MAX);
    c.advance(s(21));
    let before = fs::read(dir.join("lease.json")).unwrap();
    assert!(!a.release());
    assert_eq!(fs::read(dir.join("lease.json")).unwrap(), before);
    assert!(!read_record(&dir).released);
}
