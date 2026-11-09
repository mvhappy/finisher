//! Checkpoints and `undo --to` through the store (plan 4B Task 15): `checkpoint` records with
//! no ops, names unique per branch, found on a branch's lineage newest first; branches forked
//! at a checkpoint; `undo --to` as one undo record for the author's units after it, all or
//! nothing, reversed whole by one redo; dry runs, retries, followers and a rebuild.

mod integration_support;

use fin_core::branch::BranchState;
use fin_core::{CommittedTx, CoreError, Edge, Link, Op, Project, Rational, SCHEMA_VERSION, Tx};
use fin_store::{
    At, BranchCreate, BranchSet, CheckpointReq, HistoryReq, HistoryResult, MAIN, MergeReq, Store,
    StoreError,
};
use integration_support as is;
use rusqlite::Connection;
use tempfile::TempDir;

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

fn insert(id: &str, at: i64, duration: i64) -> Op {
    fin_core::ItemInsert {
        id: id.into(),
        track: "t1".into(),
        at: secs(at),
        duration: secs(duration),
        mode: fin_core::InsertMode::Ripple,
        body: fin_core::ItemBody::Clip {
            asset: "a1".into(),
            source_in: secs(0),
            speed: fin_core::Rational::from_int(1),
        },
    }
    .into()
}

fn delete(clip: &str) -> Op {
    fin_core::ItemDelete {
        item: clip.into(),
        mode: fin_core::DeleteMode::Ripple,
    }
    .into()
}

fn trim(clip: &str, delta: i64) -> Op {
    fin_core::ItemTrim {
        item: clip.into(),
        edge: Edge::End,
        delta: secs(delta),
        mode: fin_core::TrimMode::Ripple,
        alone: false,
    }
    .into()
}

fn edit(s: &mut Store, branch: &str, id: &str, author: &str, ops: Vec<Op>) -> CommittedTx {
    let base = s.head(branch);
    s.commit(Tx {
        tx_id: id.into(),
        author: author.into(),
        branch: branch.into(),
        base_rev: base,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    })
    .unwrap()
}

fn cp_req(id: &str, branch: &str, name: &str) -> CheckpointReq {
    CheckpointReq {
        tx_id: id.into(),
        author: "alice".into(),
        branch: branch.into(),
        name: name.into(),
        dry_run: false,
        args_hash: None,
    }
}

fn checkpoint(s: &mut Store, id: &str, branch: &str, name: &str) -> CommittedTx {
    s.checkpoint(cp_req(id, branch, name)).unwrap()
}

fn req(id: &str, author: &str, branch: &str, to: Option<&str>) -> HistoryReq {
    HistoryReq {
        tx_id: id.into(),
        author: author.into(),
        branch: branch.into(),
        dry_run: false,
        args_hash: None,
        to: to.map(Into::into),
        after_seq: None,
    }
}

fn undo_to(s: &mut Store, id: &str, branch: &str, to: &str) -> HistoryResult {
    s.undo(req(id, "alice", branch, Some(to))).unwrap()
}

fn create(from: &str, at: Option<At>, name: &str, id: &str) -> BranchCreate {
    BranchCreate {
        tx_id: id.into(),
        author: "alice".into(),
        name: Some(name.into()),
        from: from.into(),
        at,
        dry_run: false,
        args_hash: None,
    }
}

fn code(e: StoreError) -> CoreError {
    e.into()
}

fn state(s: &Store, branch: &str) -> Project {
    s.snapshot(branch).unwrap().1
}

fn starts(p: &Project) -> Vec<(String, Rational)> {
    p.timelines[0].tracks[0]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start))
        .collect()
}

fn at(list: &[(&str, i64)]) -> Vec<(String, Rational)> {
    list.iter()
        .map(|(c, s)| (c.to_string(), secs(*s)))
        .collect()
}

/// Everything a write could leave behind, for "nothing was written" checks.
fn rows(s: &Store) -> Vec<String> {
    let c = Connection::open(s.db_path()).unwrap();
    let mut out = vec![];
    for q in [
        "SELECT branch || ':' || seq || ':' || tx_id FROM txs ORDER BY branch, seq",
        "SELECT branch || ':' || seq FROM snapshots ORDER BY branch, seq",
        "SELECT kind || '=' || next FROM ids ORDER BY kind",
        "SELECT tx_id || ':' || kind || ':' || coalesce(name, '') || ':' || coalesce(targets, '')
         FROM record_links ORDER BY tx_id",
        "SELECT id || ':' || merged FROM branches ORDER BY id",
    ] {
        let mut stmt = c.prepare(q).unwrap();
        let r = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        out.extend(r.map(Result::unwrap));
    }
    out
}

/// Main at rev 2: the asset `a1`, then `c1` [0,10), `c2` [10,20), `c3` [20,30).
fn setup(s: &mut Store) {
    let a = is::asset("/media/a.mov");
    edit(s, MAIN, "s1", "setup", vec![a]);
    let clips = vec![
        insert("$x", 0, 10),
        insert("$y", 10, 10),
        insert("$z", 20, 10),
    ];
    edit(s, MAIN, "s2", "setup", clips);
}

struct Env {
    _tmp: TempDir,
    s: Store,
}

fn project() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Store::create(
        &tmp.path().join("server-data"),
        &tmp.path().join("p"),
        "Demo",
        secs(30),
        None,
    )
    .unwrap();
    setup(&mut s);
    Env { _tmp: tmp, s }
}

fn assert_verifies(s: &Store) {
    let v = s.verify();
    assert!(
        v.ok,
        "{:#?}",
        v.checks.iter().filter(|c| !c.ok).collect::<Vec<_>>()
    );
}

fn branch_state(s: &Store, id: &str) -> BranchState {
    s.branches(true)
        .unwrap()
        .into_iter()
        .find(|b| b.id == id)
        .unwrap()
        .state
}

#[test]
fn checkpoint_record_has_no_ops_and_is_skipped_by_stacks() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, MAIN, "e1", "alice", vec![delete("c2")]);
    let before = (state(&s, MAIN), s.undo_stacks(MAIN, "alice").unwrap());
    let c = checkpoint(&mut s, "k1", MAIN, "v1");
    assert_eq!(c.rev, 4);
    assert!(c.tx.ops.is_empty());
    assert_eq!(c.tx.author, "alice");
    assert_eq!(
        c.link.as_deref(),
        Some(&Link::Checkpoint {
            name: "v1".into(),
            recovers: None
        })
    );
    assert_eq!(s.head(MAIN), 4);
    assert_eq!(
        (state(&s, MAIN), s.undo_stacks(MAIN, "alice").unwrap()),
        before
    );
    // A plain undo walks past it to e1.
    let u = s.undo(req("u1", "alice", MAIN, None)).unwrap();
    assert_eq!(u.undone, ["e1"]);
    assert_eq!(s.find_checkpoint(MAIN, "v1").unwrap(), (MAIN.into(), 4));
    assert!(rows(&s).contains(&"k1:checkpoint:v1:".to_string()));

    // Never part of the merged state: a merged branch stays merged across a checkpoint, and
    // a merge has nothing to copy from it.
    let br = s
        .create_branch(create(MAIN, None, "feature", "b1"))
        .unwrap()
        .branch
        .id;
    edit(&mut s, &br, "f1", "bob", vec![delete("c1")]);
    let m = MergeReq {
        tx_id: "m".into(),
        author: "carol".into(),
        from: br.clone(),
        into: MAIN.into(),
        skip_conflicting: false,
        dry_run: false,
        args_hash: None,
        include: None,
        source_up_to: None,
    };
    s.merge(m.clone()).unwrap();
    assert_eq!(branch_state(&s, &br), BranchState::Merged);
    checkpoint(&mut s, "k2", &br, "done");
    assert_eq!(branch_state(&s, &br), BranchState::Merged);
    let again = s
        .merge(MergeReq {
            tx_id: "m2".into(),
            ..m
        })
        .unwrap();
    assert!(again.committed.is_empty(), "{again:?}");
    // A checkpoint is no unit to pick.
    let err = code(
        s.cherry_pick(fin_store::CherryPickReq {
            tx_id: "p1".into(),
            author: "alice".into(),
            origin: "k2".into(),
            into: MAIN.into(),
            dry_run: false,
            args_hash: None,
        })
        .unwrap_err(),
    );
    assert_eq!(err.code, "INVALID_OP");
    assert_verifies(&s);
}

#[test]
fn checkpoint_names_unique_per_branch() {
    let Env { _tmp, mut s } = project();
    checkpoint(&mut s, "k1", MAIN, "v1");
    let before = rows(&s);
    for name in ["v1", "V1"] {
        let err = code(s.checkpoint(cp_req("k2", MAIN, name)).unwrap_err());
        assert_eq!(err.code, "NAME_TAKEN", "{name}");
        assert_eq!(err.details["txId"], "k1");
        assert_eq!(err.details["seq"], 3);
    }
    assert_eq!(rows(&s), before);
    // Bad names, before anything else.
    for bad in ["", "12", "main", "br3", "-x", ".x", "a b", &"x".repeat(65)] {
        let err = code(s.checkpoint(cp_req("k3", MAIN, bad)).unwrap_err());
        assert_eq!(err.code, "INVALID_NAME", "{bad:?}");
    }
    // Another branch may hold the same name; its own one hides main's.
    let br = s
        .create_branch(create(MAIN, None, "feature", "b1"))
        .unwrap()
        .branch
        .id;
    assert_eq!(s.find_checkpoint(&br, "v1").unwrap(), (MAIN.into(), 3));
    let k = checkpoint(&mut s, "k4", "feature", "v1");
    assert_eq!(k.tx.branch, br);
    assert_eq!(s.find_checkpoint("feature", "V1").unwrap(), (br.clone(), 4));
    assert_eq!(s.find_checkpoint(MAIN, "v1").unwrap(), (MAIN.into(), 3));
    // Names with digits that are not all digits are fine.
    checkpoint(&mut s, "k5", MAIN, "v2.0-rc1");
    // An archived branch refuses checkpoints.
    s.set_branch(BranchSet {
        tx_id: "a1".into(),
        author: "alice".into(),
        branch: br.clone(),
        name: None,
        archived: Some(true),
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
    let err = code(s.checkpoint(cp_req("k6", &br, "v9")).unwrap_err());
    assert_eq!(err.code, "BRANCH_ARCHIVED");
    let err = code(s.checkpoint(cp_req("k6", "nope", "v9")).unwrap_err());
    assert_eq!(err.code, "BRANCH_NOT_FOUND");
}

#[test]
fn branch_from_checkpoint() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, MAIN, "e1", "alice", vec![delete("c1")]);
    let k = checkpoint(&mut s, "k1", MAIN, "v1");
    let at_cp = state(&s, MAIN);
    edit(&mut s, MAIN, "e2", "alice", vec![delete("c2")]);
    let made = s
        .create_branch(create(
            MAIN,
            Some(At::Checkpoint("v1".into())),
            "from-v1",
            "b1",
        ))
        .unwrap();
    assert_eq!(made.branch.fork_seq, k.rev);
    assert_eq!(made.branch.parent.as_deref(), Some(MAIN));
    assert_eq!(made.branch.head, k.rev);
    assert_eq!(state(&s, &made.branch.id), at_cp);
    let Some(Link::Branch { fork_seq, .. }) = made.record.link.as_deref() else {
        panic!("{:?}", made.record.link)
    };
    assert_eq!(*fork_seq, k.rev);
    // The branch's first record continues from the checkpoint.
    let f = edit(&mut s, "from-v1", "f1", "alice", vec![delete("c3")]);
    assert_eq!(f.rev, k.rev + 1);
    assert_verifies(&s);
}

#[test]
fn branch_from_seq_and_checkpoint_of_another_branch() {
    // `main@v1` reached from br1: v1 lies on main below br1's fork, so br1's lineage holds
    // it, and a branch of br1 forks there (below br1's own fork).
    let Env { _tmp, mut s } = project();
    let k = checkpoint(&mut s, "k1", MAIN, "v1");
    let at_v1 = state(&s, MAIN);
    edit(&mut s, MAIN, "e1", "alice", vec![delete("c1")]);
    let br1 = s
        .create_branch(create(MAIN, None, "feature", "b1"))
        .unwrap()
        .branch;
    assert!(br1.fork_seq > k.rev);
    // A checkpoint on main after br1's fork is not in br1's history.
    checkpoint(&mut s, "k2", MAIN, "late");
    edit(&mut s, &br1.id, "f1", "alice", vec![delete("c2")]);
    assert_eq!(
        s.find_checkpoint(&br1.id, "v1").unwrap(),
        (MAIN.into(), k.rev)
    );
    let err = code(s.find_checkpoint(&br1.id, "late").unwrap_err());
    assert_eq!(
        (err.code.as_str(), &err.details["kind"]),
        ("NOT_FOUND", &"checkpoint".into())
    );

    let b = s
        .create_branch(create(
            "feature",
            Some(At::Checkpoint("v1".into())),
            "old",
            "b2",
        ))
        .unwrap()
        .branch;
    assert_eq!(
        (b.parent.as_deref(), b.fork_seq),
        (Some(br1.id.as_str()), k.rev)
    );
    assert_eq!(state(&s, &b.id), at_v1);
    // The same fork by seq.
    let c = s
        .create_branch(create("feature", Some(At::Seq(k.rev)), "old2", "b3"))
        .unwrap()
        .branch;
    assert_eq!((c.parent, c.fork_seq), (b.parent, b.fork_seq));
    assert_eq!(state(&s, &c.id), at_v1);
    edit(&mut s, &b.id, "g1", "alice", vec![delete("c3")]);
    assert_verifies(&s);
}

#[test]
fn unknown_checkpoint_is_not_found() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, MAIN, "e1", "alice", vec![delete("c1")]);
    let before = rows(&s);
    let not_found = |e: StoreError| {
        let e = code(e);
        assert_eq!(e.code, "NOT_FOUND", "{e:?}");
        assert_eq!(e.details["kind"], "checkpoint");
        assert_eq!(e.details["name"], "nope");
    };
    not_found(s.undo(req("u1", "alice", MAIN, Some("nope"))).unwrap_err());
    not_found(
        s.create_branch(create(MAIN, Some(At::Checkpoint("nope".into())), "x", "b1"))
            .unwrap_err(),
    );
    not_found(s.find_checkpoint(MAIN, "nope").unwrap_err());
    assert_eq!(rows(&s), before, "nothing written, no branch id taken");
    // A redo takes no checkpoint.
    let err = code(s.redo(req("r1", "alice", MAIN, Some("v1"))).unwrap_err());
    assert_eq!(err.code, "INVALID_OP");
    // A checkpoint with nothing of the author's after it.
    checkpoint(&mut s, "k1", MAIN, "v1");
    let err = code(s.undo(req("u1", "alice", MAIN, Some("v1"))).unwrap_err());
    assert_eq!(err.code, "NOTHING_TO_UNDO");
    assert_eq!(err.details["checkpointSeq"], 4);
}

#[test]
fn undo_to_checkpoint_one_record_newest_first() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, MAIN, "e0", "alice", vec![delete("c3")]);
    checkpoint(&mut s, "k1", MAIN, "v1");
    let at_cp = state(&s, MAIN);
    let e1 = edit(&mut s, MAIN, "e1", "alice", vec![delete("c1")]);
    edit(&mut s, MAIN, "b1", "bob", vec![insert("$w", 0, 5)]);
    let e2 = edit(&mut s, MAIN, "e2", "alice", vec![trim("c2", -2)]);
    let head = s.head(MAIN);
    let r = undo_to(&mut s, "ut", MAIN, "v1");
    assert_eq!(r.undone, ["e2", "e1"]);
    assert_eq!(r.committed.len(), 1);
    let c = &r.committed[0];
    assert_eq!(c.rev, head + 1);
    assert_eq!(
        c.link.as_deref(),
        Some(&Link::Undo {
            undoes: vec!["e2".into(), "e1".into()]
        })
    );
    // One op per op: e2's inverse, then e1's (rebased over bob's insert).
    assert_eq!(c.tx.ops.len(), e2.tx.ops.len() + e1.tx.ops.len());
    assert!(matches!(c.tx.ops[0], Op::ItemTrim(_)), "{:?}", c.tx.ops);
    assert!(matches!(c.tx.ops[1], Op::ItemRestore(_)), "{:?}", c.tx.ops);
    // Bob's clip stays; alice's are as at the checkpoint, after it.
    assert_eq!(
        starts(&state(&s, MAIN)),
        at(&[("c4", 0), ("c1", 5), ("c2", 15)])
    );
    assert_eq!(starts(&at_cp), at(&[("c1", 0), ("c2", 10)]));
    let stacks = s.undo_stacks(MAIN, "alice").unwrap();
    assert_eq!(stacks.undo.len(), 1, "e0 is before the checkpoint");
    assert_eq!(stacks.redo[0].id, "ut");
    assert_eq!(s.undo_stacks(MAIN, "bob").unwrap().undo[0].id, "b1");
    assert!(rows(&s).contains(&"ut:undo::[\"e2\",\"e1\"]".to_string()));
    assert_eq!(s.ops_since(MAIN, head).unwrap(), r.committed);
}

#[test]
fn undo_to_includes_redo_units_and_redo_reverses_it_whole() {
    let Env { _tmp, mut s } = project();
    checkpoint(&mut s, "k1", MAIN, "v1");
    let at_cp = state(&s, MAIN);
    edit(&mut s, MAIN, "e1", "alice", vec![delete("c1")]);
    edit(&mut s, MAIN, "e2", "alice", vec![delete("c2")]);
    s.undo(req("u1", "alice", MAIN, None)).unwrap();
    s.redo(req("r1", "alice", MAIN, None)).unwrap();
    let at_end = state(&s, MAIN);
    let r = undo_to(&mut s, "ut", MAIN, "v1");
    assert_eq!(r.undone, ["r1", "e1"]);
    assert_eq!(state(&s, MAIN), at_cp);
    // One redo reverses the whole record, and one undo takes that redo back.
    let rr = s.redo(req("r2", "alice", MAIN, None)).unwrap();
    assert_eq!(rr.undone, ["ut"]);
    assert_eq!(state(&s, MAIN), at_end);
    let u = s.undo(req("u2", "alice", MAIN, None)).unwrap();
    assert_eq!(u.undone, ["r2"]);
    assert_eq!(state(&s, MAIN), at_cp);
    assert_verifies(&s);
}

#[test]
fn undo_to_conflict_is_all_or_nothing() {
    let Env { _tmp, mut s } = project();
    checkpoint(&mut s, "k1", MAIN, "v1");
    edit(&mut s, MAIN, "e1", "alice", vec![trim("c2", -2)]);
    edit(&mut s, MAIN, "e2", "alice", vec![trim("c3", -2)]);
    edit(&mut s, MAIN, "e3", "alice", vec![insert("$w", 0, 5)]);
    edit(&mut s, MAIN, "b1", "bob", vec![delete("c2"), delete("c3")]);
    let before = rows(&s);
    let err = code(s.undo(req("ut", "alice", MAIN, Some("v1"))).unwrap_err());
    assert_eq!(err.code, "CONFLICT");
    let list: Vec<(String, u64, String)> = err.details["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["details"]["unit"].as_str().unwrap().to_string(),
                c["opIndex"].as_u64().unwrap(),
                c["againstTx"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        list,
        [("e2".into(), 1, "b1".into()), ("e1".into(), 2, "b1".into())]
    );
    assert_eq!(err.details["txId"], "ut");
    assert_eq!(rows(&s), before, "nothing written");
    // A dry run reports the same.
    let dry = code(
        s.undo(HistoryReq {
            dry_run: true,
            ..req("ut", "alice", MAIN, Some("v1"))
        })
        .unwrap_err(),
    );
    assert_eq!(dry.details, err.details);
    // The units are all still there: a plain undo takes e3.
    let u = s.undo(req("u1", "alice", MAIN, None)).unwrap();
    assert_eq!(u.undone, ["e3"]);
}

#[test]
fn undo_to_before_fork_undoes_all_own() {
    let Env { _tmp, mut s } = project();
    checkpoint(&mut s, "k1", MAIN, "v1");
    edit(&mut s, MAIN, "e1", "alice", vec![delete("c3")]);
    let br = s
        .create_branch(create(MAIN, None, "feature", "b1"))
        .unwrap()
        .branch;
    let at_fork = state(&s, &br.id);
    edit(&mut s, &br.id, "f1", "alice", vec![delete("c1")]);
    edit(&mut s, &br.id, "f2", "alice", vec![delete("c2")]);
    // v1 is main's, below br1's fork: every unit of alice's own on br1 goes, main's e1 stays.
    let r = undo_to(&mut s, "ut", "feature", "v1");
    assert_eq!(r.undone, ["f2", "f1"]);
    assert_eq!(state(&s, &br.id), at_fork);
    assert_eq!(starts(&state(&s, MAIN)), at(&[("c1", 0), ("c2", 10)]));
    let err = code(
        s.undo(req("ut2", "alice", "feature", Some("v1")))
            .unwrap_err(),
    );
    assert_eq!(err.code, "NOTHING_TO_UNDO");
}

#[test]
fn checkpoint_dry_run() {
    let e = is::env();
    let mut a = e.create("a");
    setup(&mut a);
    edit(&mut a, MAIN, "e1", "alice", vec![delete("c1")]);
    let before = rows(&a);
    let dry = |r: CheckpointReq| CheckpointReq { dry_run: true, ..r };
    let planned = a.checkpoint(dry(cp_req("k1", MAIN, "v1"))).unwrap();
    assert_eq!(rows(&a), before);
    // A name taken comes back from a dry run too.
    checkpoint(&mut a, "k0", MAIN, "taken");
    let err = code(a.checkpoint(dry(cp_req("k1", MAIN, "taken"))).unwrap_err());
    assert_eq!(err.code, "NAME_TAKEN");
    e.flush(&mut a);

    // On a follower, read-only: a dry run plans, a real one is refused.
    let mut f = e.open("f");
    e.poll(&mut f);
    assert!(f.read_only().is_some());
    let before = rows(&f);
    let on_f = f.checkpoint(dry(cp_req("k1", MAIN, "v1"))).unwrap();
    assert_eq!(rows(&f), before);
    let err = code(f.checkpoint(cp_req("k1", MAIN, "v1")).unwrap_err());
    assert_eq!(err.code, "PROJECT_READ_ONLY");
    // What the leader then commits is what both planned (one seq later on the leader now).
    let real = checkpoint(&mut a, "k1", MAIN, "v1");
    assert_eq!(on_f.link, real.link);
    assert_eq!((on_f.rev, planned.rev), (real.rev, real.rev - 1));
    assert_eq!(on_f.tx, real.tx);

    // `undo --to` dry runs on the follower plan what the leader commits.
    edit(&mut a, MAIN, "e2", "alice", vec![delete("c2")]);
    edit(&mut a, MAIN, "e3", "alice", vec![delete("c3")]);
    e.flush(&mut a);
    e.poll(&mut f);
    let before = rows(&f);
    let planned = f
        .undo(HistoryReq {
            dry_run: true,
            ..req("ut", "alice", MAIN, Some("v1"))
        })
        .unwrap();
    assert!(planned.dry_run);
    assert_eq!(rows(&f), before);
    let real = undo_to(&mut a, "ut", MAIN, "v1");
    assert_eq!(planned.committed, real.committed);
    assert_eq!(real.undone, ["e3", "e2"]);
}

#[test]
fn checkpoint_and_undo_to_retry_same_tx_id() {
    let Env { _tmp, mut s } = project();
    let h = |c: char| Some(format!("blake3:{}", c.to_string().repeat(64)));
    let first = s
        .checkpoint(CheckpointReq {
            args_hash: h('a'),
            ..cp_req("k1", MAIN, "v1")
        })
        .unwrap();
    assert_eq!(first.tx.args_hash, h('a'));
    let before = rows(&s);
    // The retry answers from the log, even with the name taken (by itself) or no hash.
    assert_eq!(checkpoint(&mut s, "k1", MAIN, "v1"), first);
    assert_eq!(
        s.checkpoint(CheckpointReq {
            dry_run: true,
            ..cp_req("k1", MAIN, "v1")
        })
        .unwrap(),
        first
    );
    assert_eq!(rows(&s), before);
    let err = code(
        s.checkpoint(CheckpointReq {
            args_hash: h('b'),
            ..cp_req("k1", MAIN, "v1")
        })
        .unwrap_err(),
    );
    assert_eq!(err.code, "IDEM_KEY_REUSED");
    let bad = CheckpointReq {
        args_hash: Some("md5:x".into()),
        ..cp_req("k2", MAIN, "v2")
    };
    assert_eq!(code(s.checkpoint(bad).unwrap_err()).code, "INVALID_OP");
    assert_eq!(
        code(s.checkpoint(cp_req(" ", MAIN, "v2")).unwrap_err()).code,
        "INVALID_OP"
    );
    // A key holding another kind of record.
    edit(&mut s, MAIN, "e1", "alice", vec![delete("c1")]);
    let err = code(s.checkpoint(cp_req("e1", MAIN, "v2")).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED");

    // `undo --to` retried: the first record, not a second undo.
    edit(&mut s, MAIN, "e2", "alice", vec![delete("c2")]);
    let r = undo_to(&mut s, "ut", MAIN, "v1");
    let before = rows(&s);
    assert_eq!(undo_to(&mut s, "ut", MAIN, "v1"), r);
    assert_eq!(rows(&s), before);
    let err = code(s.checkpoint(cp_req("ut", MAIN, "v3")).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED");
    let err = code(s.undo(req("k1", "alice", MAIN, Some("v1"))).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED");

    // A txId that is a merge group's id.
    let br = s
        .create_branch(create(MAIN, None, "feature", "b1"))
        .unwrap()
        .branch
        .id;
    edit(&mut s, &br, "f1", "bob", vec![insert("$w", 0, 5)]);
    s.merge(MergeReq {
        tx_id: "m".into(),
        author: "carol".into(),
        from: br,
        into: MAIN.into(),
        skip_conflicting: false,
        dry_run: false,
        args_hash: None,
        include: None,
        source_up_to: None,
    })
    .unwrap();
    let err = code(s.checkpoint(cp_req("m", MAIN, "v4")).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED");
}

/// Checkpoints, a branch from one and an `undo --to` replay on a follower; the follower and a
/// rebuild of it find the same checkpoints, and verify passes.
#[test]
fn checkpoints_follow_and_rebuild() {
    let e = is::env();
    let mut a = e.create("a");
    let mut f = e.open("f");
    setup(&mut a);
    checkpoint(&mut a, "k1", MAIN, "v1");
    edit(&mut a, MAIN, "e1", "alice", vec![delete("c1")]);
    edit(&mut a, MAIN, "b1", "bob", vec![insert("$w", 0, 5)]);
    let br = a
        .create_branch(create(MAIN, Some(At::Checkpoint("v1".into())), "old", "b2"))
        .unwrap()
        .branch
        .id;
    checkpoint(&mut a, "k2", &br, "v1");
    edit(&mut a, &br, "f1", "alice", vec![delete("c2")]);
    edit(&mut a, MAIN, "e2", "alice", vec![delete("c2")]);
    undo_to(&mut a, "ut", MAIN, "v1");
    e.flush(&mut a);
    e.poll(&mut f);
    for b in [MAIN, br.as_str()] {
        assert_eq!(f.head(b), a.head(b));
        assert_eq!(state(&f, b), state(&a, b));
        assert_eq!(f.ops_since(b, 0).unwrap(), a.ops_since(b, 0).unwrap());
        assert_eq!(
            f.find_checkpoint(b, "v1").unwrap(),
            a.find_checkpoint(b, "v1").unwrap()
        );
    }
    assert_eq!(f.find_checkpoint(&br, "v1").unwrap().0, br);
    assert_eq!(
        f.undo_stacks(MAIN, "alice").unwrap(),
        a.undo_stacks(MAIN, "alice").unwrap()
    );
    assert_verifies(&f);
    let want = rows(&f);
    let rep = f.rebuild().unwrap();
    assert!(rep.verify.ok, "{:#?}", rep.verify);
    e.poll(&mut f);
    assert_eq!(rows(&f), want);
    assert_eq!(f.find_checkpoint(&br, "v1").unwrap(), (br.clone(), 4));
    assert_eq!(f.find_checkpoint(MAIN, "V1").unwrap(), (MAIN.into(), 3));
    assert_verifies(&f);
}

/// `afterSeq` (7B Task 8, a session's rollback): as `undo --to`, after a seq instead of a
/// checkpoint's; not with `to`, and a redo takes none.
#[test]
fn undo_after_seq_one_record_newest_first() {
    let Env { _tmp, mut s } = project();
    let e0 = edit(&mut s, MAIN, "e0", "alice", vec![delete("c3")]);
    edit(&mut s, MAIN, "e1", "alice", vec![delete("c1")]);
    edit(&mut s, MAIN, "b1", "bob", vec![insert("$w", 0, 5)]);
    edit(&mut s, MAIN, "e2", "alice", vec![trim("c2", -2)]);
    let after = |id: &str, seq: u64| HistoryReq {
        after_seq: Some(seq),
        ..req(id, "alice", MAIN, None)
    };
    let r = s.undo(after("ua", e0.rev)).unwrap();
    assert_eq!(r.undone, ["e2", "e1"]);
    assert_eq!(r.committed.len(), 1);
    assert_eq!(
        starts(&state(&s, MAIN)),
        at(&[("c4", 0), ("c1", 5), ("c2", 15)])
    );
    // One redo reverses it whole.
    let back = s.redo(req("ra", "alice", MAIN, None)).unwrap();
    assert_eq!(back.undone, ["ua"]);
    // Nothing of alice's after her newest: NOTHING_TO_UNDO.
    let head = s.head(MAIN);
    assert_eq!(
        code(s.undo(after("ub", head)).unwrap_err()).code,
        "NOTHING_TO_UNDO"
    );
    let both = HistoryReq {
        to: Some("v1".into()),
        ..after("uc", 1)
    };
    assert_eq!(code(s.undo(both).unwrap_err()).code, "INVALID_OP");
    assert_eq!(code(s.redo(after("rb", 1)).unwrap_err()).code, "INVALID_OP");
}
