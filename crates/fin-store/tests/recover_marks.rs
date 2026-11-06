//! Only a recovery's records mark quarantine rows recovered (plan 4B Decisions 5, 27; final
//! review of 4B): they name the row in `recovers`. An ordinary cherry-pick names its origin
//! root in `originHash`, and that root may be set aside later; it still counts and is
//! offered.
//!
//! Real-lease setup as `tests/cascade.rs`: A (epoch 1) writes, pauses, B takes over
//! (epoch 2); A's late files reach the follower F before B's.

mod integration_support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use fin_core::{CommittedTx, Link, Op, SCHEMA_VERSION, Tx};
use fin_store::{BranchCreate, CherryPickReq, MAIN, RecoverReq, RecoverSel, Store};
use integration_support::*;

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

fn pick(s: &mut Store, tx: &str, origin: &str, into: &str) -> CommittedTx {
    s.cherry_pick(CherryPickReq {
        tx_id: tx.into(),
        author: "test".into(),
        origin: origin.into(),
        into: into.into(),
        dry_run: false,
        args_hash: None,
    })
    .unwrap()
    .committed
    .remove(0)
}

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

/// As `tests/cascade.rs`; returns B and F.
fn split(
    e: &Env,
    before: impl FnOnce(&mut Store),
    late: impl FnOnce(&mut Store),
    won: impl FnOnce(&mut Store),
) -> (Store, Store) {
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/0.mov")]);
    before(&mut a);
    e.flush(&mut a);
    let mut f = e.open("f");
    let mut b = e.open("b");
    a.lease().expect("holding").fail_heartbeats(u32::MAX);
    let mut waited = 0;
    while held_epoch(&b).is_none() {
        assert!(waited <= 80, "not acquired: {:?}", b.lease_state());
        e.advance(2);
        waited += 2;
        e.poll(&mut b);
    }
    e.advance(10);
    e.poll(&mut b);
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
    (b, f)
}

/// The current quarantine rows' txIds, sorted.
fn aside_ids(s: &Store) -> Vec<String> {
    let c = rusqlite::Connection::open(s.db_path()).unwrap();
    let mut stmt = c
        .prepare("SELECT record_bytes FROM quarantine WHERE current = 1")
        .unwrap();
    let mut out: Vec<String> = stmt
        .query_map([], |r| {
            let b: Vec<u8> = r.get(0)?;
            let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
            Ok(v["txId"].as_str().unwrap().to_string())
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    out.sort();
    out
}

/// The review's repro: A's late `br2` (on `br1`) holds x2 and x3, and `br1` holds a pick of
/// x2 (p2); B makes its own `br2`, which wins the id, so x2 and x3 are set aside
/// (`ID_COLLISION`) while p2 stays. Both count, one group holds both, forked where `br2`
/// forked (before p2: the review found it forked at p2, with s1 and without x2).
#[test]
fn pick_of_a_record_set_aside_later_does_not_mark_it_recovered() {
    let e = env();
    let (_b, mut f) = split(
        &e,
        |a| assert_eq!(branch(a, "xs", "side", MAIN), "br1"),
        |a| {
            assert_eq!(branch(a, "xb", "x-deep", "br1"), "br2");
            on(a, "x2", "br2", vec![insert(0, "a1")]);
            on(a, "x3", "br2", vec![insert(0, "a1")]);
            on(a, "s1", "br1", vec![insert(0, "a1")]);
            let p2 = pick(a, "p2", "x2", "br1");
            assert!(matches!(
                p2.link.as_deref(),
                Some(Link::CherryPick {
                    origin_hash: Some(_),
                    recovers: None,
                    ..
                })
            ));
        },
        |b| {
            assert_eq!(branch(b, "yb", "y-two", MAIN), "br2");
            on(b, "y2", "br2", vec![insert(4, "a1")]);
        },
    );
    e.poll(&mut f);
    assert_eq!(aside_ids(&f), ["x2", "x3"]);
    assert_eq!(f.status().quarantine.count, 2, "neither is recovered");
    let groups = f.recover_groups().unwrap();
    assert_eq!(groups.len(), 1, "{groups:#?}");
    let g = &groups[0];
    assert_eq!((g.records, g.records_branch.as_str()), (2, "br2"));
    // br2 forked at br1's fork, t0 on main: the record its first parent names.
    assert_eq!(
        (g.branch.as_str(), g.fork_seq),
        (MAIN, 1),
        "where br2 forked"
    );
    let r = f
        .recover(RecoverReq {
            tx_id: "rc".into(),
            author: "rec".into(),
            create: Some(RecoverSel::All),
            import_dir: None,
            dismiss: None,
            dry_run: true,
        })
        .unwrap();
    assert_eq!(r.created.len(), 1);
    let picked: Vec<&str> = r
        .committed
        .iter()
        .filter_map(|c| match c.link.as_deref() {
            Some(Link::CherryPick { origin, .. }) => Some(origin.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        picked,
        ["x2", "x3"],
        "x2 is offered, s1 is not on the branch"
    );
    assert_eq!(r.created[0].fork_seq, 1);
}

/// An ordinary pick names its root's hash (`originHash`) and never `recovers`, so it marks
/// nothing recovered, whatever is set aside.
#[test]
fn ordinary_pick_never_marks_a_row_recovered() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/0.mov")]);
    let b1 = branch(&mut a, "xs", "side", MAIN);
    on(&mut a, "e1", &b1, vec![insert(0, "a1")]);
    let p = pick(&mut a, "p1", "e1", MAIN);
    match p.link.as_deref() {
        Some(Link::CherryPick {
            origin_hash,
            recovers,
            ..
        }) => assert!(origin_hash.is_some() && recovers.is_none()),
        other => panic!("{other:?}"),
    }
    // A quarantine row with the picked record's hash (as if its line were set aside).
    let c = rusqlite::Connection::open(a.db_path()).unwrap();
    let (hash, bytes): (String, Vec<u8>) = c
        .query_row(
            "SELECT tx_hash, record_bytes FROM txs WHERE tx_id = 'e1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    c.execute("DELETE FROM txs WHERE tx_id = 'e1'", []).unwrap();
    c.execute(
        "INSERT INTO quarantine (tx_hash, branch, origin, reason, detail, record_bytes,
         added_at, seen_at, current) VALUES (?1, ?2, 'local', 'DIVERGED_TAIL', '', ?3, 0, 0, 1)",
        rusqlite::params![hash, b1, bytes],
    )
    .unwrap();
    let recovering: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM record_links WHERE recovers IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(recovering, 0);
    assert_eq!(a.status().quarantine.count, 1);
    assert_eq!(a.recover_groups().unwrap().len(), 1);
}
