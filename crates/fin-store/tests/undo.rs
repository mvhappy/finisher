//! Undo and redo through the store (plan 4B Task 12): records appended through the one write
//! path, stacks per author and branch derived from the log, rebased inverses, dry runs on a
//! follower, retries, and followers replaying the records.

mod integration_support;

use std::path::PathBuf;

use fin_core::history::UnitSource;
use fin_core::{
    CommittedTx, CoreError, Edge, Link, Op, Project, Rational, SCHEMA_VERSION, Tx, VideoInfo,
};
use fin_store::{BranchCreate, BranchSet, HistoryReq, HistoryResult, MAIN, Store, StoreError};
use integration_support as is;
use rusqlite::Connection;
use tempfile::TempDir;

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

struct Env {
    _tmp: TempDir,
    data: PathBuf,
    dir: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    Env {
        data: tmp.path().join("server-data"),
        dir: tmp.path().join("p"),
        _tmp: tmp,
    }
}

fn asset() -> Op {
    fin_core::AssetAdd::media(
        "$a",
        "/media/a.mov",
        secs(600),
        Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: secs(30),
        }),
        None,
        "fp",
    )
    .into()
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

fn tx(id: &str, author: &str, branch: &str, base_rev: u64, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: id.into(),
        author: author.into(),
        branch: branch.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    }
}

fn edit(s: &mut Store, id: &str, author: &str, ops: Vec<Op>) -> CommittedTx {
    let base = s.head(MAIN);
    s.commit(tx(id, author, MAIN, base, ops)).unwrap()
}

fn req(id: &str, author: &str, branch: &str) -> HistoryReq {
    HistoryReq {
        tx_id: id.into(),
        author: author.into(),
        branch: branch.into(),
        dry_run: false,
        args_hash: None,
        to: None,
        after_seq: None,
    }
}

fn undo(s: &mut Store, id: &str, author: &str) -> HistoryResult {
    s.undo(req(id, author, MAIN)).unwrap()
}

fn redo(s: &mut Store, id: &str, author: &str) -> HistoryResult {
    s.redo(req(id, author, MAIN)).unwrap()
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
        "SELECT tx_id || ':' || kind || ':' || coalesce(targets, '') FROM record_links ORDER BY tx_id",
    ] {
        let mut stmt = c.prepare(q).unwrap();
        let r = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        out.extend(r.map(Result::unwrap));
    }
    out
}

/// Main at rev 2, by `setup`: the asset `a1`, then `c1` [0,10), `c2` [10,20), `c3` [20,30).
fn project(e: &Env) -> Store {
    let mut s = Store::create(&e.data, &e.dir, "Demo", secs(30), None).unwrap();
    setup(&mut s);
    s
}

fn setup(s: &mut Store) {
    edit(s, "m1", "setup", vec![asset()]);
    let clips = vec![
        insert("$x", 0, 10),
        insert("$y", 10, 10),
        insert("$z", 20, 10),
    ];
    edit(s, "m2", "setup", clips);
}

#[test]
fn undo_appends_inverse_and_restores_state() {
    let e = env();
    let mut s = project(&e);
    let before = state(&s, MAIN);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    let r = undo(&mut s, "u1", "alice");
    assert!(!r.dry_run);
    assert_eq!(r.undone, ["e1"]);
    let c = &r.committed[0];
    assert_eq!(c.rev, 4);
    assert_eq!(c.tx.author, "alice");
    assert_eq!(
        c.link.as_deref(),
        Some(&Link::Undo {
            undoes: vec!["e1".into()]
        })
    );
    let Op::ItemRestore(restore) = &c.tx.ops[0] else {
        panic!("{:?}", c.tx.ops)
    };
    assert_eq!(
        (restore.item.id.as_str(), restore.item.start),
        ("c2", secs(10))
    );
    assert_eq!(state(&s, MAIN), before);
    // Reads carry the link too (the stale-commit mirrors rely on it).
    let since = s.ops_since(MAIN, 3).unwrap();
    assert_eq!(since, r.committed);
    // `record_links` names the unit.
    assert!(rows(&s).contains(&"u1:undo:[\"e1\"]".to_string()));
}

#[test]
fn undo_twice_then_redo_once() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    let after_e1 = state(&s, MAIN);
    edit(&mut s, "e2", "alice", vec![delete("c3")]);
    assert_eq!(undo(&mut s, "u1", "alice").undone, ["e2"]);
    assert_eq!(undo(&mut s, "u2", "alice").undone, ["e1"]);
    assert_eq!(
        starts(&state(&s, MAIN)),
        at(&[("c1", 0), ("c2", 10), ("c3", 20)])
    );
    let r = redo(&mut s, "r1", "alice");
    assert_eq!(r.undone, ["u2"]);
    assert_eq!(
        r.committed[0].link.as_deref(),
        Some(&Link::Redo {
            redoes: "u2".into()
        })
    );
    assert_eq!(state(&s, MAIN), after_e1);
    let st = s.undo_stacks(MAIN, "alice").unwrap();
    let ids =
        |v: &[fin_core::history::UnitEntry]| v.iter().map(|u| u.id.clone()).collect::<Vec<_>>();
    assert_eq!(
        (ids(&st.undo), ids(&st.redo)),
        (vec!["r1".into()], vec!["u1".into()])
    );
    assert_eq!(st.undo[0].source, UnitSource::Redo { tx_id: "r1".into() });
}

#[test]
fn undo_redo_undo_again() {
    let e = env();
    let mut s = project(&e);
    let before = state(&s, MAIN);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    let after_e1 = state(&s, MAIN);
    undo(&mut s, "u1", "alice");
    redo(&mut s, "r1", "alice");
    let r = undo(&mut s, "u2", "alice");
    assert_eq!(r.undone, ["r1"], "the redo record is the unit");
    assert_eq!(state(&s, MAIN), before);
    redo(&mut s, "r2", "alice");
    assert_eq!(state(&s, MAIN), after_e1);
}

#[test]
fn undo_rebases_over_other_authors_edit() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    // Bob inserts 5 s at 0; everything moves by 5, and the restore with it.
    edit(&mut s, "b1", "bob", vec![insert("$w", 0, 5)]);
    let r = undo(&mut s, "u1", "alice");
    let Op::ItemRestore(restore) = &r.committed[0].tx.ops[0] else {
        panic!()
    };
    assert_eq!(restore.item.start, secs(15));
    assert_eq!(
        starts(&state(&s, MAIN)),
        at(&[("c4", 0), ("c1", 5), ("c2", 15), ("c3", 25)])
    );
    // Bob's stack is his own.
    let bob = s.undo_stacks(MAIN, "bob").unwrap();
    assert_eq!(bob.undo.len(), 1);
    assert_eq!(bob.undo[0].id, "b1");
    assert!(s.undo_stacks(MAIN, "alice").unwrap().undo.is_empty());
}

#[test]
fn per_author_stacks() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![insert("$p", 30, 1)]);
    edit(&mut s, "b1", "bob", vec![insert("$q", 31, 1)]);
    edit(&mut s, "e2", "alice", vec![insert("$r", 32, 1)]);
    // Bob undoes his own edit, rebased over alice's later one.
    assert_eq!(undo(&mut s, "bu1", "bob").undone, ["b1"]);
    assert_eq!(
        starts(&state(&s, MAIN))[3..],
        at(&[("c4", 30), ("c6", 31)])[..]
    );
    let alice = s.undo_stacks(MAIN, "alice").unwrap();
    assert_eq!(
        alice.undo.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        ["e1", "e2"]
    );
    assert!(alice.redo.is_empty(), "bob's undo is on bob's redo stack");
    assert_eq!(undo(&mut s, "u1", "alice").undone, ["e2"]);
    assert_eq!(redo(&mut s, "br1", "bob").undone, ["bu1"]);
    assert_eq!(
        starts(&state(&s, MAIN))[3..],
        at(&[("c4", 30), ("c5", 31)])[..]
    );
    // Nothing of carol's.
    let err = code(s.undo(req("x", "carol", MAIN)).unwrap_err());
    assert_eq!(
        (err.code.as_str(), err.details["author"].as_str()),
        ("NOTHING_TO_UNDO", Some("carol"))
    );
}

#[test]
fn undo_over_another_authors_undone_pair_is_exact() {
    // X Y Z: alice deletes Y, bob deletes Z and undoes it. Alice's undo maps through bob's
    // mirrored pair, so Y comes back before Z, exactly where it was.
    let e = env();
    let mut s = project(&e);
    let before = state(&s, MAIN);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    edit(&mut s, "b1", "bob", vec![delete("c3")]);
    undo(&mut s, "bu", "bob");
    undo(&mut s, "u1", "alice");
    assert_eq!(state(&s, MAIN), before);
}

#[test]
fn stale_commit_over_undo_pair_is_mirrored() {
    // A stale edit whose interval holds an undo and its unit: the real undo record carries its
    // link, so the pair mirrors and an insert at Z's start lands before Z, as at the head.
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![delete("c3")]);
    undo(&mut s, "u1", "alice");
    s.commit(tx("b1", "bob", MAIN, 2, vec![insert("$w", 20, 5)]))
        .unwrap();
    assert_eq!(
        starts(&state(&s, MAIN)),
        at(&[("c1", 0), ("c2", 10), ("c4", 20), ("c3", 25)])
    );
}

#[test]
fn undo_conflicts_when_other_author_removed_target() {
    let e = env();
    let mut s = project(&e);
    let trim = fin_core::ItemTrim {
        item: "c2".into(),
        edge: Edge::End,
        delta: secs(-2),
        mode: fin_core::TrimMode::Ripple,
        alone: false,
    }
    .into();
    edit(&mut s, "e1", "alice", vec![trim]);
    edit(&mut s, "b1", "bob", vec![delete("c2")]);
    let before = rows(&s);
    let err = code(s.undo(req("u1", "alice", MAIN)).unwrap_err());
    assert_eq!(err.code, "CONFLICT");
    let c = &err.details["conflicts"][0];
    assert_eq!(
        (
            c["txId"].as_str(),
            c["reason"].as_str(),
            c["againstTx"].as_str()
        ),
        (Some("u1"), Some("REMOVED"), Some("b1"))
    );
    assert_eq!(rows(&s), before);
    // The unit stays on the stack.
    assert_eq!(s.undo_stacks(MAIN, "alice").unwrap().undo[0].id, "e1");
}

#[test]
fn redo_after_new_edit_is_nothing_to_redo() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    undo(&mut s, "u1", "alice");
    edit(&mut s, "e2", "alice", vec![delete("c3")]);
    let err = code(s.redo(req("r1", "alice", MAIN)).unwrap_err());
    assert_eq!(err.code, "NOTHING_TO_REDO");
    assert_eq!(err.details["branch"], MAIN);
    assert!(err.fix.is_some());
}

#[test]
fn nothing_to_undo_on_fresh_branch() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    s.create_branch(BranchCreate {
        tx_id: "b1".into(),
        author: "alice".into(),
        name: Some("feature".into()),
        from: MAIN.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
    // Main's edits (and the branch record) are not the branch's own.
    let err = code(s.undo(req("u1", "alice", "feature")).unwrap_err());
    assert_eq!(err.code, "NOTHING_TO_UNDO");
    assert_eq!(err.details["branch"], "br1");
    let err = code(s.redo(req("r1", "alice", "feature")).unwrap_err());
    assert_eq!(err.code, "NOTHING_TO_REDO");
    // The branch's own edit is.
    let base = s.head("feature");
    s.commit(tx("f1", "alice", "feature", base, vec![delete("c1")]))
        .unwrap();
    let r = s.undo(req("u1", "alice", "feature")).unwrap();
    assert_eq!(r.undone, ["f1"]);
    assert_eq!(r.committed[0].tx.branch, "br1");
    assert_eq!(state(&s, "feature"), state(&s, MAIN));
    // Main's stack is untouched: e1 still there.
    assert_eq!(s.undo_stacks(MAIN, "alice").unwrap().undo[0].id, "e1");
    // An archived branch takes no undo.
    s.set_branch(BranchSet {
        tx_id: "s1".into(),
        author: "alice".into(),
        branch: "feature".into(),
        name: None,
        archived: Some(true),
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
    assert_eq!(
        code(s.redo(req("r2", "alice", "br1")).unwrap_err()).code,
        "BRANCH_ARCHIVED"
    );
    assert_eq!(
        code(s.undo(req("u9", "alice", "nope")).unwrap_err()).code,
        "BRANCH_NOT_FOUND"
    );
}

#[test]
fn undo_on_follower_is_read_only() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    let mut f = Store::open(&e.data, &e.dir).unwrap();
    assert!(f.read_only().is_some());
    let err = code(f.undo(req("u1", "alice", MAIN)).unwrap_err());
    assert_eq!(err.code, "PROJECT_READ_ONLY");
    let err = code(f.redo(req("r1", "alice", MAIN)).unwrap_err());
    assert_eq!(err.code, "PROJECT_READ_ONLY");
}

#[test]
fn undo_dry_run_on_follower_plans() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    let before = rows(&s);
    let dry = |r: HistoryReq| HistoryReq { dry_run: true, ..r };

    // On the leader: planned, nothing kept.
    let planned = s.undo(dry(req("u1", "alice", MAIN))).unwrap();
    assert!(planned.dry_run);
    assert_eq!(planned.undone, ["e1"]);
    assert_eq!(rows(&s), before);
    assert_eq!(s.head(MAIN), 3);

    // On a follower (no lease) too.
    let mut f = Store::open(&e.data, &e.dir).unwrap();
    assert!(f.read_only().is_some());
    let on_f = f.undo(dry(req("u1", "alice", MAIN))).unwrap();
    assert_eq!(on_f.committed, planned.committed);
    assert_eq!(rows(&f), before);
    let err = code(f.redo(dry(req("r1", "alice", MAIN))).unwrap_err());
    assert_eq!(
        err.code, "NOTHING_TO_REDO",
        "planning errors come back from a dry run"
    );

    // The real undo is what the dry runs planned (apart from the commit time, not in it).
    let real = undo(&mut s, "u1", "alice");
    assert_eq!(real.committed, planned.committed);
    // A dry redo plans against the real undo now, and keeps nothing.
    let before = rows(&s);
    let r = s.redo(dry(req("r1", "alice", MAIN))).unwrap();
    assert_eq!(r.undone, ["u1"]);
    assert_eq!(rows(&s), before);
}

#[test]
fn undo_retry_same_tx_id_returns_first() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    edit(&mut s, "e2", "alice", vec![delete("c3")]);
    let first = undo(&mut s, "u1", "alice");
    let before = rows(&s);
    // The retry answers from the log; it does not undo e1 as well.
    let again = undo(&mut s, "u1", "alice");
    assert_eq!(again, first);
    assert_eq!(rows(&s), before);
    // Also as a dry run, and for a redo retry.
    let dry = s
        .undo(HistoryReq {
            dry_run: true,
            ..req("u1", "alice", MAIN)
        })
        .unwrap();
    assert_eq!(
        (dry.committed, dry.dry_run),
        (first.committed.clone(), false)
    );
    let r = redo(&mut s, "r1", "alice");
    assert_eq!(redo(&mut s, "r1", "alice"), r);
    // A key that holds another kind of record is reused.
    for (op, key) in [("undo", "e1"), ("redo", "u1"), ("undo", "r1")] {
        let r = req(key, "alice", MAIN);
        let err = code(if op == "undo" { s.undo(r) } else { s.redo(r) }.unwrap_err());
        assert_eq!(err.code, "IDEM_KEY_REUSED", "{op} {key}");
    }
    assert_eq!(
        code(s.undo(req(" ", "alice", MAIN)).unwrap_err()).code,
        "INVALID_OP"
    );
}

#[test]
fn undo_replays_on_follower() {
    let e = is::env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    setup(&mut a);
    edit(&mut a, "e1", "alice", vec![delete("c2")]);
    edit(&mut a, "b1", "bob", vec![insert("$w", 0, 5)]);
    undo(&mut a, "u1", "alice");
    redo(&mut a, "r1", "alice");
    undo(&mut a, "u2", "alice");
    undo(&mut a, "bu", "bob");
    e.flush(&mut a);
    e.poll(&mut b);
    assert_eq!(b.head(MAIN), a.head(MAIN));
    assert_eq!(state(&b, MAIN), state(&a, MAIN));
    assert_eq!(b.ops_since(MAIN, 0).unwrap(), a.ops_since(MAIN, 0).unwrap());
    for author in ["alice", "bob"] {
        assert_eq!(
            b.undo_stacks(MAIN, author).unwrap(),
            a.undo_stacks(MAIN, author).unwrap()
        );
    }
    // The follower's dry run plans what the leader then commits.
    let planned = b
        .redo(HistoryReq {
            dry_run: true,
            ..req("r2", "alice", MAIN)
        })
        .unwrap();
    let real = redo(&mut a, "r2", "alice");
    assert_eq!(planned.committed, real.committed);
    e.flush(&mut a);
    e.poll(&mut b);
    assert_eq!(state(&b, MAIN), state(&a, MAIN));
    let v = b.verify();
    assert!(v.ok, "{v:?}");
}

#[test]
fn undo_args_hash_is_stored_and_checked_on_retry() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "e1", "alice", vec![delete("c2")]);
    let h = |c: char| Some(format!("blake3:{}", c.to_string().repeat(64)));
    let with = |id: &str, hash: Option<String>| HistoryReq {
        args_hash: hash,
        ..req(id, "alice", MAIN)
    };
    let first = s.undo(with("u1", h('a'))).unwrap();
    assert_eq!(first.committed[0].tx.args_hash, h('a'));
    // The same key and hash (or no hash) is a retry; another hash is a reused key.
    assert_eq!(s.undo(with("u1", h('a'))).unwrap(), first);
    assert_eq!(s.undo(with("u1", None)).unwrap(), first);
    let err = code(s.undo(with("u1", h('b'))).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED");
    let err = code(s.redo(with("r1", Some("md5:x".into()))).unwrap_err());
    assert_eq!(err.code, "INVALID_OP");
    let r = s.redo(with("r1", h('c'))).unwrap();
    assert_eq!(r.committed[0].tx.args_hash, h('c'));
}

/// Owner decisions of 2026-11-09 (`fin_core::rebase::placement`): an undo whose item lost both
/// neighbours goes back at the nearest boundary for a person (the default) and says so in
/// `placements`; under `Store::with_when_lost(Refuse)` (an agent) it is `NEIGHBOURS_GONE` and
/// writes nothing. A clean undo still carries the field.
#[test]
fn undo_placement_follows_the_callers_policy() {
    use fin_core::rebase::placement::{PlacementOutcome as Outcome, WhenLost};
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "a1", "alice", vec![delete("c2")]);
    edit(&mut s, "b1", "bob", vec![delete("c1")]);
    edit(&mut s, "b2", "bob", vec![delete("c3")]);
    let before = rows(&s);
    let refused = s
        .with_when_lost(WhenLost::Refuse, |s| s.undo(req("u1", "alice", MAIN)))
        .unwrap_err();
    let c = code(refused);
    assert_eq!(c.code, "CONFLICT");
    assert_eq!(c.details["reason"], "NEIGHBOURS_GONE");
    assert_eq!(c.details["againstTx"], "b1");
    assert_eq!(rows(&s), before, "a refused undo writes nothing");
    let r = undo(&mut s, "u2", "alice");
    assert_eq!(r.placements.len(), 1, "{:?}", r.placements);
    let p = &r.placements[0];
    assert_eq!((p.item.as_str(), p.outcome), ("c2", Outcome::Relocated));
    assert_eq!(p.causes[0].author.as_deref(), Some("bob"));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["placements"][0]["outcome"], "relocated");
    // The redo deletes again (nothing to place), and its JSON still carries the field.
    let r = redo(&mut s, "r1", "alice");
    assert_eq!(
        serde_json::to_value(&r).unwrap()["placements"],
        serde_json::json!([])
    );
}
