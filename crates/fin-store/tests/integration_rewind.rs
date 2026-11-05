//! Rewinding an applied line that lost (plan Task 11b; `archive` decision "AUTO-REWIND"):
//! a follower that applied a late write of a fenced holder rewinds to the winner and ends up
//! where every other follower is; the same `Rewound` event and branch generation report a
//! diverged local tail.

mod integration_support;

use std::fs;
use std::path::{Path, PathBuf};

use fin_core::{CoreError, SCHEMA_VERSION, Tx};
use fin_store::conflict_names::Provider;
use fin_store::lease::MonoClock;
use fin_store::storage_kind::StorageKind;
use fin_store::{MAIN, ReadOnlyReason, Store, StoreEvent, TxStatus};
use integration_support::*;

/// Generation `n` of `s`'s database.
fn gen_of(s: &Store, n: u64) -> fin_store::Generation {
    fin_store::Generation {
        incarnation: s.incarnation(),
        n,
    }
}

/// Lets `s`'s lease look stopped to others: every heartbeat fails from now on.
fn freeze(s: &Store) {
    s.lease().expect("holding").fail_heartbeats(u32::MAX);
}

fn poll_until_held(e: &Env, s: &mut Store, max: u64) {
    let mut waited = 0;
    while held_epoch(s).is_none() {
        assert!(waited <= max, "not acquired: {:?}", s.lease_state());
        e.advance(2);
        waited += 2;
        e.poll(s);
    }
}

struct Split {
    a: Store,
    b: Store,
    f: Store,
}

/// A (epoch 1) creates the project and exports `common` records; F follows. A pauses, B
/// takes over (epoch 2). A's late export of `lost` (still labelled epoch 1; one segment per
/// record when `each`) reaches F before B writes. B, whose listing does not show A's late
/// files, commits `won` on top of `common` (at `b_epoch` when given: a test lease no lease
/// file backs) and exports them. Then everyone sees every file.
fn split(
    e: &Env,
    common: usize,
    lost: &[&str],
    each: bool,
    won: &[&str],
    b_epoch: Option<u64>,
) -> Split {
    let mut a = e.create("a");
    for i in 0..common {
        commit(
            &mut a,
            &format!("t0-{i}"),
            vec![asset(&format!("/m/{i}.mov"))],
        );
    }
    e.flush(&mut a);
    let mut f = e.open("f");
    let mut b = e.open("b");
    freeze(&a);
    poll_until_held(e, &mut b, 80);
    assert_eq!(held_epoch(&b), Some(2));
    e.advance(10);
    e.poll(&mut b);
    assert_eq!(b.read_only(), None);

    // A's late write: its rename lands after the takeover.
    a.set_test_lease(1, true);
    let before = segments(&e.dir).len();
    for id in lost {
        commit(&mut a, id, vec![asset(&format!("/m/{id}.mov"))]);
        if each {
            e.flush(&mut a);
        }
    }
    e.flush(&mut a);
    let late: Vec<PathBuf> = segments(&e.dir)[before..]
        .iter()
        .map(|s| s.0.clone())
        .collect();
    e.poll(&mut f);
    assert_eq!(
        f.head(MAIN),
        (common + lost.len()) as u64,
        "F applied the late line"
    );

    for p in &late {
        fin_store::scan::hide_for_test(p);
    }
    if let Some(epoch) = b_epoch {
        b.set_test_lease(epoch, true);
    }
    for id in won {
        commit(&mut b, id, vec![insert(0, "a1")]);
    }
    e.flush(&mut b);
    for p in &late {
        fin_store::scan::unhide_for_test(p);
    }
    Split { a, b, f }
}

fn rewound(events: &[StoreEvent]) -> Vec<(String, u64, u32, bool, u64)> {
    events
        .iter()
        .filter_map(|ev| match ev {
            StoreEvent::Rewound {
                branch,
                from_rev,
                quarantined,
                own,
                generation,
            } => Some((branch.clone(), *from_rev, *quarantined, *own, *generation)),
            _ => None,
        })
        .collect()
}

fn applied(events: &[StoreEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|ev| match ev {
            StoreEvent::Applied(c) => Some(c.tx.tx_id.clone()),
            _ => None,
        })
        .collect()
}

/// `(txHash, reason, origin)` of the records set aside now.
fn aside(s: &Store) -> Vec<String> {
    s.quarantined_for_test().into_iter().map(|q| q.0).collect()
}

fn warning<'a>(st: &'a fin_store::ProjectStatus, code: &str) -> Option<&'a str> {
    st.warnings
        .iter()
        .find(|w| w.code == code)
        .map(|w| w.message.as_str())
}

#[test]
fn follower_that_applied_a_late_line_rewinds_and_converges() {
    let e = env();
    let Split { a, b, mut f } = split(&e, 1, &["t1"], false, &["u1"], None);
    let r = e.poll(&mut f);
    assert_eq!(
        rewound(&r.events),
        [(MAIN.to_string(), 2, 1, false, 1)],
        "{:?}",
        r.events
    );
    assert_eq!(
        applied(&r.events),
        ["u1"],
        "the rewind comes before the winner"
    );
    assert_eq!(
        f.read_only(),
        Some(ReadOnlyReason::LeaseHeld),
        "not CHAIN_FORK"
    );
    assert_eq!(f.generation(MAIN), 1);
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());

    // G saw both lines at once: the same state and the same records set aside.
    let g = e.open("g");
    assert_eq!(g.snapshot(MAIN).unwrap(), f.snapshot(MAIN).unwrap());
    assert_eq!(aside(&g), aside(&f));
    assert_eq!(aside(&f).len(), 1);
    assert_eq!(g.generation(MAIN), 0, "G never rewound");
    let q = f.quarantined_for_test();
    assert_eq!((q[0].1.as_str(), q[0].2.as_str()), ("SUPERSEDED", "folder"));

    let st = f.status();
    assert_eq!(st.branches[0].generation, 1);
    let w = warning(&st, "REWOUND").expect("rewind warning");
    assert!(w.contains("1 record"), "{w}");
    assert!(w.contains("epoch 1") && w.contains("epoch 2"), "{w}");
    assert!(w.contains(a.server_id()), "the writer: {w}");
    assert!(warning(&st, "SPLIT_BRAIN_SUSPECTED").is_none());
    assert!(matches!(
        f.tx_status("t1").unwrap(),
        TxStatus::SetAside { ref reason, .. } if reason == "SUPERSEDED"
    ));
    assert!(matches!(f.tx_status("u1").unwrap(), TxStatus::Committed(_)));
    assert!(matches!(f.tx_status("nope").unwrap(), TxStatus::Unknown));

    // Acknowledged, the warning goes; the records stay set aside.
    assert!(!f.acknowledge_rewind(MAIN, 7).unwrap());
    assert!(f.acknowledge_rewind(MAIN, 1).unwrap());
    assert!(warning(&f.status(), "REWOUND").is_none());
    assert_eq!(f.status().quarantine.count, 1);

    // Stable: the next poll rewinds nothing and changes nothing.
    let r = e.poll(&mut f);
    assert!(rewound(&r.events).is_empty());
    assert_eq!(f.generation(MAIN), 1);
    assert_eq!(aside(&f), aside(&g));
}

#[test]
fn own_lost_records_are_rewound_too_and_reported_as_own() {
    let e = env();
    let Split { mut a, b, .. } = split(&e, 1, &["t1"], false, &["u1"], None);
    // A also has an unexported edit on top of its lost line.
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    // Still holding epoch 1, its lost line holds epoch 1: A has itself lost, no rewind.
    let r = e.poll(&mut a);
    assert!(rewound(&r.events).is_empty());
    assert_eq!(a.read_only(), Some(ReadOnlyReason::ChainFork));
    a.set_test_lease(1, false); // A's fence catches up: it has lost the lease
    let r = e.poll(&mut a);
    assert_eq!(rewound(&r.events), [(MAIN.to_string(), 2, 2, true, 1)]);
    assert_eq!(a.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    let mut q: Vec<(String, String)> = a
        .quarantined_for_test()
        .into_iter()
        .map(|q| (q.1, q.2))
        .collect();
    q.sort();
    assert_eq!(
        q,
        [
            ("DIVERGED_TAIL".to_string(), "local".to_string()),
            ("SUPERSEDED".to_string(), "local".to_string())
        ]
    );
    let st = a.status();
    let w = warning(&st, "REWOUND").expect("rewind warning");
    assert!(w.contains("this server's own"), "{w}");

    // A writes again later (a new epoch): ids the lost records used are never reused, and
    // a commit made against the rewound history is refused.
    a.set_test_lease(3, true);
    let tx = |id: &str, base_rev: u64| Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        ops: vec![asset(&format!("/m/{id}.mov"))],
    };
    let e1: CoreError = a
        .commit_at_generation(tx("x1", 2), Some(&gen_of(&a, 0)))
        .unwrap_err()
        .into();
    assert_eq!(e1.code, "HISTORY_REWOUND", "{e1:?}");
    assert_eq!(e1.details["rev"], 2);
    assert_eq!(e1.details["branch"], MAIN);
    let ok = a
        .commit_at_generation(tx("x2", 1), Some(&gen_of(&a, 0)))
        .unwrap();
    assert_eq!(ok.rev, 3, "a base before the rewind point rebases");
    assert_eq!(ok.bindings["$a"], "a3", "a2 belonged to a lost record");
    let ok = a
        .commit_at_generation(tx("x3", 3), Some(&gen_of(&a, 1)))
        .unwrap();
    assert_eq!(ok.rev, 4);
    let e2: CoreError = a
        .commit_at_generation(tx("x4", 0), Some(&gen_of(&a, 7)))
        .unwrap_err()
        .into();
    assert_eq!(e2.code, "HISTORY_REWOUND", "an unknown generation");
}

#[test]
fn a_winner_no_lease_file_backs_stays_a_chain_fork() {
    let e = env();
    let Split { b, mut f, .. } = split(&e, 1, &["t1"], false, &["u1"], Some(5));
    let r = e.poll(&mut f);
    assert!(rewound(&r.events).is_empty());
    assert_eq!(f.read_only(), Some(ReadOnlyReason::ChainFork));
    assert_eq!(f.generation(MAIN), 0);
    assert_eq!(f.head(MAIN), 2, "nothing un-applied");
    // A claim at the winner's epoch backs it.
    let claim = e.dir.join(".finisher").join("lease.claim-00000005");
    fs::write(&claim, b.server_id()).unwrap();
    let r = e.poll(&mut f);
    assert_eq!(rewound(&r.events).len(), 1, "{:?}", r.events);
    assert_eq!(f.head(MAIN), 2);
    assert_ne!(f.read_only(), Some(ReadOnlyReason::ChainFork));
}

#[test]
fn sync_folders_never_rewind() {
    let e = env();
    let Split { mut f, .. } = split(&e, 1, &["t1"], false, &["u1"], None);
    f.set_test_storage_kind(StorageKind::Sync(Provider::Dropbox));
    let r = e.poll(&mut f);
    assert!(rewound(&r.events).is_empty());
    let fork = r.imports[0].fork.clone().expect("fork");
    assert_eq!(fork.reason, fin_store::chain::ForkReason::SyncFolder);
    assert_eq!(f.read_only(), Some(ReadOnlyReason::ChainFork));
    assert_eq!(f.head(MAIN), 2);
}

#[test]
fn rewind_across_a_snapshot_boundary() {
    let e = env();
    let common = fin_store::SNAPSHOT_EVERY as usize - 1;
    let Split { b, mut f, .. } = split(&e, common, &["t1", "t2"], false, &["u1"], None);
    let r = e.poll(&mut f);
    assert_eq!(rewound(&r.events), [(MAIN.to_string(), 200, 2, false, 1)]);
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    // The snapshot at 200 is the winner's: a restart rebuilds the same head from it.
    drop(f);
    let f = e.open("f");
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
}

#[test]
fn the_winner_may_reuse_a_lost_tx_id() {
    let e = env();
    let Split { b, mut f, .. } = split(&e, 1, &["t1"], false, &["t1"], None);
    let r = e.poll(&mut f);
    assert_eq!(rewound(&r.events).len(), 1);
    assert_eq!(applied(&r.events), ["t1"]);
    assert_eq!(
        f.read_only(),
        Some(ReadOnlyReason::LeaseHeld),
        "no TxIdReused fork"
    );
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    assert!(matches!(f.tx_status("t1").unwrap(), TxStatus::Committed(c) if c.rev == 2));
}

#[test]
fn a_long_lost_line_is_reported_as_split_brain() {
    let e = env();
    let Split { mut f, .. } = split(&e, 1, &["t1", "t2"], true, &["u1"], None);
    let r = e.poll(&mut f);
    assert_eq!(rewound(&r.events), [(MAIN.to_string(), 2, 2, false, 1)]);
    let st = f.status();
    let w = warning(&st, "SPLIT_BRAIN_SUSPECTED").expect("escalated");
    assert!(w.contains("2 segment"), "{w}");
    assert!(f.acknowledge_rewind(MAIN, 1).unwrap());
    assert!(warning(&f.status(), "SPLIT_BRAIN_SUSPECTED").is_none());
}

#[test]
fn a_diverged_tail_is_a_rewind_too() {
    let e = env();
    let Split { mut a, b, .. } = split(&e, 1, &[], false, &["u1"], None);
    commit(&mut a, "t1", vec![insert(3, "a1")]);
    a.set_test_lease(1, false);
    let r = e.poll(&mut a);
    assert_eq!(rewound(&r.events), [(MAIN.to_string(), 2, 1, true, 1)]);
    assert_eq!(applied(&r.events), ["u1"]);
    assert_eq!(a.generation(MAIN), 1);
    assert_eq!(a.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    assert!(matches!(
        a.tx_status("t1").unwrap(),
        TxStatus::SetAside { ref reason, ref origin, .. }
            if reason == "DIVERGED_TAIL" && origin == "local"
    ));
}

/// Copies `src` (a project folder) without its segment files.
fn copy_without_segments(src: &Path, to: &Path) {
    copy_dir(src, to);
    for (p, _, _) in segments(to) {
        fs::remove_file(p).unwrap();
    }
}

/// Every order in which the segments after the first can arrive at a follower (one poll per
/// arrival) ends with the state and quarantine of a follower that saw them all at once, and
/// never in `CHAIN_FORK`. Exhaustive (the space is small; a proptest would sample it).
fn all_orders_converge(e: &Env, n_segments: usize) {
    let names: Vec<String> = segments(&e.dir)
        .iter()
        .map(|s| s.0.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), n_segments, "{names:?}");
    let g = e.open("g");
    let want = (g.snapshot(MAIN).unwrap(), aside(&g));

    let rest: Vec<usize> = (1..names.len()).collect();
    let mut orders = vec![vec![]];
    for _ in 0..rest.len() {
        orders = orders
            .into_iter()
            .flat_map(|o: Vec<usize>| {
                rest.iter()
                    .filter(|i| !o.contains(i))
                    .map(|&i| {
                        let mut n = o.clone();
                        n.push(i);
                        n
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    assert_eq!(orders.len(), (1..n_segments).product::<usize>());
    for (k, order) in orders.iter().enumerate() {
        let dir = e.root.join(format!("order-{k}"));
        copy_without_segments(&e.dir, &dir);
        let seg = seg_dir(&dir);
        fs::copy(seg_dir(&e.dir).join(&names[0]), seg.join(&names[0])).unwrap();
        let mut x = e.try_open_at(&format!("x{k}"), &dir).unwrap();
        for &i in order {
            fs::copy(seg_dir(&e.dir).join(&names[i]), seg.join(&names[i])).unwrap();
            e.poll(&mut x);
        }
        let names_of = || order.iter().map(|&i| &names[i]).collect::<Vec<_>>();
        assert_eq!(
            (x.snapshot(MAIN).unwrap(), aside(&x)),
            want,
            "order {:?}",
            names_of()
        );
        assert_ne!(
            x.read_only(),
            Some(ReadOnlyReason::ChainFork),
            "{:?}",
            names_of()
        );
    }
}

#[test]
fn every_arrival_order_ends_in_the_same_state() {
    let e = env();
    let Split { mut b, .. } = split(&e, 1, &["t1"], false, &["u1"], None);
    commit(&mut b, "u2", vec![insert(2, "a1")]);
    e.flush(&mut b);
    all_orders_converge(&e, 4);
}

#[test]
fn every_arrival_order_of_two_lost_and_two_winning_segments_converges() {
    let e = env();
    let Split { mut b, .. } = split(&e, 1, &["t1", "t2"], true, &["u1"], None);
    commit(&mut b, "u2", vec![insert(2, "a1")]);
    e.flush(&mut b);
    all_orders_converge(&e, 5);
}

#[test]
fn a_follower_that_takes_the_lease_still_rewinds_an_older_lost_line() {
    // F applied A's late epoch-1 line; B's epoch-2 line is not listed for F yet when B stops
    // and F takes the lease (epoch 3). When B's files appear, F (holding) rewinds: the lost
    // range holds only epoch 1, below its own.
    let e = env();
    let Split { b, mut f, .. } = split(&e, 1, &["t1"], false, &["u1"], None);
    let won: Vec<PathBuf> = segments(&e.dir)
        .iter()
        .filter(|s| s.1.epoch() == 2)
        .map(|s| s.0.clone())
        .collect();
    assert_eq!(won.len(), 1);
    for p in &won {
        fin_store::scan::hide_for_test(p);
    }
    let want = b.snapshot(MAIN).unwrap();
    drop(b); // B stops
    poll_until_held(&e, &mut f, 80);
    assert_eq!(held_epoch(&f), Some(3));
    assert_eq!(f.head(MAIN), 2, "still on the late line");
    for p in &won {
        fin_store::scan::unhide_for_test(p);
    }
    let r = e.poll(&mut f);
    assert_eq!(rewound(&r.events), [(MAIN.to_string(), 2, 1, false, 1)]);
    assert_ne!(f.read_only(), Some(ReadOnlyReason::ChainFork));
    assert_eq!(f.snapshot(MAIN).unwrap(), want);
    e.advance(10);
    e.poll(&mut f);
    assert_eq!(f.read_only(), None, "caught up");
    assert_eq!(commit(&mut f, "f1", vec![insert(5, "a1")]).rev, 3);
}

#[test]
fn a_failed_rewind_rolls_back_and_a_later_pass_rewinds() {
    let e = env();
    let Split { b, mut f, .. } = split(&e, 1, &["t1"], false, &["u1"], None);
    f.fail_next_rewind_for_test();
    f.poll_now_for_test();
    let err: CoreError = f.tick_report(e.clock.now()).unwrap_err().into();
    assert_eq!(err.code, "TEST_FAULT");
    assert_eq!(f.head(MAIN), 2);
    assert_eq!(f.generation(MAIN), 0);
    assert!(aside(&f).is_empty());
    assert!(matches!(f.tx_status("t1").unwrap(), TxStatus::Committed(_)));
    let r = e.poll(&mut f);
    assert_eq!(rewound(&r.events), [(MAIN.to_string(), 2, 1, false, 1)]);
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
}

#[test]
fn a_second_store_on_the_database_sees_the_rewind() {
    let e = env();
    let Split { b, mut f, .. } = split(&e, 1, &["t1"], false, &["u1"], None);
    // Another Store on F's database (same server data) runs the rewind at its open.
    let f2 = e.open("f");
    assert_eq!(f2.generation(MAIN), 1);
    assert_eq!(f.generation(MAIN), 1);
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    let r = e.poll(&mut f);
    assert!(rewound(&r.events).is_empty(), "rewound once");
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    // F's caller read generation 0 at rev 2: its edit is refused, not rebased.
    f.set_test_lease(9, true);
    let tx = Tx {
        tx_id: "x1".into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev: 2,
        schema_version: SCHEMA_VERSION,
        ops: vec![asset("/m/x1.mov")],
    };
    let err: CoreError = f
        .commit_at_generation(tx, Some(&gen_of(&f, 0)))
        .unwrap_err()
        .into();
    assert_eq!(err.code, "HISTORY_REWOUND", "{err:?}");
}
