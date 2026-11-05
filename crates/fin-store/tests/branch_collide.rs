//! The same branch id made on two servers across a takeover (plan 4B Decision 33): the
//! `branch` record with the higher `epoch`, then the lower `txHash`, holds the id; the other
//! is an inert row on its parent's chain, and its branch's segments (which name their
//! `branch` record) are set aside as `ID_COLLISION`. Every follower converges whatever the
//! arrival order, and the parent chains keep following.
//!
//! Real-lease scenarios as `tests/cascade.rs`: A (epoch 1) writes, pauses, B takes over
//! (epoch 2); A's late files reach the follower F before B's. Test-lease scenarios as
//! `tests/branch_follow.rs`.

mod integration_support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fin_core::{CommittedTx, Op, Rational, SCHEMA_VERSION, Tx};
use fin_store::{BranchCreate, MAIN, ReadOnlyReason, Store, db_path};
use integration_support::*;
use rusqlite::Connection;
use tempfile::TempDir;

fn on(s: &mut Store, id: &str, branch: &str, ops: Vec<Op>) -> CommittedTx {
    let base = s.head(branch);
    s.commit(Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: branch.into(),
        base_rev: base,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    })
    .unwrap()
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
    })
    .unwrap()
    .branch
    .id
}

fn db(s: &Store) -> Connection {
    Connection::open(s.db_path()).unwrap()
}

/// Every branch's state at its head.
fn states(s: &Store) -> Vec<(String, u64, fin_core::Project)> {
    s.branches(true)
        .unwrap()
        .into_iter()
        .map(|b| {
            let (rev, p) = s.snapshot(&b.id).unwrap();
            (b.id, rev, p)
        })
        .collect()
}

/// `(id, name, parent)` of every branch in the index.
fn names(s: &Store) -> Vec<(String, String, Option<String>)> {
    s.branches(true)
        .unwrap()
        .into_iter()
        .map(|b| (b.id, b.name, b.parent))
        .collect()
}

/// The `txHash`es set aside now.
fn aside(s: &Store) -> BTreeSet<String> {
    s.quarantined_for_test().into_iter().map(|q| q.0).collect()
}

/// `(txId, reason)` of the current quarantine rows, sorted.
fn reasons(s: &Store) -> Vec<(String, String)> {
    let c = db(s);
    let mut stmt = c
        .prepare("SELECT record_bytes, reason FROM quarantine WHERE current = 1")
        .unwrap();
    let mut out: Vec<(String, String)> = stmt
        .query_map([], |r| {
            let bytes: Vec<u8> = r.get(0)?;
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            Ok((v["txId"].as_str().unwrap().to_string(), r.get(1)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    out.sort();
    out
}

/// `(txId, branch, seq, kind, ignored)` of every `record_links` row.
type Link = (String, String, i64, String, Option<String>);

fn links(s: &Store) -> Vec<Link> {
    let c = db(s);
    let mut stmt = c
        .prepare("SELECT tx_id, branch, seq, kind, ignored FROM record_links ORDER BY branch, seq")
        .unwrap();
    stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn ignored(s: &Store) -> Vec<String> {
    links(s)
        .into_iter()
        .filter(|l| l.4.is_some())
        .map(|l| l.0)
        .collect()
}

fn warns(s: &Store, code: &str) -> bool {
    s.status().warnings.iter().any(|w| w.code == code)
}

// ---- real lease ----------------------------------------------------------------------------

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
    b: Store,
    f: Store,
}

/// As `tests/cascade.rs`: A (epoch 1) commits `t0` and `before`, exports; F and B follow. A
/// pauses, B takes over (epoch 2). A's `late` writes (epoch 1) are exported and F applies
/// them before B, whose listing does not show them, writes `won` and exports. Then every
/// file is visible.
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
    drop(a);
    Split { b, f }
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
    let first = segments(&e.dir)[0]
        .0
        .strip_prefix(&root)
        .unwrap()
        .to_path_buf();
    let rest: Vec<&PathBuf> = files.iter().filter(|p| **p != first).collect();
    let g = e.open("g");
    let want = |s: &Store| (states(s), aside(s), names(s), links(s));
    let target = want(&g);
    assert_eq!(target.1.len(), n_aside, "{:?}", reasons(&g));

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

/// Review E4: A's late line (its `br1` and an edit on it) reaches F first; B's own `br1`
/// forks at the same point and has no edit of its own. F ends as B: B's empty br1, A's
/// records set aside.
#[test]
fn late_same_id_seen_first_converges() {
    let e = env();
    let Split { mut b, mut f } = split(
        &e,
        |_| {},
        |a| {
            assert_eq!(branch(a, "xb", "x-side", MAIN), "br1");
            on(a, "x1", "br1", vec![insert(0, "a1")]);
        },
        |b| assert_eq!(branch(b, "yb", "y-side", MAIN), "br1"),
    );
    assert_eq!(f.head("br1"), 2, "F applied A's late line");
    e.poll(&mut f);
    e.poll(&mut b);
    assert_eq!(states(&f), states(&b));
    assert_eq!(names(&f), names(&b));
    assert_eq!(f.head("y-side"), 1, "nothing of A's on B's br1");
    assert_eq!(aside(&f), aside(&b));
    assert_eq!(f.read_only(), Some(ReadOnlyReason::LeaseHeld));
    // B edits its br1; F follows.
    on(&mut b, "y1", "br1", vec![insert(3, "a1")]);
    e.flush(&mut b);
    e.poll(&mut f);
    assert_eq!(states(&f), states(&b));
    all_orders_converge(&e, 5, 2);
}

/// Task 10's reused-id case, every order (x1, yb, xb, y1 one poll each, …): A's late `br1`
/// with an edit, B's `br1` with an edit, same parent and fork point.
#[test]
fn reused_id_converges_in_every_arrival_order() {
    let e = env();
    split(
        &e,
        |_| {},
        |a| {
            branch(a, "xb", "x-side", MAIN);
            on(a, "x1", "br1", vec![insert(0, "a1")]);
        },
        |b| {
            branch(b, "yb", "y-side", MAIN);
            on(b, "y1", "br1", vec![insert(5, "a1")]);
        },
    );
    all_orders_converge(&e, 5, 2);
}

/// Review E1 (B1): the same id on different parents. A's late `br2` is made on `br1`, on a
/// line nobody competes on, so it is applied; B's `br2` on `main` (epoch 2) holds the id. A's
/// record stays an inert row on br1 (br1 keeps following), its branch's edit is
/// `ID_COLLISION`, in every order.
#[test]
fn same_id_on_different_parents_converges_in_every_order() {
    let e = env();
    let Split { mut b, mut f } = split(
        &e,
        |a| assert_eq!(branch(a, "xs", "side", MAIN), "br1"),
        |a| {
            assert_eq!(branch(a, "xb", "x-deep", "br1"), "br2");
            on(a, "x2", "br2", vec![insert(0, "a1")]);
        },
        |b| {
            assert_eq!(branch(b, "yb", "y-two", MAIN), "br2");
            on(b, "y2", "br2", vec![insert(4, "a1")]);
        },
    );
    assert_eq!(
        f.branches(true).unwrap()[2].name,
        "x-deep",
        "F followed A first"
    );
    e.poll(&mut f);
    e.poll(&mut b);
    assert_eq!(states(&f), states(&b));
    assert_eq!(
        names(&f)[2],
        ("br2".into(), "y-two".into(), Some(MAIN.into()))
    );
    assert_eq!(
        reasons(&f),
        [("x2".to_string(), "ID_COLLISION".to_string())]
    );
    assert_eq!(reasons(&b), reasons(&f));
    assert_eq!(ignored(&f), ["xb"]);
    assert_eq!(ignored(&b), ["xb"]);
    assert!(warns(&f, "BRANCH_RECORDS_IGNORED"));
    assert!(
        f.head("br1") == 2 && b.head("br1") == 2,
        "br1 holds the inert record"
    );
    // br1 keeps following.
    on(&mut b, "s1", "br1", vec![insert(9, "a1")]);
    e.flush(&mut b);
    e.poll(&mut f);
    assert_eq!(states(&f), states(&b));
    all_orders_converge(&e, 6, 1);
}

// ---- test lease ------------------------------------------------------------------------

struct T {
    _tmp: TempDir,
    root: PathBuf,
    dir: PathBuf,
}

fn tenv() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    T {
        dir: root.join("My Project"),
        root,
        _tmp: tmp,
    }
}

fn tcreate(t: &T) -> Store {
    Store::create(
        &t.root.join("server-a"),
        &t.dir,
        "Demo",
        Rational::from_int(30),
        None,
    )
    .unwrap()
}

/// Another server whose DB is a copy of `a`'s at rev 0, on the test lease at epoch 0.
fn server(t: &T, a: &Store, name: &str) -> Store {
    let data = t.root.join(format!("server-{name}"));
    let to = db_path(&data, a.project_id());
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    db(a)
        .execute("VACUUM INTO ?1", [to.to_string_lossy()])
        .unwrap();
    let mut s = Store::open(&data, &t.dir).unwrap();
    s.set_test_lease(0, true);
    s
}

fn export(s: &mut Store) -> u32 {
    let at = Instant::now();
    s.tick_report(at).unwrap();
    let r = s.tick_report(at + Duration::from_secs(3)).unwrap().export;
    assert_eq!(r.error, None);
    r.segments_written
}

fn row_hash(s: &Store, tx_id: &str) -> String {
    db(s)
        .query_row("SELECT tx_hash FROM txs WHERE tx_id = ?1", [tx_id], |r| {
            r.get(0)
        })
        .unwrap()
}

/// Review E2 (B2): Y makes `br1` and edits it at epoch 2, re-acquires at 3 before exporting
/// and commits on `main`, so its `branch` record is exported in a segment labelled 3. X's
/// `br1` (epoch 1) lands too. Followers follow Y's br1 by the record its segments name, not
/// by epoch labels; a compaction pack of Y's br1 names the same record.
#[test]
fn reacquired_holder_keeps_its_branch_on_followers() {
    let t = tenv();
    let a = tcreate(&t);
    let mut x = server(&t, &a, "x");
    let mut y = server(&t, &a, "y");
    let mut f = server(&t, &a, "f");
    x.set_test_lease(1, true);
    on(&mut x, "r1", MAIN, vec![asset("/m/a.mov")]);
    export(&mut x);
    y.import().unwrap();
    y.set_test_lease(2, true);
    assert_eq!(branch(&mut y, "yb", "y-side", MAIN), "br1");
    on(&mut y, "y2", "y-side", vec![insert(0, "a1")]);
    assert_eq!(branch(&mut x, "xb", "x-side", MAIN), "br1");
    on(&mut x, "x2", "x-side", vec![insert(5, "a1")]);
    export(&mut x);
    y.set_test_lease(3, true);
    on(&mut y, "y3", MAIN, vec![insert(7, "a1")]);
    y.import().unwrap();
    assert_eq!(export(&mut y), 2);
    let r = f.import().unwrap();
    assert!(r.unknown_branches.is_empty() && r.waiting.is_empty() && r.pending.is_empty());
    assert_eq!(states(&f), states(&y));
    assert_eq!(
        reasons(&f),
        [
            ("x2".to_string(), "STALE_EPOCH".to_string()),
            ("xb".to_string(), "STALE_EPOCH".to_string())
        ]
    );
    on(&mut y, "y4", "y-side", vec![insert(9, "a1")]);
    export(&mut y);
    f.import().unwrap();
    assert_eq!(states(&f), states(&y));

    // Compaction packs Y's br1 (relabelled at its highest epoch): the pack names yb.
    y.set_compaction_grace_for_test(Duration::from_secs(1));
    let yb = row_hash(&y, "yb");
    let br1 = t.dir.join(".finisher").join("segments").join("br1");
    let packs = |dir: &Path| -> Vec<fin_store::segment::Segment> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|e| fin_store::segment::decode(&fs::read(e.path()).ok()?).ok())
            .filter(|s| s.header.pack)
            .collect()
    };
    for _ in 0..3 {
        if !packs(&br1).is_empty() {
            break;
        }
        y.compact(true).unwrap();
    }
    let p = packs(&br1);
    assert_eq!(p.len(), 1, "a pack of Y's br1");
    assert_eq!(p[0].header.branch_record.as_deref(), Some(yb.as_str()));
    assert_eq!(p[0].header.epoch, 3);
    let mut g = server(&t, &a, "g");
    g.import().unwrap();
    assert_eq!(states(&g), states(&y));
    assert_eq!(aside(&g), aside(&f));
}

/// Review E1 on the test lease: X's `br2` on `br1` lands first and alone on F2; Y's `br2` on
/// `main` (epoch 2) wins on every follower, and `main` keeps following.
#[test]
fn same_id_on_different_parents_test_lease() {
    let t = tenv();
    let a = tcreate(&t);
    let mut x = server(&t, &a, "x");
    let mut y = server(&t, &a, "y");
    let mut f1 = server(&t, &a, "f1");
    let mut f2 = server(&t, &a, "f2");
    x.set_test_lease(1, true);
    on(&mut x, "r1", MAIN, vec![asset("/m/a.mov")]);
    assert_eq!(branch(&mut x, "xb1", "side", MAIN), "br1");
    export(&mut x);
    for s in [&mut y, &mut f1, &mut f2] {
        s.import().unwrap();
    }
    y.set_test_lease(2, true);
    assert_eq!(branch(&mut x, "xb2", "x-deep", "br1"), "br2");
    assert_eq!(branch(&mut y, "yb2", "y-two", MAIN), "br2");
    export(&mut x);
    f2.import().unwrap();
    assert_eq!(f2.branches(true).unwrap()[2].name, "x-deep");
    y.import().unwrap();
    export(&mut y);
    on(&mut y, "y3", MAIN, vec![insert(0, "a1")]);
    export(&mut y);
    let r = f2.import().unwrap();
    assert_eq!(r.error, None);
    f1.import().unwrap();
    for s in [&f1, &f2] {
        assert_eq!(states(s), states(&y));
        assert_eq!(names(s), names(&y));
        assert_eq!(ignored(s), ["xb2"]);
        assert_eq!(s.read_only(), None);
        assert!(warns(s, "BRANCH_RECORDS_IGNORED"));
    }
    assert_eq!(f2.head(MAIN), 4, "main keeps following");
}

/// Review (b): a rebuild seeds from the highest folder snapshot below the first `branch`
/// record on the chain, instead of replaying everything.
#[test]
fn rebuild_seeds_below_the_first_branch_record() {
    let t = tenv();
    let a = tcreate(&t);
    let mut x = server(&t, &a, "x");
    let mut f = server(&t, &a, "f");
    x.set_test_lease(1, true);
    x.set_compaction_grace_for_test(Duration::from_secs(1));
    let snaps = |t: &T| -> Vec<String> {
        let dir = t.dir.join(".finisher").join("snapshots").join(MAIN);
        let mut v: Vec<String> = fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    };
    on(&mut x, "r1", MAIN, vec![asset("/m/a.mov")]);
    export(&mut x);
    on(&mut x, "r2", MAIN, vec![insert(0, "a1")]);
    export(&mut x);
    x.compact(true).unwrap();
    assert_eq!(snaps(&t).len(), 1, "a snapshot at seq 2");
    branch(&mut x, "xb", "side", MAIN);
    export(&mut x);
    for i in 4..=5 {
        on(&mut x, &format!("r{i}"), MAIN, vec![insert(i, "a1")]);
        export(&mut x);
    }
    // The first compaction's grace period passes, then the next plans seqs 3–5.
    for _ in 0..4 {
        if snaps(&t).len() == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1100));
        x.compact(true).unwrap();
    }
    assert_eq!(
        snaps(&t).len(),
        2,
        "and one at seq 5, above the branch record"
    );
    f.import().unwrap();
    assert_eq!(states(&f), states(&x));
    let rep = f.rebuild().unwrap();
    assert_eq!(rep.seeded_from_snapshot, Some(2));
    assert_eq!(states(&f), states(&x));
    assert_eq!(names(&f), names(&x));
}
