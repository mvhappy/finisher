//! Lease v3 (plan Task 9, spec §3.5, parent spec §27.9): an in-place checksummed record,
//! exclusive claim files for epochs, staleness judged in server time (probe-file mtimes), and
//! a holder that self-fences by its monotonic clock.
//!
//! Every test uses a [`FakeClock`]: the monotonic clock only moves when the test advances it,
//! and the file server's clock (real mtimes) is shifted along with it, so 45 s of server time
//! pass without waiting.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use fin_core::{Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::lease::claim::{self, claim_name};
use fin_store::lease::record::{self, LeaseRecord, RECORD_SIZE};
use fin_store::lease::{
    Contender, FakeClock, Lease, LeaseState, Params, ReadFault,
    crash_before_publishing_record_for_test, deny_claim_creates_for_test,
    fault_record_reads_for_test, no_hard_links_for_test,
};
use fin_store::storage_kind::StorageKind;
use fin_store::{MAIN, Store};
use tempfile::TempDir;

const A: &str = "servera";
const B: &str = "serverb";
const C: &str = "serverc";

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A temp dir and its `.finisher` subdirectory. The lease never creates that directory: a
/// project folder that vanished must not be recreated empty.
fn folder() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".finisher");
    fs::create_dir(&dir).unwrap();
    (tmp, dir)
}

fn params(kind: StorageKind, clock: &Arc<FakeClock>) -> Params {
    Params {
        clock: clock.clone(),
        poll: ms(2),
        ..Params::for_kind(kind)
    }
}

fn local(clock: &Arc<FakeClock>) -> Params {
    params(StorageKind::Local, clock)
}

fn sync(clock: &Arc<FakeClock>) -> Params {
    params(
        StorageKind::Sync(fin_store::conflict_names::Provider::Dropbox),
        clock,
    )
}

fn acquire(dir: &Path, id: &str, p: Params) -> Lease {
    Lease::acquire(dir, id, &format!("holder {id}"), p).unwrap()
}

fn contender(dir: &Path, id: &str, p: Params) -> Contender {
    Contender::new(dir, id, &format!("holder {id}"), p)
}

/// Polls `f` in real time for up to 3 s.
fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + s(3);
    while Instant::now() < until {
        if f() {
            return;
        }
        std::thread::sleep(ms(2));
    }
    panic!("timed out waiting for {what}");
}

fn read_record(dir: &Path) -> LeaseRecord {
    record::decode(&fs::read(dir.join("lease.json")).unwrap()).expect("a valid record")
}

/// Overwrites `lease.json` in place with `r`, as another server's write would.
fn overwrite(dir: &Path, r: &LeaseRecord) {
    fin_store::fsio::write_in_place(&dir.join("lease.json"), &record::encode(r)).unwrap();
}

fn held_elsewhere(r: Result<Lease, LeaseState>) -> (String, u64, Option<i64>) {
    match r {
        Err(LeaseState::HeldElsewhere {
            holder,
            epoch,
            age_server_ms,
        }) => (holder, epoch, age_server_ms),
        Err(other) => panic!("expected HeldElsewhere, got {other:?}"),
        Ok(l) => panic!("expected HeldElsewhere, acquired epoch {}", l.epoch()),
    }
}

fn claims(dir: &Path) -> Vec<u64> {
    let mut v: Vec<u64> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| claim::parse_claim(e.unwrap().file_name().to_str()?))
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn lease_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Lease>();
    check::<Contender>();
}

#[test]
fn record_is_512_bytes_and_crc_checked() {
    let r = LeaseRecord::new(A, 7, 3, "studio-mac (finisher 0.1.0)", 1_762_000_000_123);
    let bytes = record::encode(&r);
    assert_eq!(bytes.len(), RECORD_SIZE);
    assert_eq!(RECORD_SIZE, 512);
    assert_eq!(record::decode(&bytes), Some(r.clone()));
    let text = std::str::from_utf8(&bytes).unwrap();
    let (body, rest) = text.split_once('\n').unwrap();
    assert!(
        body.starts_with(
            r#"{"magic":"finisher-lease","version":1,"serverId":"servera","epoch":7,"heartbeat":3,"#
        ),
        "{body}"
    );
    let (crc, pad) = rest.split_once('\n').unwrap();
    assert_eq!(crc, format!("{:08x}", crc32c::crc32c(body.as_bytes())));
    assert!(pad.bytes().all(|b| b == b' '));

    // Any flipped byte in the body or checksum, a short read, or litter in the padding.
    for i in 0..body.len() + 9 {
        let mut bad = bytes;
        bad[i] ^= 0x01;
        assert_eq!(record::decode(&bad), None, "byte {i}");
    }
    assert_eq!(record::decode(&bytes[..511]), None);
    let mut litter = bytes;
    litter[511] = b'x';
    assert_eq!(record::decode(&litter), None);
    let mut newer = r.clone();
    newer.version = 2;
    assert_eq!(record::decode(&record::encode(&newer)), None);

    // An overlong holder description is cut to fit.
    let long = LeaseRecord::new(A, 1, 0, &"ж".repeat(400), -5);
    let back = record::decode(&record::encode(&long)).unwrap();
    assert!(back.holder.len() < 400 && "ж".repeat(400).starts_with(&back.holder));
    assert_eq!((back.epoch, back.probe_server_ms), (1, -5));
}

#[test]
fn first_acquire_takes_epoch_1() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let l = acquire(&dir, A, local(&c));
    assert_eq!(l.epoch(), 1);
    assert_eq!(l.state(), LeaseState::Held { epoch: 1 });
    assert_eq!(l.check(), Ok(1));
    assert_eq!(fs::metadata(dir.join("lease.json")).unwrap().len(), 512);
    let r = read_record(&dir);
    assert_eq!(
        (r.server_id.as_str(), r.epoch, r.holder.as_str()),
        (A, 1, "holder servera")
    );
    assert_eq!(fs::read_to_string(dir.join(claim_name(1))).unwrap(), A);
    assert_eq!(claim_name(1), "lease.claim-00000001");
    assert!(dir.join(format!("lease.probe-{A}")).is_file());
    for e in fs::read_dir(&dir).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        assert!(fin_store::conflict_names::is_lease_name(&name), "{name}");
    }
}

#[test]
fn second_server_sees_held() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let _a = acquire(&dir, A, local(&c));
    let (holder, epoch, age) = held_elsewhere(Lease::acquire(&dir, B, "b", local(&c)));
    assert_eq!((holder.as_str(), epoch), ("holder servera", 1));
    assert!(age.is_some_and(|a| a.abs() < 2_000), "{age:?}");
    // A takeover is refused while the holder is live, confirmed or not.
    let mut b = contender(&dir, B, local(&c));
    for confirm in [false, true] {
        let e = b.takeover(confirm).unwrap_err();
        assert_eq!(e.code, "LEASE_TAKEOVER_REFUSED");
        assert_eq!(e.details["reason"], "HELD", "{e:?}");
    }
}

#[test]
fn claim_name_exclusive_gives_unique_epochs() {
    let tmp = tempfile::tempdir().unwrap();
    for round in 0..200 {
        let dir = tmp.path().join(format!("r{round}"));
        fs::create_dir(&dir).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [A, B]
            .into_iter()
            .map(|id| {
                let (dir, barrier) = (dir.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    claim::claim_next(&dir, 0, id)
                })
            })
            .collect();
        let got: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        // One wins epoch 1; the other backs off without creating anything (a claim at 2 would
        // only fence the winner).
        let won: Vec<_> = got.iter().filter_map(|r| r.as_ref().ok()).collect();
        assert_eq!(won.len(), 1, "round {round}: {got:?}");
        assert_eq!(won[0].epoch, 1);
        let lost = got.iter().find_map(|r| r.as_ref().err()).unwrap();
        assert_eq!(lost.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(claims(&dir), [1], "round {round}");
    }
    // Non-canonical spellings are not claims.
    assert_eq!(claim::parse_claim("lease.claim-00000012"), Some(12));
    assert_eq!(claim::parse_claim("lease.claim-12"), None);
    assert_eq!(claim::parse_claim("lease.claim-000000012"), None);
}

#[test]
fn missing_record_with_claim_is_held() {
    let (_t, dir) = folder();
    fs::write(dir.join(claim_name(3)), C).unwrap();
    let c = FakeClock::new();
    let mut b = contender(&dir, B, local(&c));
    let (holder, epoch, _) = held_elsewhere(b.try_acquire());
    assert_eq!((holder.as_str(), epoch), (C, 3));
    assert!(
        !dir.join("lease.json").exists(),
        "never created behind a claim"
    );
    // A claim whose acquirer never wrote the record goes stale like a record does.
    c.advance(s(46));
    held_elsewhere(b.try_acquire());
    c.advance(s(5));
    let l = b.try_acquire().unwrap();
    assert_eq!(l.epoch(), 4);
    assert_eq!(claims(&dir), [4]);
}

#[test]
fn bad_crc_is_held_on_acquire_and_lost_on_fence_at_the_deadline() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let path = dir.join("lease.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes[20] ^= 0x20;
    fin_store::fsio::write_in_place(&path, &bytes).unwrap();
    // The holder cannot tell (a torn read looks the same): the write is refused, the lease
    // stands until the self-fence deadline, and nothing is written over the damaged record.
    let refused = a.check().unwrap_err();
    assert!(refused.reason.contains("checksum"), "{refused:?}");
    assert!(refused.transient, "{refused:?}");
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    c.advance(s(19));
    eventually("failed beats", || a.failed_heartbeats() >= 1);
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    c.advance(s(1));
    let lost = a.check().unwrap_err();
    assert!(lost.reason.contains("self-fenced"), "{lost:?}");
    assert!(matches!(a.state(), LeaseState::Lost { .. }));
    assert_eq!(fs::read(&path).unwrap(), bytes, "never written over");
    let mut b = contender(&dir, B, local(&c));
    held_elsewhere(b.try_acquire());
    // Never taken over automatically, however long it stays damaged…
    for _ in 0..4 {
        c.advance(s(30));
        held_elsewhere(b.try_acquire());
    }
    // …nor by a plain takeover on network storage; only with confirmation.
    assert_eq!(b.takeover(false).unwrap_err().details["reason"], "HELD");
    assert_eq!(b.takeover(true).unwrap().epoch(), 2);
    assert_eq!(read_record(&dir).server_id, B);
}

#[test]
fn unreadable_record_is_lost_on_fence_at_the_deadline() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    fs::remove_file(dir.join("lease.json")).unwrap();
    fs::create_dir(dir.join("lease.json")).unwrap();
    assert!(a.check().unwrap_err().transient);
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    c.advance(s(20));
    assert!(a.check().unwrap_err().reason.contains("self-fenced"));
    assert!(matches!(a.state(), LeaseState::Lost { .. }));
    held_elsewhere(Lease::acquire(&dir, B, "b", local(&c)));
}

#[test]
fn check_fails_once_another_server_overwrites_record() {
    for (server, epoch) in [(B, 2), (A, 2), (B, 1)] {
        let (_t, dir) = folder();
        let c = FakeClock::new();
        let a = acquire(&dir, A, local(&c));
        let mut r = read_record(&dir);
        (r.server_id, r.epoch) = (server.to_string(), epoch);
        overwrite(&dir, &r);
        let lost = a.check().unwrap_err();
        assert!(lost.reason.contains("lease.json"), "{lost:?}");
        assert_eq!(a.check().unwrap_err(), lost, "lost stays lost");
        // The heartbeat thread has ended, so nothing writes over it.
        a.hooks().wait_exited();
        c.advance(s(6));
        assert_eq!(read_record(&dir), r, "{server} {epoch}");
    }
}

#[test]
fn heartbeats_keep_the_lease_and_stop_on_drop() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    for i in 1..=12 {
        c.advance(s(5));
        eventually("a heartbeat", || a.heartbeats() >= i);
        assert_eq!(a.check(), Ok(1), "after {i} heartbeats");
    }
    let r = read_record(&dir);
    assert_eq!(r.heartbeat, 12);
    let hooks = a.hooks();
    assert!(!hooks.exited());
    drop(a);
    assert!(hooks.exited(), "drop stopped and joined the thread");
    c.advance(s(10));
    assert_eq!(read_record(&dir), r, "no heartbeat after drop");
}

#[test]
fn self_fences_after_20s_without_heartbeat() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    a.fail_heartbeats(u32::MAX);
    c.advance(s(19));
    eventually("failed attempts", || a.failed_heartbeats() >= 1);
    assert_eq!(a.check(), Ok(1));
    c.advance(ms(1_000));
    let lost = a.check().unwrap_err();
    assert!(lost.reason.contains("self-fenced"), "{lost:?}");
    assert!(matches!(a.state(), LeaseState::Lost { .. }));
    assert_eq!(read_record(&dir).heartbeat, 0, "nothing was written");
}

#[test]
fn a_failed_heartbeat_is_retried() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    a.fail_heartbeats(1);
    c.advance(s(5));
    eventually("the failed attempt", || a.failed_heartbeats() == 1);
    assert_eq!(a.heartbeats(), 0);
    c.advance(s(1));
    eventually("the retry", || a.heartbeats() == 1);
    assert_eq!(a.check(), Ok(1));
    assert_eq!(read_record(&dir).heartbeat, 1);
}

#[test]
fn stale_needs_two_probes_5s_apart() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    drop(acquire(&dir, A, local(&c)));
    let mut b = contender(&dir, B, local(&c));
    held_elsewhere(b.try_acquire()); // B starts watching
    c.advance(s(44));
    let (_, _, age) = held_elsewhere(b.try_acquire());
    assert!(
        age.is_some_and(|a| (43_000..45_000).contains(&a)),
        "{age:?}"
    );
    c.advance(s(2));
    held_elsewhere(b.try_acquire()); // the first stale probe
    c.advance(ms(4_900));
    held_elsewhere(b.try_acquire()); // 4.9 s later: not yet
    c.advance(ms(100));
    let l = b.try_acquire().unwrap();
    assert_eq!(l.epoch(), 2);
    assert_eq!(read_record(&dir).server_id, B);
}

#[test]
fn a_slow_probe_does_not_count() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    drop(acquire(&dir, A, local(&c)));
    let mut b = contender(&dir, B, local(&c));
    c.advance(s(60));
    // Every clock read takes 3 s: each probe's round trip exceeds TTL/4.
    c.set_step(s(3));
    for _ in 0..6 {
        c.advance(s(5));
        let (_, _, age) = held_elsewhere(b.try_acquire());
        assert_eq!(age, None);
    }
    c.set_step(Duration::ZERO);
    held_elsewhere(b.try_acquire());
    c.advance(s(5));
    assert_eq!(b.try_acquire().unwrap().epoch(), 2);
}

#[test]
fn a_record_change_between_probes_resets_staleness() {
    let (_t, dir) = folder();
    let (h, o) = (FakeClock::new(), FakeClock::new());
    // The holder's clock reads far behind the observer's: its stamps look 55 s old.
    let a = acquire(&dir, A, local(&h));
    let mut b = contender(&dir, B, local(&o));
    o.advance(s(60));
    held_elsewhere(b.try_acquire()); // stale, first probe
    h.advance(s(5));
    eventually("a heartbeat", || a.heartbeats() == 1);
    o.advance(s(5));
    held_elsewhere(b.try_acquire()); // the record changed: start again
    o.advance(s(20));
    held_elsewhere(b.try_acquire()); // stale and 5 s apart, but unchanged for only 20 s
    o.advance(s(5));
    let l = b.try_acquire().unwrap();
    assert_eq!(l.epoch(), 2);
    assert!(a.check().is_err());
}

#[test]
fn takeover_on_network_after_confirmed_staleness_fences_the_old_holder() {
    let (_t, dir) = folder();
    let (h, o) = (FakeClock::new(), FakeClock::new());
    // A is frozen (its clock does not move, so it neither heartbeats nor self-fences).
    let a = acquire(&dir, A, params(StorageKind::Network, &h));
    let mut b = contender(&dir, B, params(StorageKind::Network, &o));
    assert_eq!(b.takeover(false).unwrap_err().details["reason"], "HELD");
    o.advance(s(46));
    assert_eq!(
        b.takeover(false).unwrap_err().details["reason"],
        "STALE_UNCONFIRMED"
    );
    o.advance(s(5));
    let bl = b.takeover(false).unwrap();
    assert_eq!(bl.epoch(), 2);
    assert_eq!(bl.check(), Ok(2));
    let lost = a.check().unwrap_err();
    assert!(lost.reason.contains("serverb"), "{lost:?}");
    // A's heartbeat has ended and does not resurrect it.
    a.hooks().wait_exited();
    h.advance(s(5));
    assert_eq!(read_record(&dir).server_id, B);
    assert_eq!(claims(&dir), [2], "claims below the new epoch are gone");
}

#[test]
fn sync_kind_requires_confirm() {
    let (_t, dir) = folder();
    let (h, o) = (FakeClock::new(), FakeClock::new());
    let a = acquire(&dir, A, sync(&h));
    let mut b = contender(&dir, B, sync(&o));
    // Live: refused without confirmation.
    let e = b.takeover(false).unwrap_err();
    assert_eq!(e.code, "LEASE_TAKEOVER_REFUSED");
    assert_eq!(e.details["reason"], "CONFIRM_REQUIRED");
    // Stale: still never automatic.
    o.advance(s(60));
    held_elsewhere(b.try_acquire());
    o.advance(s(5));
    match b.try_acquire() {
        Err(LeaseState::Stale { holder, epoch }) => {
            assert_eq!((holder.as_str(), epoch), ("holder servera", 1));
        }
        other => panic!("{:?}", other.map(|l| l.epoch())),
    }
    assert_eq!(
        b.takeover(false).unwrap_err().details["reason"],
        "CONFIRM_REQUIRED"
    );
    let bl = b.takeover(true).unwrap();
    assert_eq!(bl.epoch(), 2);
    assert!(a.check().is_err());
}

#[test]
fn reacquire_after_lost_takes_fresh_epoch() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    a.fail_heartbeats(u32::MAX);
    c.advance(s(21));
    assert!(a.check().is_err());
    // This process wrote the record last and nothing heartbeats it: no wait for staleness.
    let mut again = None;
    eventually("re-acquisition", || {
        again = Lease::acquire(&dir, A, "holder servera", local(&c)).ok();
        again.is_some()
    });
    let again = again.unwrap();
    assert_eq!(again.epoch(), 2);
    assert!(a.check().is_err(), "the old lease stays lost");
    // A live lease in this process is never displaced by another of the same server.
    let (_, epoch, _) = held_elsewhere(Lease::acquire(&dir, A, "x", local(&c)));
    assert_eq!(epoch, 2);
    drop(again);
    let mut third = None;
    eventually("re-acquisition after drop", || {
        third = Lease::acquire(&dir, A, "x", local(&c)).ok();
        third.is_some()
    });
    assert_eq!(third.unwrap().epoch(), 3);
}

#[test]
fn reacquire_after_the_folder_returns() {
    let (t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let away = t.path().join("away");
    fs::rename(&dir, &away).unwrap();
    assert!(a.check().is_err());
    assert!(matches!(
        Lease::acquire(&dir, A, "x", local(&c)),
        Err(LeaseState::Lost { .. })
    ));
    assert!(
        !dir.exists(),
        "acquiring never recreates the folder's contents"
    );
    fs::rename(&away, &dir).unwrap();
    let mut back = None;
    eventually("re-acquisition", || {
        back = Lease::acquire(&dir, A, "x", local(&c)).ok();
        back.is_some()
    });
    assert_eq!(back.unwrap().epoch(), 2);
}

#[test]
fn old_claims_deleted_after_acquire() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    drop(acquire(&dir, A, local(&c)));
    assert_eq!(claims(&dir), [1]);
    for (id, epoch) in [(B, 2), (C, 3)] {
        let mut x = contender(&dir, id, local(&c));
        held_elsewhere(x.try_acquire());
        c.advance(s(46));
        held_elsewhere(x.try_acquire());
        c.advance(s(5));
        drop(x.try_acquire().unwrap());
        assert_eq!(claims(&dir), [epoch]);
    }
}

#[test]
fn a_rival_claim_in_progress_backs_off() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    drop(acquire(&dir, A, local(&c)));
    let mut b = contender(&dir, B, local(&c));
    held_elsewhere(b.try_acquire());
    c.advance(s(46));
    held_elsewhere(b.try_acquire());
    c.advance(s(5));
    // C claimed epoch 2 a moment ago (51 s into the fake clock) and is about to write the
    // record.
    fs::write(dir.join(claim_name(2)), C).unwrap();
    let f = fs::File::options()
        .write(true)
        .open(dir.join(claim_name(2)))
        .unwrap();
    f.set_modified(std::time::SystemTime::now() + s(51))
        .unwrap();
    let (_, epoch, _) = held_elsewhere(b.try_acquire());
    assert_eq!(epoch, 2);
    assert_eq!(read_record(&dir).server_id, A, "B wrote nothing");
    assert_eq!(claims(&dir), [1, 2], "B left no claim behind");
}

/// `.~fin-*.tmp` files directly in `dir`.
fn temps(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            fin_store::names::is_temp(name.to_str().unwrap())
        })
        .count()
}

/// A read of the holder's own record that fails (a Linux cifs `soft` mount returns `EIO`
/// while it reconnects) is a failed beat, retried: nothing is lost, nothing is written.
#[test]
fn a_failed_read_in_a_beat_is_retried_not_lost() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    fault_record_reads_for_test(&dir, 0, ReadFault::Error(ErrorKind::Other), 1);
    c.advance(s(5));
    eventually("the failed beat", || a.failed_heartbeats() == 1);
    assert_eq!(a.heartbeats(), 0);
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    assert_eq!(read_record(&dir).heartbeat, 0, "nothing written");
    c.advance(s(1));
    eventually("the retry", || a.heartbeats() == 1);
    assert_eq!(a.check(), Ok(1));
    assert_eq!(read_record(&dir).heartbeat, 1);
}

/// Reads that keep failing lose the lease at the self-fence deadline (2×TTL after the last
/// successful beat), not before; a check meanwhile refuses its write without losing it.
#[test]
fn reads_failing_until_the_deadline_lose_the_lease_only_then() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    fault_record_reads_for_test(&dir, 0, ReadFault::Error(ErrorKind::Other), 10_000);
    c.advance(s(5));
    eventually("a failed beat", || a.failed_heartbeats() >= 1);
    c.advance(s(14));
    eventually("another failed beat", || a.failed_heartbeats() >= 2);
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    let refused = a.check().unwrap_err();
    assert!(
        refused.transient && refused.reason.contains("unreadable"),
        "{refused:?}"
    );
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    c.advance(s(1));
    match a.state() {
        LeaseState::Lost { reason } => assert!(reason.contains("self-fenced"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(a.heartbeats(), 0);
    assert_eq!(read_record(&dir).heartbeat, 0, "nothing written");
    fault_record_reads_for_test(&dir, 0, ReadFault::Real, 0);
}

/// A torn read-back (SMB) is re-read; one that settles within the re-reads is no failure, one
/// that outlasts them is a failed beat that counts for nothing (the write landed: the next
/// beat adopts it).
#[test]
fn a_torn_read_back_settles_or_is_retried() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    // The fence reads for real, then the read-back is torn three times.
    fault_record_reads_for_test(&dir, 1, ReadFault::Torn, 3);
    c.advance(s(5));
    eventually("a heartbeat", || a.heartbeats() == 1);
    assert_eq!(a.failed_heartbeats(), 0);
    // Torn for the first read and all ten re-reads.
    fault_record_reads_for_test(&dir, 1, ReadFault::Torn, 11);
    c.advance(s(5));
    eventually("the failed beat", || a.failed_heartbeats() == 1);
    assert_eq!(a.heartbeats(), 1, "a retried beat is no success");
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    assert_eq!(read_record(&dir).heartbeat, 2, "its write landed");
    c.advance(s(1));
    eventually("the retry", || a.heartbeats() == 2);
    assert_eq!(read_record(&dir).heartbeat, 3);
    assert_eq!(a.check(), Ok(1));
}

/// A commit may trust a full check up to 1 s old, but not across a beat that could not read
/// the record: that commit runs a full check of its own.
#[test]
fn a_doubtful_beat_makes_the_next_commit_check_in_full() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    c.advance(ms(4_500));
    assert_eq!(a.check(), Ok(1));
    let full = a.hooks().full_checks();
    assert_eq!(a.check_recent(s(1)), Ok(1));
    assert_eq!(a.hooks().full_checks(), full, "a fresh check is trusted");
    fault_record_reads_for_test(&dir, 0, ReadFault::Torn, 10_000);
    c.advance(ms(500));
    eventually("the failed beat", || a.failed_heartbeats() == 1);
    let refused = a.check_recent(s(1)).unwrap_err();
    assert!(refused.transient, "{refused:?}");
    assert_eq!(
        a.hooks().full_checks(),
        full + 1,
        "not trusted after the beat"
    );
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    fault_record_reads_for_test(&dir, 0, ReadFault::Real, 0);
}

/// A beat that doubts while a check is running (after the check read the record, before it
/// finished) wins: the commit after them runs a full check rather than trusting that one.
#[test]
fn a_beat_doubting_during_a_check_is_not_overridden_by_it() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    c.advance(ms(4_500));
    let hooks = a.hooks();
    hooks.hang(HangPoint::CheckAfterVerify);
    // The check's read is real; the beat's fails.
    fault_record_reads_for_test(&dir, 1, ReadFault::Error(ErrorKind::Other), 1);
    std::thread::scope(|sc| {
        let check = sc.spawn(|| a.check());
        hooks.wait_hung();
        c.advance(ms(500));
        eventually("the failed beat", || a.failed_heartbeats() == 1);
        hooks.release();
        assert_eq!(check.join().unwrap(), Ok(1));
    });
    let full = hooks.full_checks();
    assert_eq!(a.check_recent(s(1)), Ok(1));
    assert_eq!(hooks.full_checks(), full + 1, "the check was not trusted");
}

/// A read-back showing an older heartbeat of ours (the write did not land, or not visibly)
/// is a failed beat: it must not move the self-fence deadline.
#[test]
fn a_read_back_of_an_older_heartbeat_is_no_success() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    fault_record_reads_for_test(&dir, 1, ReadFault::Stale, 1);
    c.advance(s(5));
    eventually("the failed beat", || a.failed_heartbeats() == 1);
    assert_eq!(a.heartbeats(), 0);
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    c.advance(s(1));
    eventually("the retry", || a.heartbeats() == 1);
    assert_eq!(
        read_record(&dir).heartbeat,
        2,
        "the retry adopted the landed write"
    );
}

/// Without hard links the first record falls back to `create_new` and writing in place.
#[test]
fn without_hard_links_the_first_record_is_created_in_place() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    no_hard_links_for_test(&dir);
    let a = acquire(&dir, A, local(&c));
    assert_eq!(a.epoch(), 1);
    assert_eq!(read_record(&dir).server_id, A);
    assert_eq!(temps(&dir), 0, "the staged temp file is removed");
    assert_eq!(a.check(), Ok(1));
}

/// Evidence still loses the lease at once in a beat: `lease.json` deleted (and never
/// recreated).
#[test]
fn a_deleted_record_loses_the_lease_in_a_beat_at_once() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    fs::remove_file(dir.join("lease.json")).unwrap();
    c.advance(s(5));
    a.hooks().wait_exited();
    match a.state() {
        LeaseState::Lost { reason } => assert!(reason.contains("missing"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(!dir.join("lease.json").exists());
    // Only our own claim is left. To another server the folder is held (by us: missing +
    // claims); we know we hold nothing, so we report lost, not held by ourselves.
    let (holder, epoch, _) = held_elsewhere(contender(&dir, B, local(&c)).try_acquire());
    assert_eq!((holder.as_str(), epoch), (A, 1));
    let mut a2 = contender(&dir, A, local(&c));
    match a2.try_acquire() {
        Err(LeaseState::Lost { reason }) => assert!(reason.contains("epoch 1"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // A takeover is refused as for any live-looking holder, but says what happened.
    let e = a2.takeover(false).unwrap_err();
    assert_eq!(e.details["reason"], "HELD");
    assert!(
        e.message
            .contains("this server's lease at epoch 1 was lost"),
        "{e:?}"
    );
    // Re-acquired like by anyone once the claim is stale, at a fresh epoch.
    c.advance(s(46));
    assert!(matches!(a2.try_acquire(), Err(LeaseState::Lost { .. })));
    c.advance(s(5));
    assert_eq!(a2.try_acquire().unwrap().epoch(), 2);
}

/// On a sync folder our own lost claim is re-taken only by a confirmed takeover.
#[test]
fn a_deleted_record_on_a_sync_folder_is_re_taken_by_a_confirmed_takeover() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, sync(&c));
    fs::remove_file(dir.join("lease.json")).unwrap();
    c.advance(s(5));
    a.hooks().wait_exited();
    let mut a2 = contender(&dir, A, sync(&c));
    match a2.try_acquire() {
        Err(LeaseState::Lost { reason }) => assert!(reason.contains("--confirm"), "{reason}"),
        other => panic!("{other:?}"),
    }
    let e = a2.takeover(false).unwrap_err();
    assert_eq!(e.details["reason"], "CONFIRM_REQUIRED");
    assert!(
        e.message.contains("this server's lease at epoch 1"),
        "{e:?}"
    );
    assert_eq!(a2.takeover(true).unwrap().epoch(), 2);
}

/// The first `lease.json` is staged whole in a temp file and published without replacing: a
/// crash before publishing leaves a claim without a record (taken over automatically once
/// stale), never a short record (which only `takeover --confirm` could clear).
#[test]
fn a_crash_before_publishing_the_first_record_leaves_no_damaged_record() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    claim::keep_backed_off_claims_for_test(&dir);
    crash_before_publishing_record_for_test(&dir);
    match Lease::acquire(&dir, A, "holder a", local(&c)) {
        Err(LeaseState::Lost { reason }) => assert!(reason.contains("lease.json"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(!dir.join("lease.json").exists());
    assert_eq!(claims(&dir), [1]);
    assert_eq!(temps(&dir), 1, "the crash left its temp file");
    let mut b = contender(&dir, B, local(&c));
    let (holder, epoch, _) = held_elsewhere(b.try_acquire());
    assert_eq!((holder.as_str(), epoch), (A, 1));
    c.advance(s(46));
    held_elsewhere(b.try_acquire());
    c.advance(s(5));
    let l = b.try_acquire().unwrap();
    assert_eq!(l.epoch(), 2);
    assert_eq!(read_record(&dir).server_id, B);
    assert_eq!(temps(&dir), 0, "swept once the lease is held");
}

/// Creating a claim whose name is still pending deletion fails on Windows with
/// `ERROR_ACCESS_DENIED`: retried briefly, not lost.
#[test]
fn a_claim_name_pending_deletion_is_retried() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    deny_claim_creates_for_test(&dir, 3);
    let a = acquire(&dir, A, local(&c));
    assert_eq!(a.epoch(), 1);
    assert_eq!(claims(&dir), [1]);
    assert_eq!(a.check(), Ok(1));
}

/// Still denied after the retries (1.5 s): this attempt fails without creating anything, and
/// the next one succeeds.
#[test]
fn a_claim_denied_past_the_retries_is_tried_again_later() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    deny_claim_creates_for_test(&dir, 61);
    match Lease::acquire(&dir, A, "holder a", local(&c)) {
        Err(LeaseState::Lost { reason }) => assert!(reason.contains("claim"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(claims(&dir).is_empty());
    assert!(!dir.join("lease.json").exists());
    assert_eq!(acquire(&dir, A, local(&c)).epoch(), 1);
}

/// Retrying never gives a claim name two owners: once the name stops being denied, another
/// server's claim still wins (`AlreadyExists`), even one our listing cannot see yet.
#[test]
fn retrying_a_denied_claim_never_takes_anothers_name() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    claim::hide_young_claims(&dir, s(60));
    fs::write(dir.join(claim_name(1)), C).unwrap();
    deny_claim_creates_for_test(&dir, 2);
    let (_, epoch, _) = held_elsewhere(Lease::acquire(&dir, A, "holder a", local(&c)));
    assert_eq!(epoch, 1);
    assert_eq!(fs::read_to_string(dir.join(claim_name(1))).unwrap(), C);
    assert!(!dir.join("lease.json").exists());
}

#[test]
fn server_id_is_stable() {
    let tmp = tempfile::tempdir().unwrap();
    let a = fin_store::server_id(tmp.path()).unwrap();
    assert_eq!(fin_store::server_id(tmp.path()).unwrap(), a);
    assert!(a.bytes().all(|b| b.is_ascii_alphanumeric()));
    // The probe file is named after it, so it must be a valid lease file name.
    assert!(fin_store::conflict_names::is_lease_name(&format!(
        "lease.probe-{a}"
    )));
}

// ---- Store fencing ----

/// A store whose lease (acquired at create) runs on the fake clock.
fn store_with(data: &Path, proj: &Path, c: &Arc<FakeClock>) -> Store {
    let opts = fin_store::OpenOptions {
        clock: Some(c.clone()),
        poll: Some(ms(2)),
        ..fin_store::OpenOptions::default()
    };
    Store::create_with(data, proj, "P", Rational::from_int(30), None, opts).unwrap()
}

fn tx(id: &str, base_rev: u64) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops: vec![
            fin_core::AssetAdd::media(
                "$a",
                format!("/media/{id}.mov"),
                Rational::from_int(60),
                Some(VideoInfo {
                    width: 1920,
                    height: 1080,
                    rate: Rational::from_int(30),
                }),
                None,
                "fp",
            )
            .into(),
        ],
    }
}

#[test]
fn commit_refuses_without_a_held_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let (data, proj) = (tmp.path().join("data"), tmp.path().join("P"));
    let c = FakeClock::new();
    let mut store = store_with(&data, &proj, &c);
    let dir = proj.join(".finisher");
    store.commit(tx("t1", 0)).unwrap();
    // Recorded at the checked epoch: the segment is labelled with it.
    let name = store.planned_segment_name(MAIN).unwrap().unwrap();
    assert_eq!(&name[..8], "00000001");

    // Another server takes the record over. A commit trusts a full check for 1 s (it only
    // writes the local DB); after that the next full check sees the takeover.
    let mut r = read_record(&dir);
    (r.server_id, r.epoch) = (B.to_string(), 2);
    overwrite(&dir, &r);
    c.advance(ms(1_001));
    let e = fin_core::CoreError::from(store.commit(tx("t2", 1)).unwrap_err());
    assert_eq!(e.code, "PROJECT_READ_ONLY");
    assert_eq!(e.details["reason"], "LEASE_LOST");
    assert_eq!(store.head(MAIN), 1);
    // A retry of the committed tx is still answered (it writes nothing).
    assert_eq!(store.commit(tx("t1", 0)).unwrap().rev, 1);
    assert_eq!(
        store.close().unwrap().segments_written,
        0,
        "no export either"
    );

    // The test lease hook fences the same way.
    let mut store = Store::open(&data, &proj).unwrap();
    store.set_test_lease(3, false);
    let e = fin_core::CoreError::from(store.commit(tx("t3", 1)).unwrap_err());
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("PROJECT_READ_ONLY", &serde_json::json!("LEASE_LOST"))
    );
}

// ---- Review follow-up: suspend-aware fencing, claim authority, safer takeover ----

use fin_store::lease::HangPoint;

#[test]
fn a_hung_beat_past_the_deadline_never_writes() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let hooks = a.hooks();
    hooks.hang(HangPoint::BeatBeforeWrite);
    c.advance(s(5));
    hooks.wait_hung(); // the beat has stamped and verified, then stalls (a slow share)
    c.advance(s(21)); // past last heartbeat + 20 s
    hooks.release();
    hooks.wait_exited();
    assert_eq!(
        read_record(&dir).heartbeat,
        0,
        "the stalled beat wrote nothing"
    );
    assert!(a.check().is_err());
    assert!(matches!(a.state(), LeaseState::Lost { .. }));
}

#[test]
fn a_beat_stalled_across_drop_never_writes() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let hooks = a.hooks();
    hooks.hang(HangPoint::BeatBeforeWrite);
    c.advance(s(5));
    hooks.wait_hung();
    let releaser = {
        let hooks = a.hooks();
        std::thread::spawn(move || {
            std::thread::sleep(ms(50));
            hooks.release();
        })
    };
    drop(a);
    releaser.join().unwrap();
    hooks.wait_exited();
    assert_eq!(
        read_record(&dir).heartbeat,
        0,
        "a dropped lease never writes"
    );
}

/// A Mac sleeps right after its heartbeat verified the record: on wake both clocks have
/// moved 10 minutes, and another server has taken over. The pending write must not happen.
#[test]
fn a_holder_waking_from_sleep_does_not_overwrite_the_new_holder() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let mut b = contender(&dir, B, local(&c));
    held_elsewhere(b.try_acquire());
    let hooks = a.hooks();
    hooks.hang(HangPoint::BeatBeforeWrite);
    c.advance(s(5));
    hooks.wait_hung();
    c.advance(s(600));
    // B slept too (one clock): its evidence expired, so it watches for 25 s again.
    held_elsewhere(b.try_acquire());
    c.advance(s(25));
    let bl = b.try_acquire().unwrap();
    assert_eq!(bl.epoch(), 2);
    hooks.release();
    hooks.wait_exited();
    let r = read_record(&dir);
    assert_eq!((r.server_id.as_str(), r.epoch), (B, 2));
    assert!(a.check().is_err());
    assert!(matches!(a.state(), LeaseState::Lost { .. }));
    assert_eq!(bl.check(), Ok(2));
}

#[test]
fn a_newer_claim_loses_the_lease() {
    // Seen by the fence…
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    fs::write(dir.join(claim_name(2)), C).unwrap();
    let lost = a.check().unwrap_err();
    assert!(lost.reason.contains("lease.claim-00000002"), "{lost:?}");
    // …and by the heartbeat, which then stops without writing.
    let (_t2, dir2) = folder();
    let a2 = acquire(&dir2, A, local(&c));
    fs::write(dir2.join(claim_name(5)), C).unwrap();
    c.advance(s(5));
    a2.hooks().wait_exited();
    assert_eq!(read_record(&dir2).heartbeat, 0);
    assert!(matches!(a2.state(), LeaseState::Lost { .. }));
    // A claim at or below our epoch is no threat.
    let (_t3, dir3) = folder();
    let a3 = acquire(&dir3, A, local(&c));
    fs::write(dir3.join(claim_name(0)), C).unwrap();
    assert_eq!(a3.check(), Ok(1));
}

#[test]
fn a_running_holder_is_never_taken_over_whatever_the_clocks_say() {
    let (_t, dir) = folder();
    let (h, o) = (FakeClock::new(), FakeClock::new());
    let a = acquire(&dir, A, local(&h));
    let mut b = contender(&dir, B, local(&o));
    // The observer's server time runs 60 s ahead of the holder's stamps…
    o.advance(s(60));
    // …and the holder's heartbeats fail for 15 s (but it is within its 20 s).
    a.fail_heartbeats(u32::MAX);
    h.advance(s(15));
    for _ in 0..4 {
        held_elsewhere(b.try_acquire());
        o.advance(s(5));
    }
    // Its next beat succeeds and changes the record before 25 s of it being unchanged.
    a.fail_heartbeats(0);
    h.advance(s(1));
    eventually("a heartbeat", || a.heartbeats() == 1);
    for _ in 0..4 {
        held_elsewhere(b.try_acquire());
        o.advance(s(5));
    }
    assert_eq!(a.check(), Ok(1));
}

#[test]
fn stale_evidence_expires() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    drop(acquire(&dir, A, local(&c)));
    let mut b = contender(&dir, B, local(&c));
    held_elsewhere(b.try_acquire());
    c.advance(s(46));
    held_elsewhere(b.try_acquire()); // first stale probe
    c.advance(s(100)); // no observation for more than 2 × 45 s
    held_elsewhere(b.try_acquire()); // starts over
    c.advance(s(5));
    held_elsewhere(b.try_acquire()); // 5 s apart, but watched for only 5 s
    c.advance(s(20));
    assert_eq!(b.try_acquire().unwrap().epoch(), 2);
}

#[test]
fn a_record_with_our_epoch_and_a_later_heartbeat_is_ours() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    // A heartbeat write that landed late (after its retry) leaves a later counter.
    let mut r = read_record(&dir);
    r.heartbeat = 7;
    overwrite(&dir, &r);
    assert_eq!(a.check(), Ok(1));
    c.advance(s(5));
    eventually("a heartbeat", || a.heartbeats() == 1);
    assert_eq!(read_record(&dir).heartbeat, 8);
}

#[test]
fn a_lost_lease_with_a_stuck_thread_does_not_block_reacquiring() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let hooks = a.hooks();
    hooks.hang(HangPoint::BeatBeforeWrite);
    c.advance(s(5));
    hooks.wait_hung();
    c.advance(s(21));
    assert!(a.check().is_err());
    let again = Lease::acquire(&dir, A, "x", local(&c)).unwrap();
    assert_eq!(again.epoch(), 2);
    hooks.release();
    hooks.wait_exited();
    assert_eq!(read_record(&dir).epoch, 2);
    assert_eq!(again.check(), Ok(2));
}

#[test]
fn rival_claims_are_aged_only_by_a_timely_probe() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    drop(acquire(&dir, A, local(&c)));
    // A claim above the record, written 100 s ago: an acquirer that crashed.
    fs::write(dir.join(claim_name(5)), C).unwrap();
    fs::File::options()
        .write(true)
        .open(dir.join(claim_name(5)))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - s(100))
        .unwrap();
    // A slow probe cannot tell its age: assume it is fresh.
    c.set_step(s(3));
    held_elsewhere(Lease::acquire(&dir, A, "x", local(&c)));
    c.set_step(Duration::ZERO);
    assert_eq!(Lease::acquire(&dir, A, "x", local(&c)).unwrap().epoch(), 6);
}

#[test]
fn a_failed_read_back_leaves_no_claim() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let mut b = contender(&dir, B, local(&c));
    b.after_write_for_test(|dir| {
        let r = LeaseRecord::new(C, 9, 0, "c", 0);
        fin_store::fsio::write_in_place(&dir.join("lease.json"), &record::encode(&r)).unwrap();
    });
    held_elsewhere(b.try_acquire());
    assert_eq!(claims(&dir), Vec::<u64>::new(), "B removed its claim");
}

#[test]
fn a_torn_read_back_is_rewritten() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let mut b = contender(&dir, B, local(&c));
    b.after_write_for_test(|dir| {
        let mut bytes = fs::read(dir.join("lease.json")).unwrap();
        bytes[30] ^= 0x01;
        fin_store::fsio::write_in_place(&dir.join("lease.json"), &bytes).unwrap();
    });
    let l = b.try_acquire().unwrap();
    assert_eq!(l.epoch(), 1);
    assert_eq!(read_record(&dir).server_id, B);
    assert_eq!(l.check(), Ok(1));
}

#[test]
fn two_contenders_racing_never_both_hold() {
    let c = FakeClock::new();
    let tmp = tempfile::tempdir().unwrap();
    let mut held = 0;
    for round in 0..150 {
        let dir = tmp.path().join(format!("r{round}"));
        fs::create_dir(&dir).unwrap();
        // Half the rounds hide fresh claims from listings, as Windows does for up to 9.9 s.
        if round % 2 == 1 {
            claim::hide_young_claims(&dir, s(5));
        }
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [A, B]
            .into_iter()
            .map(|id| {
                let (dir, barrier, c) = (dir.clone(), barrier.clone(), c.clone());
                std::thread::spawn(move || {
                    let mut x = contender(&dir, id, local(&c));
                    barrier.wait();
                    x.try_acquire().ok()
                })
            })
            .collect();
        let leases: Vec<Lease> = handles
            .into_iter()
            .filter_map(|h| h.join().unwrap())
            .collect();
        let valid: Vec<&Lease> = leases.iter().filter(|l| l.check().is_ok()).collect();
        assert!(valid.len() <= 1, "round {round}: {leases:?}");
        if let [l] = valid.as_slice() {
            assert_eq!(read_record(&dir).epoch, l.epoch(), "round {round}");
            held += 1;
        }
    }
    assert!(held > 100, "a winner in most rounds: {held}");
}

#[test]
fn commit_does_not_hold_the_database_lock_during_a_blocked_check() {
    let tmp = tempfile::tempdir().unwrap();
    let (data, proj) = (tmp.path().join("data"), tmp.path().join("P"));
    let c = FakeClock::new();
    let mut store = store_with(&data, &proj, &c);
    let hooks = store.lease().expect("acquired at create").hooks();
    let db = store.db_path().to_path_buf();
    // Past the 1 s a commit trusts the check made by create's import.
    c.advance(ms(1_001));
    hooks.hang(HangPoint::CheckBeforeRead);
    let committer = std::thread::spawn(move || store.commit(tx("t1", 0)).map(|c| c.rev));
    hooks.wait_hung();
    // While the fence waits on the share, the database stays writable.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.busy_timeout(Duration::ZERO).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
        .expect("no write lock held across the lease check");
    hooks.release();
    assert_eq!(committer.join().unwrap().unwrap(), 1);
}

// ---- Second review: fewer spurious lease losses ----

#[test]
fn many_contenders_confirming_together_never_fence_the_winner() {
    let tmp = tempfile::tempdir().unwrap();
    for round in 0..40 {
        let dir = tmp.path().join(format!("r{round}"));
        fs::create_dir(&dir).unwrap();
        if round % 2 == 1 {
            claim::hide_young_claims(&dir, s(5));
        }
        // A loser's clean-up that never happens (it crashed, or its delete is slow).
        claim::keep_backed_off_claims_for_test(&dir);
        let c = FakeClock::new();
        // A holder that died 100 s ago (its last stamp is that old). The fake clock then only
        // moves 26 s, so claims made during the race are, as on a real share, seen as young
        // (files written after a fake advance look as old as the advance).
        drop(acquire(&dir, A, local(&c)));
        let mut r = read_record(&dir);
        r.probe_server_ms -= 100_000;
        overwrite(&dir, &r);
        let ids = [B, C, "serverd", "servere"];
        let mut cs: Vec<Contender> = ids
            .iter()
            .map(|id| contender(&dir, id, local(&c)))
            .collect();
        for x in &mut cs {
            held_elsewhere(x.try_acquire()); // the first stale probe
        }
        c.advance(s(26));
        let barrier = Arc::new(Barrier::new(cs.len()));
        let handles: Vec<_> = cs
            .into_iter()
            .map(|mut x| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    x.try_acquire().ok()
                })
            })
            .collect();
        let leases: Vec<Lease> = handles
            .into_iter()
            .filter_map(|h| h.join().unwrap())
            .collect();
        assert_eq!(leases.len(), 1, "round {round}: one winner");
        assert_eq!(
            leases[0].check(),
            Ok(2),
            "round {round}: not fenced by the losers"
        );
        // (With listings lagging, the winner may not have seen claim 1 to delete it yet.)
        let left = claims(&dir);
        assert!(
            left.contains(&2) && left.iter().all(|e| *e <= 2),
            "round {round}: the losers created nothing: {left:?}"
        );
    }
}

#[test]
fn an_old_claim_above_our_epoch_is_ignored() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    // An acquirer that crashed 100 s ago, right after claiming.
    fs::write(dir.join(claim_name(2)), C).unwrap();
    fs::File::options()
        .write(true)
        .open(dir.join(claim_name(2)))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - s(100))
        .unwrap();
    assert_eq!(a.check(), Ok(1));
    c.advance(s(5));
    eventually("a heartbeat", || a.heartbeats() == 1);
    assert_eq!(a.check(), Ok(1));
    // Its late record write is still caught by the record compare.
    let mut r = read_record(&dir);
    (r.server_id, r.epoch) = (C.to_string(), 2);
    overwrite(&dir, &r);
    assert!(a.check().is_err());
}

#[test]
fn a_listing_error_refuses_one_write_but_keeps_the_lease() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    // A transient one is retried inside the listing.
    claim::fail_next_list_for_test(&dir, std::io::ErrorKind::Interrupted);
    assert_eq!(a.check(), Ok(1));
    // Any other refuses this write, without losing the lease.
    claim::fail_next_list_for_test(&dir, std::io::ErrorKind::Other);
    let e = a.check().unwrap_err();
    assert!(e.transient, "{e:?}");
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    assert_eq!(a.check(), Ok(1));
    // In a heartbeat it is a failed beat, retried.
    claim::fail_next_list_for_test(&dir, std::io::ErrorKind::Other);
    c.advance(s(5));
    eventually("the failed beat", || a.failed_heartbeats() == 1);
    assert_eq!(a.state(), LeaseState::Held { epoch: 1 });
    c.advance(s(1));
    eventually("the retry", || a.heartbeats() == 1);
    assert_eq!(a.check(), Ok(1));
}

#[test]
fn commit_trusts_a_full_check_for_one_second() {
    let tmp = tempfile::tempdir().unwrap();
    let (data, proj) = (tmp.path().join("data"), tmp.path().join("P"));
    let c = FakeClock::new();
    let mut store = store_with(&data, &proj, &c);
    let hooks = store.lease().expect("acquired at create").hooks();
    store.commit(tx("t1", 0)).unwrap();
    store.commit(tx("t2", 1)).unwrap();
    assert_eq!(
        hooks.full_checks(),
        1,
        "the second commit trusted the first check"
    );
    c.advance(ms(1_001));
    store.commit(tx("t3", 2)).unwrap();
    assert_eq!(hooks.full_checks(), 2);
    // A lease lost in between is still refused (the in-memory part always runs).
    c.advance(s(21));
    let e = fin_core::CoreError::from(store.commit(tx("t4", 3)).unwrap_err());
    assert_eq!(e.details["reason"], "LEASE_LOST");
}

fn segment_file(dir: &Path, epoch: u64) {
    let seg = dir.join("segments").join(MAIN);
    fs::create_dir_all(&seg).unwrap();
    let name = fin_store::names::SegmentName::new(epoch, 1, 1, "abcdefabcdef").unwrap();
    fs::write(seg.join(name.to_string()), b"x").unwrap();
}

#[test]
fn epochs_never_repeat_when_lease_files_are_gone() {
    // A damaged record, its claims deleted, segments at epoch 7 in the folder.
    let (_t, dir) = folder();
    let c = FakeClock::new();
    drop(acquire(&dir, A, local(&c)));
    segment_file(&dir, 7);
    fs::write(dir.join("lease.json"), [b'x'; 512]).unwrap();
    fs::remove_file(dir.join(claim_name(1))).unwrap();
    let mut b = contender(&dir, B, local(&c));
    assert_eq!(b.takeover(true).unwrap().epoch(), 8);

    // No lease files at all, segments at epoch 7.
    let (_t2, dir2) = folder();
    segment_file(&dir2, 7);
    assert_eq!(acquire(&dir2, B, local(&c)).epoch(), 8);

    // Our own last epoch, after someone deleted every lease file.
    let (_t3, dir3) = folder();
    drop(acquire(&dir3, A, local(&c)));
    let a2 = acquire(&dir3, A, local(&c));
    assert_eq!(a2.epoch(), 2);
    drop(a2);
    fs::remove_file(dir3.join("lease.json")).unwrap();
    for e in claims(&dir3) {
        fs::remove_file(dir3.join(claim_name(e))).unwrap();
    }
    assert_eq!(acquire(&dir3, A, local(&c)).epoch(), 3);
}
