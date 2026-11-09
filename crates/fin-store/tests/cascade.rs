//! Rewinds and diverged tails cascade to descendant branches (plan 4B Task 10, Decisions 6–7):
//! a child whose `branch` record is among the lost rows is rewound whole, recursively, on the
//! holder and on every follower, in any arrival order; a child whose `branch` record survives
//! is untouched. The derived tables follow the log (`record_links`, the branch index).
//!
//! Setup as `tests/integration_rewind.rs`: A (epoch 1) writes, pauses, B takes over (epoch 2);
//! A's late files reach the follower F before B's.

mod integration_support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use fin_core::{CommittedTx, CoreError, Op, SCHEMA_VERSION, Tx};
use fin_store::{BranchCreate, BranchSet, MAIN, ReadOnlyReason, Store, StoreEvent};
use integration_support::*;
use rusqlite::Connection;

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

fn tx(id: &str, branch: &str, base_rev: u64, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: branch.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    }
}

fn on(s: &mut Store, id: &str, branch: &str, ops: Vec<Op>) -> CommittedTx {
    let base = s.head(branch);
    s.commit(tx(id, branch, base, ops)).unwrap()
}

/// Makes branch `name` off `from` at its head; returns its id.
fn branch(s: &mut Store, tx_id: &str, name: &str, from: &str) -> String {
    s.create_branch(BranchCreate {
        tx_id: tx_id.into(),
        author: "test".into(),
        name: Some(name.into()),
        from: from.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap()
    .branch
    .id
}

fn rename(s: &mut Store, tx_id: &str, b: &str, name: &str) {
    s.set_branch(BranchSet {
        tx_id: tx_id.into(),
        author: "test".into(),
        branch: b.into(),
        name: Some(name.into()),
        archived: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
}

/// Every segment file of every branch directory.
fn all_segments(dir: &Path) -> Vec<PathBuf> {
    let root = dir.join(".finisher").join("segments");
    let mut out = vec![];
    for d in fs::read_dir(&root).into_iter().flatten().flatten() {
        if d.path().is_dir() {
            for f in fs::read_dir(d.path()).unwrap().flatten() {
                out.push(f.path());
            }
        }
    }
    out.sort();
    out
}

struct Split {
    a: Store,
    b: Store,
    f: Store,
}

/// A (epoch 1) creates the project, commits `t0` (an asset) and `before`, and exports; F and
/// B follow. A pauses, B takes over (epoch 2). A's `late` writes (still epoch 1) are exported
/// and F applies them before B writes. B, whose listing does not show A's late files, writes
/// `won` and exports. Then everyone sees every file.
fn split(
    e: &Env,
    before: impl FnOnce(&mut Store),
    late: impl FnOnce(&mut Store),
    won: impl FnOnce(&mut Store),
) -> Split {
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/0.mov")]);
    before(&mut a);
    e.flush(&mut a);
    let mut f = e.open("f");
    let mut b = e.open("b");
    freeze(&a);
    poll_until_held(e, &mut b, 80);
    assert_eq!(held_epoch(&b), Some(2));
    e.advance(10);
    e.poll(&mut b);
    assert_eq!(b.read_only(), None);

    a.set_test_lease(1, true);
    let had: BTreeSet<PathBuf> = all_segments(&e.dir).into_iter().collect();
    late(&mut a);
    e.flush(&mut a);
    let late_files: Vec<PathBuf> = all_segments(&e.dir)
        .into_iter()
        .filter(|p| !had.contains(p))
        .collect();
    e.poll(&mut f);
    for p in &late_files {
        fin_store::scan::hide_for_test(p);
    }
    won(&mut b);
    e.flush(&mut b);
    for p in &late_files {
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

fn db(s: &Store) -> Connection {
    Connection::open(s.db_path()).unwrap()
}

/// `(txId, branch, reason, origin)` of the current quarantine rows, sorted.
fn quarantine(s: &Store) -> Vec<(String, String, String, String)> {
    let c = db(s);
    let mut stmt = c
        .prepare("SELECT record_bytes, branch, reason, origin FROM quarantine WHERE current = 1")
        .unwrap();
    let mut out: Vec<_> = stmt
        .query_map([], |r| {
            let bytes: Vec<u8> = r.get(0)?;
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            Ok((
                v["txId"].as_str().unwrap().to_string(),
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    out.sort();
    out
}

/// The `txHash`es set aside now.
fn aside(s: &Store) -> BTreeSet<String> {
    s.quarantined_for_test().into_iter().map(|q| q.0).collect()
}

/// `(txId, branch, seq, kind)` of every `record_links` row.
fn links(s: &Store) -> Vec<(String, String, i64, String)> {
    let c = db(s);
    let mut stmt = c
        .prepare("SELECT tx_id, branch, seq, kind FROM record_links ORDER BY branch, seq")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// `(id, name)` of every branch in the index.
fn names(s: &Store) -> Vec<(String, String)> {
    s.branches(true)
        .unwrap()
        .into_iter()
        .map(|b| (b.id, b.name))
        .collect()
}

/// Rows of `branch` in `txs`, `snapshots` and `segments`.
fn rows_of(s: &Store, branch: &str) -> (i64, i64, i64) {
    let c = db(s);
    let n = |t: &str| -> i64 {
        c.query_row(
            &format!("SELECT COUNT(*) FROM {t} WHERE branch = ?1"),
            [branch],
            |r| r.get(0),
        )
        .unwrap()
    };
    (n("txs"), n("snapshots"), n("segments"))
}

fn late_child(a: &mut Store) {
    assert_eq!(branch(a, "xb", "side", MAIN), "br1");
    on(a, "x1", "br1", vec![insert(0, "a1")]);
}

fn winner(b: &mut Store) {
    commit(b, "u1", vec![insert(0, "a1")]);
}

#[test]
fn rewind_cascades_to_child() {
    let e = env();
    let Split { b, mut f, .. } = split(&e, |_| {}, late_child, winner);
    assert_eq!(
        (f.head(MAIN), f.head("br1")),
        (2, 2),
        "F applied the late line"
    );
    let r = e.poll(&mut f);
    assert_eq!(
        rewound(&r.events),
        [
            (MAIN.to_string(), 2, 1, false, 1),
            ("br1".to_string(), 0, 1, false, 1)
        ],
        "{:?}",
        r.events
    );
    assert_eq!(applied(&r.events), ["u1"]);
    assert_ne!(f.read_only(), Some(ReadOnlyReason::ChainFork));
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    assert_eq!(names(&f), [(MAIN.to_string(), MAIN.to_string())]);
    assert_eq!(
        rows_of(&f, "br1"),
        (0, 0, 0),
        "rows, snapshots and segments rows gone"
    );
    assert_eq!(f.generation("br1"), 1);
    let q = |id: &str, b: &str| (id.into(), b.into(), "SUPERSEDED".into(), "folder".into());
    assert_eq!(quarantine(&f), [q("x1", "br1"), q("xb", MAIN)]);
    assert!(f.status().unknown_branches.is_empty());

    // G saw every file at once: the same state, quarantine set and index.
    let g = e.open("g");
    assert_eq!(g.snapshot(MAIN).unwrap(), f.snapshot(MAIN).unwrap());
    assert_eq!(aside(&g), aside(&f));
    assert_eq!(names(&g), names(&f));
    assert_eq!(links(&g), links(&f));

    // Stable: the next poll rewinds and changes nothing.
    let r = e.poll(&mut f);
    assert!(rewound(&r.events).is_empty());
    assert_eq!(quarantine(&f).len(), 2);
    assert_eq!(f.generation("br1"), 1);
}

#[test]
fn rewind_keeps_child_whose_branch_record_survives() {
    let e = env();
    let Split { mut f, .. } = split(
        &e,
        late_child,
        |a| {
            commit(a, "t1", vec![asset("/m/1.mov")]);
            on(a, "x2", "br1", vec![insert(1, "a1")]);
        },
        winner,
    );
    assert_eq!((f.head(MAIN), f.head("br1")), (3, 3));
    let r = e.poll(&mut f);
    assert_eq!(rewound(&r.events), [(MAIN.to_string(), 3, 1, false, 1)]);
    assert_eq!(f.generation("br1"), 0);
    assert_eq!(f.head("br1"), 3, "the child keeps its rows");
    assert_eq!(names(&f).len(), 2);
    let g = e.open("g");
    assert_eq!(g.snapshot(MAIN).unwrap(), f.snapshot(MAIN).unwrap());
    assert_eq!(g.snapshot("br1").unwrap(), f.snapshot("br1").unwrap());
    assert_eq!(aside(&g), aside(&f));
    assert_eq!(names(&g), names(&f));
}

#[test]
fn diverged_tail_with_branch_record_cascades_to_child() {
    let e = env();
    let Split { mut a, b, .. } = split(&e, |_| {}, |_| {}, winner);
    late_child(&mut a); // never exported
    a.set_test_lease(1, false);
    let r = e.poll(&mut a);
    assert_eq!(
        rewound(&r.events),
        [
            (MAIN.to_string(), 2, 1, true, 1),
            ("br1".to_string(), 0, 1, true, 1)
        ]
    );
    assert_eq!(applied(&r.events), ["u1"]);
    assert_eq!(a.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    assert_eq!(names(&a), [(MAIN.to_string(), MAIN.to_string())]);
    assert_eq!(rows_of(&a, "br1"), (0, 0, 0));
    let q = |id: &str, b: &str| (id.into(), b.into(), "DIVERGED_TAIL".into(), "local".into());
    assert_eq!(quarantine(&a), [q("x1", "br1"), q("xb", MAIN)]);
    assert!(
        links(&a).is_empty(),
        "the lost branch record's link is gone"
    );

    // A writes again later: the id the lost record used is never handed out again.
    a.set_test_lease(3, true);
    assert_eq!(branch(&mut a, "zb", "again", MAIN), "br2");
}

#[test]
fn cascade_recurses_to_grandchildren() {
    let e = env();
    let Split { b, mut f, .. } = split(
        &e,
        |_| {},
        |a| {
            late_child(a);
            assert_eq!(branch(a, "xg", "deeper", "br1"), "br2");
            on(a, "x2", "br2", vec![insert(2, "a1")]);
        },
        winner,
    );
    assert_eq!(names(&f).len(), 3, "F applied the late line");
    let r = e.poll(&mut f);
    assert_eq!(
        rewound(&r.events),
        [
            (MAIN.to_string(), 2, 1, false, 1),
            ("br1".to_string(), 0, 2, false, 1),
            ("br2".to_string(), 0, 1, false, 1),
        ]
    );
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    assert_eq!(names(&f).len(), 1);
    assert_eq!(rows_of(&f, "br2"), (0, 0, 0));
    let ids: Vec<String> = quarantine(&f).into_iter().map(|q| q.0).collect();
    assert_eq!(ids, ["x1", "x2", "xb", "xg"]);
    let g = e.open("g");
    assert_eq!(aside(&g), aside(&f));
    assert_eq!(names(&g), names(&f));
}

/// Every branch's state at its head.
fn states(s: &Store) -> Vec<(String, u64, fin_core::Project)> {
    names(s)
        .into_iter()
        .map(|(id, _)| {
            let (rev, p) = s.snapshot(&id).unwrap();
            (id, rev, p)
        })
        .collect()
}

/// Every order in which the segments after the common one can reach a follower (one poll per
/// arrival) ends where a follower that saw them all at once is: the same states, quarantine
/// set, index and links, never `CHAIN_FORK`, nothing waiting. Exhaustive.
fn all_orders_converge(e: &Env, n_files: usize, n_aside: usize) {
    let root = e.dir.join(".finisher").join("segments");
    let files: Vec<PathBuf> = all_segments(&e.dir)
        .into_iter()
        .map(|p| p.strip_prefix(&root).unwrap().to_path_buf())
        .collect();
    assert_eq!(files.len(), n_files, "{files:?}");
    // The common segment (seq 1 on main) is there from the start.
    let first = segments(&e.dir)[0]
        .0
        .strip_prefix(&root)
        .unwrap()
        .to_path_buf();
    let rest: Vec<&PathBuf> = files.iter().filter(|p| **p != first).collect();
    let g = e.open("g");
    let want = |s: &Store| (states(s), aside(s), names(s), links(s));
    let target = want(&g);
    assert_eq!(target.1.len(), n_aside);

    let mut orders: Vec<Vec<usize>> = vec![vec![]];
    for _ in 0..rest.len() {
        orders = orders
            .into_iter()
            .flat_map(|o| {
                (0..rest.len())
                    .filter(|i| !o.contains(i))
                    .map(|i| {
                        let mut n = o.clone();
                        n.push(i);
                        n
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    assert_eq!(orders.len(), (1..=rest.len()).product::<usize>());
    for (k, order) in orders.iter().enumerate() {
        let dir = e.root.join(format!("order-{k}"));
        copy_dir(&e.dir, &dir);
        for p in all_segments(&dir) {
            fs::remove_file(p).unwrap();
        }
        let put = |rel: &Path| {
            let to = dir.join(".finisher").join("segments").join(rel);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(root.join(rel), to).unwrap();
        };
        put(&first);
        let mut x = e.try_open_at(&format!("x{k}"), &dir).unwrap();
        for &i in order {
            put(rest[i]);
            e.poll(&mut x);
        }
        let seen: Vec<&PathBuf> = order.iter().map(|&i| rest[i]).collect();
        assert_eq!(want(&x), target, "order {seen:?}");
        assert_ne!(x.read_only(), Some(ReadOnlyReason::ChainFork), "{seen:?}");
        assert!(x.status().unknown_branches.is_empty(), "{seen:?}");
    }
}

/// The late loser, its child and the winner, in every order.
#[test]
fn cascade_converges_in_every_arrival_order() {
    let e = env();
    split(&e, |_| {}, late_child, winner);
    all_orders_converge(&e, 4, 2);
}

#[test]
fn record_links_follow_rewound_rows() {
    // A's late `branchSet` (a rename at main seq 3) loses to B's `branch` record at the same
    // seq: the link of the lost record goes, B's takes its seq, and the index forgets the
    // rename.
    let e = env();
    let Split { b, mut f, .. } = split(
        &e,
        |a| assert_eq!(branch(a, "xb", "side", MAIN), "br1"),
        |a| rename(a, "xs", "br1", "renamed"),
        |b| assert_eq!(branch(b, "yb", "b-side", MAIN), "br2"),
    );
    assert_eq!(names(&f)[1].1, "renamed");
    let r = e.poll(&mut f);
    assert_eq!(rewound(&r.events), [(MAIN.to_string(), 3, 1, false, 1)]);
    assert_eq!(f.generation("br1"), 0, "its branch record survives");
    assert_eq!(
        names(&f),
        [
            (MAIN.to_string(), MAIN.to_string()),
            ("br1".to_string(), "side".to_string()),
            ("br2".to_string(), "b-side".to_string()),
        ]
    );
    let link = |id: &str, seq: i64| (id.into(), MAIN.into(), seq, "branch".into());
    assert_eq!(links(&f), [link("xb", 2), link("yb", 3)]);
    assert_eq!(links(&b), links(&f));
    let g = e.open("g");
    assert_eq!(names(&g), names(&f));
    assert_eq!(links(&g), links(&f));
}

/// The cascaded child's subscribers learn of it from the child's own `Rewound` event, and a
/// caller holding the child's old generation is refused even when the winner made a branch
/// under the same id at the same fork point.
#[test]
fn subscriber_on_cascaded_branch_gets_history_rewound() {
    let e = env();
    let Split { b, mut f, .. } = split(
        &e,
        |_| {},
        |a| {
            assert_eq!(branch(a, "xb", "a-side", MAIN), "br1");
            on(a, "x1", "br1", vec![insert(0, "a1")]);
        },
        |b| {
            assert_eq!(branch(b, "yb", "b-side", MAIN), "br1");
            on(b, "y1", "br1", vec![insert(4, "a1")]);
        },
    );
    let (old, ops) = f.ops_since_at("br1", 0).unwrap();
    assert_eq!(ops.last().unwrap().tx.tx_id, "x1");
    let r = e.poll(&mut f);
    assert_eq!(
        rewound(&r.events),
        [
            (MAIN.to_string(), 2, 1, false, 1),
            ("br1".to_string(), 0, 1, false, 1)
        ]
    );
    assert_eq!(applied(&r.events), ["yb", "y1"]);
    assert_eq!(names(&f)[1], ("br1".to_string(), "b-side".to_string()));
    assert_eq!(f.snapshot("br1").unwrap(), b.snapshot("br1").unwrap());
    let ids: Vec<(String, String)> = quarantine(&f).into_iter().map(|q| (q.0, q.2)).collect();
    assert_eq!(
        ids,
        [
            ("x1".to_string(), "SUPERSEDED".to_string()),
            ("xb".to_string(), "SUPERSEDED".to_string())
        ]
    );
    let (now, _) = f.ops_since_at("br1", 0).unwrap();
    assert_eq!(now.n, 1);

    // A commit made against the old child is refused, whatever its base.
    f.set_test_lease(9, true);
    let err: CoreError = f
        .commit_at_generation(tx("s1", "br1", 1, vec![insert(1, "a1")]), Some(&old))
        .unwrap_err()
        .into();
    assert_eq!(err.code, "HISTORY_REWOUND", "{err:?}");
    assert_eq!(err.details["rev"], 0);
    let base = f.head("br1");
    let ok = f
        .commit_at_generation(tx("s2", "br1", base, vec![insert(1, "a1")]), Some(&now))
        .unwrap();
    assert_eq!(ok.rev, base + 1);
}

/// A holder at epoch E whose own edits at E sit on a child that a rewind would cascade to has
/// lost itself (rule (b), the cascaded rows included): no rewind, `CHAIN_FORK`, nothing lost.
#[test]
fn holder_with_own_edits_on_a_lost_child_does_not_rewind() {
    let e = env();
    let Split { b, mut f, .. } = split(&e, |_| {}, late_child, winner);
    let won: Vec<PathBuf> = segments(&e.dir)
        .iter()
        .filter(|s| s.1.epoch() == 2)
        .map(|s| s.0.clone())
        .collect();
    assert_eq!(won.len(), 1);
    for p in &won {
        fin_store::scan::hide_for_test(p);
    }
    drop(b);
    poll_until_held(&e, &mut f, 80);
    let epoch = held_epoch(&f).unwrap();
    assert!(epoch >= 3);
    e.advance(10);
    e.poll(&mut f);
    on(&mut f, "f1", "br1", vec![insert(3, "a1")]);
    for p in &won {
        fin_store::scan::unhide_for_test(p);
    }
    let r = e.poll(&mut f);
    assert!(rewound(&r.events).is_empty(), "{:?}", r.events);
    assert_eq!(f.read_only(), Some(ReadOnlyReason::ChainFork));
    assert_eq!(f.head("br1"), 3, "nothing taken out");
    assert_eq!(f.generation("br1"), 0);
}
