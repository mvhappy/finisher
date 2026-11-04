//! Lease v3 (plan Task 9, spec §3.5, parent spec §27.9): an in-place checksummed record,
//! exclusive claim files for epochs, staleness judged in server time (probe-file mtimes), and
//! a holder that self-fences by its monotonic clock.
//!
//! Every test uses a [`FakeClock`]: the monotonic clock only moves when the test advances it,
//! and the file server's clock (real mtimes) is shifted along with it, so 45 s of server time
//! pass without waiting.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use fin_core::{Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::lease::claim::{self, claim_name};
use fin_store::lease::record::{self, LeaseRecord, RECORD_SIZE};
use fin_store::lease::{Contender, FakeClock, Lease, LeaseState, Params};
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
                    claim::claim_next(&dir, 0, id).unwrap()
                })
            })
            .collect();
        let got: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_ne!(got[0].epoch, got[1].epoch, "round {round}");
        assert_eq!(claims(&dir).len(), 2);
        // Exactly one of them had to step past a taken name.
        assert!(got.iter().filter(|c| c.contended).count() == 1, "{got:?}");
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
fn bad_crc_is_held_on_acquire_and_lost_on_fence() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    let path = dir.join("lease.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes[20] ^= 0x20;
    fin_store::fsio::write_in_place(&path, &bytes).unwrap();
    let lost = a.check().unwrap_err();
    assert!(lost.reason.contains("checksum"), "{lost:?}");
    assert!(matches!(a.state(), LeaseState::Lost { .. }));
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
fn unreadable_record_is_lost_on_fence() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    fs::remove_file(dir.join("lease.json")).unwrap();
    fs::create_dir(dir.join("lease.json")).unwrap();
    assert!(a.check().is_err());
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
        // The heartbeat never writes over it.
        c.advance(s(6));
        std::thread::sleep(ms(30));
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
    drop(a);
    let before = fs::read(dir.join("lease.json")).unwrap();
    c.advance(s(10));
    std::thread::sleep(ms(50));
    assert_eq!(
        fs::read(dir.join("lease.json")).unwrap(),
        before,
        "no heartbeat after drop"
    );
}

#[test]
fn self_fences_after_20s_without_heartbeat() {
    let (_t, dir) = folder();
    let c = FakeClock::new();
    let a = acquire(&dir, A, local(&c));
    a.fail_heartbeats(u32::MAX);
    c.advance(s(19));
    std::thread::sleep(ms(20));
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
    // A's heartbeat does not resurrect it.
    h.advance(s(5));
    std::thread::sleep(ms(30));
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

fn tx(id: &str, base_rev: u64) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        ops: vec![Op::AssetAdd {
            id: "$a".into(),
            path: format!("/media/{id}.mov"),
            duration: Rational::from_int(60),
            video: Some(VideoInfo {
                width: 1920,
                height: 1080,
                rate: Rational::from_int(30),
            }),
            audio: None,
            fingerprint: "fp".into(),
        }],
    }
}

#[test]
fn commit_refuses_without_a_held_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let (data, proj) = (tmp.path().join("data"), tmp.path().join("P"));
    let mut store = Store::create(&data, &proj, "P", Rational::from_int(30)).unwrap();
    let c = FakeClock::new();
    let dir = proj.join(".finisher");
    fs::create_dir_all(&dir).unwrap();
    store.set_lease_for_test(acquire(&dir, A, local(&c)));
    store.commit(tx("t1", 0)).unwrap();
    // Recorded at the checked epoch: the segment is labelled with it.
    let name = store.planned_segment_name(MAIN).unwrap().unwrap();
    assert_eq!(&name[..8], "00000001");

    // Another server takes the record over.
    let mut r = read_record(&dir);
    (r.server_id, r.epoch) = (B.to_string(), 2);
    overwrite(&dir, &r);
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
