//! Store integration, lease side (plan Task 11): takeover, the ordering rule after an
//! acquisition, a folder that vanishes and returns, confirmed takeover on sync folders.

mod integration_support;

use std::fs;

use fin_store::lease::LeaseState;
use fin_store::{MAIN, ReadOnlyReason};
use integration_support::*;

#[test]
fn takeover_after_writer_stops() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let mut b = e.open("b");
    a.close().unwrap(); // the writer stops: nothing is released
    e.advance(44);
    e.poll(&mut b);
    assert_eq!(
        b.read_only(),
        Some(ReadOnlyReason::LeaseHeld),
        "44 s: alive"
    );
    e.advance(2);
    e.poll(&mut b);
    assert_eq!(
        held_epoch(&b),
        None,
        "stale once; the confirming probe comes 5 s later"
    );
    e.advance(5);
    e.poll(&mut b);
    assert_eq!(held_epoch(&b), Some(2), "taken over");
    // A wrote before: B waits out the directory-cache window and imports again first.
    assert_eq!(b.read_only(), Some(ReadOnlyReason::CatchingUp));
    assert_eq!(refused(&mut b, "b1"), "CATCHING_UP");
    e.advance(10);
    e.poll(&mut b);
    assert_eq!(b.read_only(), None);
    let c = commit(&mut b, "b1", vec![insert(0, "a1")]);
    assert_eq!(c.rev, 2);
    e.flush(&mut b);
    assert_eq!(segments(&e.dir).last().unwrap().1.epoch(), 2);
    let f = e.open("f");
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
}

/// Lets `s`'s lease look stopped to others: every heartbeat fails from now on, so it
/// self-fences 20 s later and its record stops changing.
fn freeze(s: &fin_store::Store) {
    s.lease().expect("holding").fail_heartbeats(u32::MAX);
}

/// Polls `s` every 2 s (fake) until it holds the lease; returns the seconds it took.
fn poll_until_held(e: &Env, s: &mut fin_store::Store, max: u64) -> u64 {
    let mut waited = 0;
    while held_epoch(s).is_none() {
        assert!(
            waited <= max,
            "not acquired within {max} s: {:?}",
            s.lease_state()
        );
        e.advance(2);
        waited += 2;
        e.poll(s);
    }
    waited
}

#[test]
fn ordering_rule_imports_before_writing_after_an_acquisition() {
    // A holds (epoch 1) and commits t1, then stalls before exporting it. B takes over
    // (epoch 2) and exports u1 and u2. A's directory listing does not show u1 yet (a
    // directory cache). A re-acquires (epoch 3). Without the ordering rule, a commit t2 on
    // top of t1 at epoch 3 would export t1+t2 labelled 3 and beat B's line, which others
    // already applied. With it, A waits out the cache window, then waits for the gap, then
    // imports: t1 is set aside and t2 goes on top of u2.
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let mut b = e.open("b");
    commit(&mut a, "t1", vec![insert(0, "a1")]);
    freeze(&a);

    let waited = poll_until_held(&e, &mut b, 80);
    assert!(waited >= 46, "never early: {waited} s");
    e.advance(10);
    e.poll(&mut b);
    commit(&mut b, "u1", vec![insert(5, "a1")]);
    e.flush(&mut b);
    commit(&mut b, "u2", vec![insert(6, "a1")]);
    e.flush(&mut b);
    let segs = segments(&e.dir);
    assert_eq!(segs.len(), 3);
    let u1 = segs[1].0.clone();
    fin_store::scan::hide_for_test(&u1);
    drop(b);

    // A: lost (self-fenced), follows, then re-acquires once B looks stopped.
    e.advance(2);
    e.poll(&mut a);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::LeaseHeld));
    poll_until_held(&e, &mut a, 80);
    assert_eq!(held_epoch(&a), Some(3));
    assert_eq!(refused(&mut a, "t2"), "CATCHING_UP");
    // The window passes; u1 is still not listed, so u2 waits for its predecessor.
    e.advance(10);
    e.poll(&mut a);
    let st = a.status();
    assert_eq!(st.waiting.len(), 1, "{st:?}");
    assert_eq!(refused(&mut a, "t2"), "CATCHING_UP");
    assert_eq!(
        folder_tx_ids(&e.dir),
        ["t0", "u1", "u2"],
        "t1 was never exported"
    );

    // The listing catches up: u1, u2 are imported, t1 is set aside, A writes on top of u2.
    fin_store::scan::unhide_for_test(&u1);
    e.advance(2);
    e.poll(&mut a);
    assert_eq!(a.read_only(), None);
    let t2 = commit(&mut a, "t2", vec![insert(7, "a1")]);
    assert_eq!(t2.rev, 4);
    e.flush(&mut a);
    assert_eq!(folder_tx_ids(&e.dir), ["t0", "u1", "u2", "t2"]);
    let st = a.status();
    assert_eq!(st.quarantine.reasons.get("DIVERGED_TAIL"), Some(&1));
    let w = st.warnings.iter().find(|w| w.code == "QUARANTINE").unwrap();
    assert!(
        w.message
            .contains("DIVERGED_TAIL records are this server's own edits")
    );

    // A fresh follower sees one line and sets nothing aside.
    let mut f = e.open("f");
    e.poll(&mut f);
    assert_eq!(f.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    let fs_ = f.status();
    assert_eq!(
        (fs_.quarantine.count, fs_.read_only.map(|r| r.reason)),
        (0, Some(ReadOnlyReason::LeaseHeld))
    );
}

#[test]
fn reacquiring_our_own_lease_needs_no_wait() {
    // Only another server's writes can be hidden by our directory cache.
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    drop(a);
    let mut a = e.open("a");
    assert_eq!(held_epoch(&a), Some(2));
    assert_eq!(a.read_only(), None, "writable as open returns");
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    e.flush(&mut a);
}

#[test]
fn folder_vanishes_then_returns() {
    // Review Focus 2: the folder disappears mid-session (unmounted, renamed).
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let away = e.root.join("away");
    fs::rename(&e.dir, &away).unwrap();
    e.advance(1);
    e.poll(&mut a);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::LeaseLost));
    assert!(matches!(a.lease_state(), LeaseState::Lost { .. }));
    assert_eq!(refused(&mut a, "t2"), "LEASE_LOST");
    let st = a.status();
    assert!(st.import_error.is_some(), "{st:?}");
    for _ in 0..3 {
        e.advance(2);
        e.poll(&mut a);
    }
    assert!(!e.dir.exists(), "nothing recreates the folder");
    assert_eq!(a.head(MAIN), 1, "the database is untouched");

    fs::rename(&away, &e.dir).unwrap();
    e.advance(2);
    e.poll(&mut a);
    assert_eq!(held_epoch(&a), Some(2), "re-acquired at a new epoch");
    assert_eq!(a.read_only(), None);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    e.flush(&mut a);
    let segs = segments(&e.dir);
    assert_eq!(segs.iter().map(|s| s.1.epoch()).collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn takeover_on_a_sync_folder_needs_confirmation() {
    let e = env();
    drop(e.create("a"));
    edit_project_file(&e.dir, |f| {
        f.insert("storageOverride".into(), "sync:dropbox".into());
    });
    let mut a = e.open("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let mut b = e.open("b");
    drop(a);
    // Never automatic on a sync folder, however long the holder is gone.
    for _ in 0..3 {
        e.advance(25);
        e.poll(&mut b);
    }
    assert!(
        matches!(b.lease_state(), LeaseState::Stale { .. }),
        "{:?}",
        b.lease_state()
    );
    assert_eq!(b.read_only(), Some(ReadOnlyReason::LeaseHeld));
    let err = fin_core::CoreError::from(b.takeover(false).unwrap_err());
    assert_eq!(err.code, "LEASE_TAKEOVER_REFUSED");
    assert_eq!(err.details["reason"], "CONFIRM_REQUIRED");
    let st = b.takeover(true).unwrap();
    assert!(matches!(st.lease, LeaseState::Held { .. }), "{st:?}");
    assert_eq!(
        st.read_only.map(|r| r.reason),
        Some(ReadOnlyReason::CatchingUp)
    );
    e.advance(10);
    e.poll(&mut b);
    assert_eq!(b.read_only(), None);
    commit(&mut b, "b1", vec![insert(0, "a1")]);
}

// ---- Review of 546b7f6: a restarted process re-takes its own lease (server-data lock) ----

/// Forgets what this process knows about its leases on the project folder, as a new process
/// would.
fn restart(e: &Env) {
    fin_store::lease::forget_process_for_test(&e.dir.join(".finisher"));
}

#[test]
fn a_restarted_server_retakes_its_own_lease_at_once() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    a.close().unwrap();
    restart(&e);
    let mut a = e.open("a");
    assert_eq!(held_epoch(&a), Some(2), "{:?}", a.lease_state());
    assert_eq!(a.read_only(), None);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
}

#[test]
fn a_second_process_on_the_same_server_data_follows() {
    let e = env();
    let a = e.create("a");
    restart(&e); // "another process" from here on
    let mut a2 = e.open("a");
    assert_eq!(
        a2.read_only(),
        Some(ReadOnlyReason::LeaseHeld),
        "{:?}",
        a2.lease_state()
    );
    assert_eq!(refused(&mut a2, "x"), "LEASE_HELD");
    // The first process stops: its lock goes with it, and the second re-takes at once.
    drop(a);
    e.poll(&mut a2);
    assert_eq!(held_epoch(&a2), Some(2));
    assert_eq!(a2.read_only(), None);
}

#[test]
fn a_server_data_lock_held_by_someone_else_means_follow() {
    let e = env();
    let a = e.create("a");
    let lock_path = a.db_path().with_file_name("lease.lock");
    drop(a);
    restart(&e);
    let other = fs::File::open(&lock_path).unwrap();
    other.try_lock().unwrap();
    let mut a = e.open("a");
    assert_eq!(a.read_only(), Some(ReadOnlyReason::LeaseHeld));
    other.unlock().unwrap();
    e.poll(&mut a);
    assert_eq!(held_epoch(&a), Some(2));
}

// ---- Review of 546b7f6: catch-up gate on sync folders and after a missing lease.json ----

#[test]
fn on_a_sync_folder_catching_up_waits_for_files_still_arriving() {
    let e = env();
    drop(e.create("a"));
    edit_project_file(&e.dir, |f| {
        f.insert("storageOverride".into(), "sync:dropbox".into());
    });
    let mut a = e.open("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let mut b = e.open("b");
    drop(a);
    // A segment the sync client is still writing.
    let arriving = seg_dir(&e.dir).join("00000001-000000000002-000000000002-abcdefabcdef.fseg");
    fs::write(&arriving, br#"{"magic":"finisher-segment""#).unwrap();
    b.takeover(true).unwrap();
    e.advance(12);
    e.poll(&mut b);
    assert_eq!(b.read_only(), Some(ReadOnlyReason::CatchingUp));
    let st = b.status();
    assert!(
        st.warnings.iter().any(|w| w.code == "SYNC_ARRIVING"),
        "{st:?}"
    );
    fs::remove_file(&arriving).unwrap();
    e.advance(2);
    e.poll(&mut b);
    assert_eq!(b.read_only(), None);
}

#[test]
fn a_missing_lease_json_still_waits_when_another_server_wrote() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    drop(a);
    let dot = e.dir.join(".finisher");
    for f in fs::read_dir(&dot).unwrap() {
        let p = f.unwrap().path();
        if p.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("lease.")
        {
            fs::remove_file(p).unwrap();
        }
    }
    let mut b = e.open("b");
    assert_eq!(
        held_epoch(&b),
        Some(2),
        "free: taken, above the segment epoch"
    );
    assert_eq!(b.read_only(), Some(ReadOnlyReason::CatchingUp));
    e.advance(10);
    e.poll(&mut b);
    assert_eq!(b.read_only(), None);
}

// ---- Re-review: a cloned server data is another server ----

#[test]
fn a_cloned_server_data_is_another_server_and_follows() {
    // `cp -a` of a live holder's server data on the same host (or a cloned VM): the clone
    // must not take the lease as "its own" and ping-pong it with the original.
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    copy_dir(&e.data("a"), &e.data("clone"));
    let mut c = e.open("clone");
    assert_ne!(
        c.server_id(),
        a.server_id(),
        "the clone gets a server id of its own"
    );
    assert_eq!(
        c.read_only(),
        Some(ReadOnlyReason::LeaseHeld),
        "{:?}",
        c.lease_state()
    );
    assert!(
        c.status()
            .warnings
            .iter()
            .any(|w| w.code == "SERVER_ID_RENEWED"),
        "{:?}",
        c.status().warnings
    );
    for _ in 0..3 {
        e.advance(2);
        e.poll(&mut a);
        e.poll(&mut c);
    }
    assert_eq!((held_epoch(&a), held_epoch(&c)), (Some(1), None));
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    // The original keeps its id across reopens.
    let id = a.server_id().to_string();
    drop(a);
    restart(&e);
    let a = e.open("a");
    assert_eq!(a.server_id(), id);
    assert_eq!(held_epoch(&a), Some(2));
}
