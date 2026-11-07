//! Cherry-pick, revert and the derived `merged` state through the store (plan 4B Task 14):
//! picks keep their origin root and txHash and are no-ops when repeated, reverts take a merge
//! group's inverses newest first and rebase over later edits, and `merged` is derived from
//! the log alike on the leader, on followers in any arrival order, and after a rebuild.

mod integration_support;

use std::fs;
use std::path::{Path, PathBuf};

use fin_core::branch::BranchState;
use fin_core::{
    CommittedTx, CoreError, Edge, Link, Op, Project, Rational, SCHEMA_VERSION, Tx, VideoInfo,
};
use fin_store::{
    BranchCreate, CherryPickReq, HistoryReq, HistoryResult, MAIN, MergeReport, MergeReq, RevertReq,
    Store, StoreError,
};
use integration_support as is;
use rusqlite::Connection;
use tempfile::TempDir;

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
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

fn merge(s: &mut Store, id: &str, from: &str, into: &str) -> MergeReport {
    s.merge(MergeReq {
        tx_id: id.into(),
        author: "carol".into(),
        from: from.into(),
        into: into.into(),
        skip_conflicting: false,
        dry_run: false,
        args_hash: None,
    })
    .unwrap()
}

fn pick_req(id: &str, origin: &str, into: &str) -> CherryPickReq {
    CherryPickReq {
        tx_id: id.into(),
        author: "dave".into(),
        origin: origin.into(),
        into: into.into(),
        dry_run: false,
        args_hash: None,
    }
}

fn pick(s: &mut Store, id: &str, origin: &str, into: &str) -> HistoryResult {
    s.cherry_pick(pick_req(id, origin, into)).unwrap()
}

fn revert_req(id: &str, merge_id: &str, branch: &str) -> RevertReq {
    RevertReq {
        tx_id: id.into(),
        author: "carol".into(),
        merge_id: merge_id.into(),
        branch: branch.into(),
        dry_run: false,
        args_hash: None,
    }
}

fn revert(s: &mut Store, id: &str, merge_id: &str, branch: &str) -> HistoryResult {
    s.revert(revert_req(id, merge_id, branch)).unwrap()
}

fn undo(s: &mut Store, id: &str, author: &str, branch: &str) -> HistoryResult {
    s.undo(HistoryReq {
        tx_id: id.into(),
        author: author.into(),
        branch: branch.into(),
        dry_run: false,
        args_hash: None,
        to: None,
    })
    .unwrap()
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
        "SELECT tx_id || ':' || kind FROM record_links ORDER BY tx_id",
        "SELECT id || ':' || merged FROM branches ORDER BY id",
    ] {
        let mut stmt = c.prepare(q).unwrap();
        let r = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        out.extend(r.map(Result::unwrap));
    }
    out
}

fn tx_hash(s: &Store, tx_id: &str) -> String {
    Connection::open(s.db_path())
        .unwrap()
        .query_row("SELECT tx_hash FROM txs WHERE tx_id = ?1", [tx_id], |r| {
            r.get(0)
        })
        .unwrap()
}

/// Main at rev 2: the asset `a1`, then `c1` [0,10), `c2` [10,20), `c3` [20,30).
fn setup(s: &mut Store) {
    edit(s, MAIN, "s1", "setup", vec![asset()]);
    let clips = vec![
        insert("$x", 0, 10),
        insert("$y", 10, 10),
        insert("$z", 20, 10),
    ];
    edit(s, MAIN, "s2", "setup", clips);
}

/// A branch `name` forked from `from` at its head, by alice.
fn branch(s: &mut Store, tx_id: &str, name: &str, from: &str) -> String {
    s.create_branch(BranchCreate {
        tx_id: tx_id.into(),
        author: "alice".into(),
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

struct Env {
    _tmp: TempDir,
    s: Store,
}

/// Main as [`setup`], `br1` ("feature") forked from it at its head.
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
    branch(&mut s, "b1", "feature", MAIN);
    Env { _tmp: tmp, s }
}

fn branch_state(s: &Store, id: &str) -> BranchState {
    s.branches(true)
        .unwrap()
        .into_iter()
        .find(|b| b.id == id)
        .unwrap()
        .state
}

fn assert_verifies(s: &Store) {
    let v = s.verify();
    assert!(v.ok, "{v:?}");
}

#[test]
fn cherry_pick_single_record() {
    let Env { _tmp, mut s } = project();
    let p = edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c1", -1)]);
    let br2 = branch(&mut s, "b2", "other", MAIN);
    edit(&mut s, &br2, "o1", "bob", vec![delete("c2")]);
    edit(&mut s, MAIN, "t1", "bob", vec![insert("$w", 0, 5)]);
    let r = pick(&mut s, "k1", "e1", MAIN);
    assert!(r.undone.is_empty() && !r.dry_run);
    let [c] = r.committed.as_slice() else {
        panic!("{r:?}")
    };
    assert_eq!(
        c.link.as_deref(),
        Some(&Link::CherryPick {
            origin: "e1".into(),
            from: "br1".into(),
            origin_hash: Some(tx_hash(&s, "e1")),
            recovers: None,
        })
    );
    assert_eq!((c.tx.author.as_str(), c.tx.branch.as_str()), ("dave", MAIN));
    assert_eq!(c.bindings, p.bindings, "the copy keeps the clip's id");
    // Rebased over main's insert at 0; e2 (the trim) is not picked.
    assert_eq!(
        starts(&state(&s, MAIN)),
        at(&[("c5", 0), ("c1", 5), ("c2", 15), ("c3", 25), ("c4", 35)])
    );
    assert_eq!(
        state(&s, MAIN).timelines[0].tracks[0].items[1].duration,
        secs(10)
    );
    assert_eq!(s.ops_since(MAIN, s.head(MAIN) - 1).unwrap(), r.committed);
    // A pick of the pick (onto a sibling forked before it) names the root: origin e1, its
    // hash, from br1.
    let again = pick(&mut s, "k2", "k1", "other");
    assert_eq!(
        again.committed[0].link.as_deref(),
        Some(&Link::CherryPick {
            origin: "e1".into(),
            from: "br1".into(),
            origin_hash: Some(tx_hash(&s, "e1")),
            recovers: None,
        })
    );
    assert_eq!(again.committed[0].bindings, p.bindings);
    assert_verifies(&s);
}

#[test]
fn cherry_pick_twice_is_idempotent() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    let h = |c: char| Some(format!("blake3:{}", c.to_string().repeat(64)));
    let first = s
        .cherry_pick(CherryPickReq {
            args_hash: h('a'),
            ..pick_req("k1", "e1", MAIN)
        })
        .unwrap();
    assert_eq!(first.committed[0].tx.args_hash, h('a'));
    let before = rows(&s);
    // The retry answers from the log, also as a dry run.
    for dry_run in [false, true] {
        let r = s
            .cherry_pick(CherryPickReq {
                dry_run,
                args_hash: h('a'),
                ..pick_req("k1", "e1", MAIN)
            })
            .unwrap();
        assert_eq!(r, first);
    }
    let err = code(
        s.cherry_pick(CherryPickReq {
            args_hash: h('b'),
            ..pick_req("k1", "e1", MAIN)
        })
        .unwrap_err(),
    );
    assert_eq!(err.code, "IDEM_KEY_REUSED");
    // Picking the same record again, or one the target holds, writes nothing.
    for (key, origin, into) in [("k2", "e1", MAIN), ("k3", "s2", MAIN), ("k4", "s2", "br1")] {
        let r = pick(&mut s, key, origin, into);
        assert!(r.committed.is_empty(), "{key}: {r:?}");
        assert_eq!(r.no_op.as_deref(), Some("ALREADY_PRESENT"), "{key}");
    }
    assert_eq!(rows(&s), before);
    // A txId holding another kind, or naming a merge group, is refused.
    let undo_key = code(s.cherry_pick(pick_req("e1", "e1", MAIN)).unwrap_err());
    assert_eq!(undo_key.code, "IDEM_KEY_REUSED");
    edit(&mut s, "br1", "e2", "alice", vec![insert("$q", 32, 1)]);
    merge(&mut s, "m", "br1", MAIN);
    let key_is_merge = code(s.cherry_pick(pick_req("m", "e2", "br1")).unwrap_err());
    assert_eq!(key_is_merge.code, "IDEM_KEY_REUSED");
    let unknown = code(s.cherry_pick(pick_req("k5", "nope", MAIN)).unwrap_err());
    assert_eq!(
        (unknown.code.as_str(), &unknown.details["kind"]),
        ("NOT_FOUND", &serde_json::json!("tx"))
    );
    let no_ops = code(s.cherry_pick(pick_req("k6", "b1", "br1")).unwrap_err());
    assert_eq!(no_ops.code, "INVALID_OP");
}

#[test]
fn cherry_pick_excluded_from_later_merge() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c1", -1)]);
    pick(&mut s, "k1", "e1", MAIN);
    let r = merge(&mut s, "m", "br1", MAIN);
    assert_eq!(r.merged, ["e2"], "e1 was picked before");
    assert_eq!(state(&s, MAIN), state(&s, "br1"));
}

#[test]
fn cherry_pick_does_not_drop_source_stack() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c1", -1)]);
    pick(&mut s, "k1", "e1", MAIN);
    pick(&mut s, "k2", "e2", MAIN);
    let ids: Vec<String> = s
        .undo_stacks("br1", "alice")
        .unwrap()
        .undo
        .into_iter()
        .map(|u| u.id)
        .collect();
    assert_eq!(ids, ["e1", "e2"]);
    // The pick is the picking author's unit on the target.
    assert_eq!(s.undo_stacks(MAIN, "dave").unwrap().undo.len(), 2);
    assert_eq!(undo(&mut s, "u1", "dave", MAIN).undone, ["k2"]);
    assert_eq!(
        branch_state(&s, "br1"),
        BranchState::Active,
        "a pick is no merge"
    );
}

#[test]
fn cherry_pick_of_a_dependent_record_depends_on_skipped() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c4", -1)]);
    let before = rows(&s);
    let err = code(s.cherry_pick(pick_req("k1", "e2", MAIN)).unwrap_err());
    assert_eq!(err.code, "CONFLICT");
    let c = &err.details["conflicts"][0];
    assert_eq!(
        (c["reason"].as_str(), c["againstTx"].as_str()),
        (Some("DEPENDS_ON_SKIPPED"), Some("e1"))
    );
    assert_eq!(rows(&s), before);
}

/// br1: `e1` inserts `c4` at 30, `e2` deletes `c1`; merged into main as `m` by carol.
fn merged_group(s: &mut Store) -> MergeReport {
    edit(s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(s, "br1", "e2", "alice", vec![delete("c1")]);
    merge(s, "m", "br1", MAIN)
}

#[test]
fn revert_merge_group_newest_first() {
    let Env { _tmp, mut s } = project();
    let before = state(&s, MAIN);
    let m = merged_group(&mut s);
    let r = revert(&mut s, "v1", "m", MAIN);
    assert_eq!(r.undone, ["m"]);
    let [c] = r.committed.as_slice() else {
        panic!("{r:?}")
    };
    assert_eq!(
        c.link.as_deref(),
        Some(&Link::Revert {
            reverts: "m".into()
        })
    );
    // The inverses of m-2 (the delete) then m-1 (the insert).
    let ops = c.tx.ops.clone();
    let inverse = |id: &str| {
        let c = Connection::open(s.db_path()).unwrap();
        let bytes: Vec<u8> = c
            .query_row("SELECT record_bytes FROM txs WHERE tx_id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        v["inverse"].as_array().unwrap().len()
    };
    assert_eq!(ops.len(), inverse("m-2") + inverse("m-1"));
    assert!(matches!(ops[0], Op::ItemRestore(_)), "{ops:?}");
    assert!(matches!(ops.last(), Some(Op::ItemRemove(_))), "{ops:?}");
    assert_eq!(state(&s, MAIN), before);
    assert_eq!(m.committed.len(), 2);
    // The revert is carol's unit; undoing it brings the merge back.
    assert_eq!(undo(&mut s, "u1", "carol", MAIN).undone, ["v1"]);
    assert_eq!(state(&s, MAIN), state(&s, "br1"));
    assert_verifies(&s);
}

#[test]
fn revert_is_idempotent() {
    let Env { _tmp, mut s } = project();
    let before = state(&s, MAIN);
    merged_group(&mut s);
    let first = revert(&mut s, "v1", "m", MAIN);
    let rows_after = rows(&s);
    assert_eq!(
        revert(&mut s, "v1", "m", MAIN),
        first,
        "the retry answers from the log"
    );
    let again = revert(&mut s, "v2", "m", MAIN);
    assert!(again.committed.is_empty() && again.undone.is_empty());
    assert_eq!(again.no_op.as_deref(), Some("ALREADY_REVERTED"));
    assert_eq!(first.no_op, None);
    assert_eq!(rows(&s), rows_after);
    // Undo the revert: the group is back, and reverting it again works.
    undo(&mut s, "u1", "carol", MAIN);
    let third = revert(&mut s, "v3", "m", MAIN);
    assert_eq!(third.committed.len(), 1);
    assert_eq!(state(&s, MAIN), before);
    // A group undone is gone too.
    undo(&mut s, "u2", "carol", MAIN); // undoes v3
    undo(&mut s, "u3", "carol", MAIN); // undoes m
    assert_eq!(state(&s, MAIN), before);
    assert!(revert(&mut s, "v4", "m", MAIN).committed.is_empty());
    // Unknown groups, groups on another branch, and keys that are taken.
    let unknown = code(s.revert(revert_req("v5", "nope", MAIN)).unwrap_err());
    assert_eq!(
        (unknown.code.as_str(), &unknown.details["kind"]),
        ("NOT_FOUND", &serde_json::json!("merge"))
    );
    let elsewhere = code(s.revert(revert_req("v5", "m", "br1")).unwrap_err());
    assert_eq!(elsewhere.code, "NOT_FOUND");
    let taken = code(s.revert(revert_req("e1", "m", MAIN)).unwrap_err());
    assert_eq!(taken.code, "IDEM_KEY_REUSED");
    let is_group = code(s.revert(revert_req("m", "m", MAIN)).unwrap_err());
    assert_eq!(is_group.code, "IDEM_KEY_REUSED");
}

#[test]
fn revert_rebases_over_later_edits() {
    let Env { _tmp, mut s } = project();
    merged_group(&mut s);
    // After the merge: c2 [0,10), c3 [10,20), c4 [20,22). bob inserts W (5 s) at 0.
    edit(&mut s, MAIN, "t1", "bob", vec![insert("$w", 0, 5)]);
    revert(&mut s, "v1", "m", MAIN);
    // c4 is gone again and c1 is back in front of c2, after W.
    assert_eq!(
        starts(&state(&s, MAIN)),
        at(&[("c5", 0), ("c1", 5), ("c2", 15), ("c3", 25)])
    );
    assert_verifies(&s);
}

#[test]
fn revert_may_conflict() {
    let Env { _tmp, mut s } = project();
    merged_group(&mut s);
    edit(&mut s, MAIN, "t1", "bob", vec![delete("c4")]);
    let before = rows(&s);
    let err = code(s.revert(revert_req("v1", "m", MAIN)).unwrap_err());
    assert_eq!(err.code, "CONFLICT");
    let c = &err.details["conflicts"][0];
    assert_eq!(
        (
            c["txId"].as_str(),
            c["reason"].as_str(),
            c["againstTx"].as_str()
        ),
        (Some("v1"), Some("REMOVED"), Some("t1"))
    );
    assert_eq!(rows(&s), before, "nothing written");
}

#[test]
fn undo_merge_is_one_unit_on_target() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, MAIN, "t1", "carol", vec![insert("$w", 30, 1)]);
    let after_t1 = state(&s, MAIN);
    merged_group(&mut s);
    let st = s.undo_stacks(MAIN, "carol").unwrap();
    let units: Vec<(&str, Vec<&str>)> = st
        .undo
        .iter()
        .map(|u| {
            (
                u.id.as_str(),
                u.records.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(units, [("t1", vec!["t1"]), ("m", vec!["m-1", "m-2"])]);
    assert_eq!(undo(&mut s, "u1", "carol", MAIN).undone, ["m"]);
    assert_eq!(state(&s, MAIN), after_t1);
    // Nothing of the merge is left to undo; t1 is next.
    assert_eq!(undo(&mut s, "u2", "carol", MAIN).undone, ["t1"]);
}

#[test]
fn source_stack_dropped_after_merge_into_parent() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c1", -1)]);
    assert_eq!(s.undo_stacks("br1", "alice").unwrap().undo.len(), 2);
    merge(&mut s, "m", "br1", MAIN);
    assert!(s.undo_stacks("br1", "alice").unwrap().undo.is_empty());
    // A new edit on br1 is undoable again (and makes br1 active).
    edit(&mut s, "br1", "e3", "alice", vec![trim("c2", -1)]);
    assert_eq!(undo(&mut s, "u1", "alice", "br1").undone, ["e3"]);
}

#[test]
fn merged_state_when_nothing_left() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c3", -2)]);
    edit(&mut s, MAIN, "t1", "bob", vec![delete("c3")]);
    // e2 conflicts and stays: br1 is not merged.
    let part = s
        .merge(MergeReq {
            tx_id: "m1".into(),
            author: "carol".into(),
            from: "br1".into(),
            into: MAIN.into(),
            skip_conflicting: true,
            dry_run: false,
            args_hash: None,
        })
        .unwrap();
    assert_eq!(part.merged, ["e1"]);
    assert_eq!(part.source_state, BranchState::Active);
    // A branch merged whole is merged, also when the last record came by cherry-pick.
    let br2 = branch(&mut s, "b2", "second", MAIN);
    edit(&mut s, &br2, "f1", "alice", vec![insert("$q", 40, 1)]);
    edit(&mut s, &br2, "f2", "alice", vec![trim("c2", -1)]);
    pick(&mut s, "k1", "f2", MAIN);
    assert_eq!(
        branch_state(&s, &br2),
        BranchState::Active,
        "no merge record yet"
    );
    let r = merge(&mut s, "m2", "second", MAIN);
    assert_eq!(r.merged, ["f1"]);
    assert_eq!(r.source_state, BranchState::Merged);
    assert_eq!(branch_state(&s, &br2), BranchState::Merged);
    // A merged branch keeps its name, and main never becomes merged by updating a child.
    assert!(
        s.branches(false)
            .unwrap()
            .iter()
            .any(|b| b.name == "second")
    );
    let br3 = branch(&mut s, "b3", "third", MAIN);
    edit(&mut s, MAIN, "t2", "bob", vec![trim("c2", -1)]);
    merge(&mut s, "m3", MAIN, &br3);
    assert_eq!(branch_state(&s, MAIN), BranchState::Active);
    // A dry run reports the state it would leave, and keeps nothing.
    edit(&mut s, &br3, "g1", "alice", vec![insert("$r", 50, 1)]);
    let before = rows(&s);
    let dry = s
        .merge(MergeReq {
            tx_id: "m4".into(),
            author: "carol".into(),
            from: br3.clone(),
            into: MAIN.into(),
            skip_conflicting: false,
            dry_run: true,
            args_hash: None,
        })
        .unwrap();
    assert_eq!(dry.source_state, BranchState::Merged);
    assert_eq!(rows(&s), before);
    assert_eq!(branch_state(&s, &br3), BranchState::Active);
    assert_verifies(&s);
}

#[test]
fn new_edit_reactivates_merged_branch() {
    let Env { _tmp, mut s } = project();
    merged_group(&mut s);
    assert_eq!(branch_state(&s, "br1"), BranchState::Merged);
    edit(&mut s, "br1", "e3", "alice", vec![trim("c2", -1)]);
    assert_eq!(branch_state(&s, "br1"), BranchState::Active);
    assert_verifies(&s);
    let r = merge(&mut s, "m2", "br1", MAIN);
    assert_eq!(
        (r.merged.as_slice(), r.source_state),
        (&["e3".to_string()][..], BranchState::Merged)
    );
    // Archived wins over merged; restoring shows merged again.
    s.set_branch(fin_store::BranchSet {
        tx_id: "a1".into(),
        author: "alice".into(),
        branch: "br1".into(),
        name: None,
        archived: Some(true),
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
    assert_eq!(branch_state(&s, "br1"), BranchState::Archived);
    assert_verifies(&s);
}

#[test]
fn cherry_pick_and_revert_dry_run() {
    let e = is::env();
    let mut a = e.create("a");
    setup(&mut a);
    branch(&mut a, "b1", "feature", MAIN);
    merged_group(&mut a);
    edit(&mut a, "br1", "e3", "alice", vec![trim("c2", -1)]);
    e.flush(&mut a);
    let mut f = e.open("f");
    assert!(f.read_only().is_some());
    let before = rows(&a);
    let dry_pick = CherryPickReq {
        dry_run: true,
        ..pick_req("k1", "e3", MAIN)
    };
    let dry_revert = RevertReq {
        dry_run: true,
        ..revert_req("v1", "m", MAIN)
    };
    let planned_pick = a.cherry_pick(dry_pick.clone()).unwrap();
    let planned_revert = a.revert(dry_revert.clone()).unwrap();
    assert!(planned_pick.dry_run && planned_revert.dry_run);
    assert_eq!(rows(&a), before);
    // On a follower too, with the same plan.
    let f_before = rows(&f);
    assert_eq!(f.cherry_pick(dry_pick).unwrap(), planned_pick);
    assert_eq!(f.revert(dry_revert).unwrap(), planned_revert);
    assert_eq!(rows(&f), f_before);
    let ro = code(f.revert(revert_req("v1", "m", MAIN)).unwrap_err());
    assert_eq!(ro.code, "PROJECT_READ_ONLY");
    // The real ones commit what was planned.
    let real = pick(&mut a, "k1", "e3", MAIN);
    assert_eq!(real.committed, planned_pick.committed);
    assert!(!real.dry_run);
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

fn states(s: &Store) -> Vec<(String, BranchState, u64)> {
    s.branches(true)
        .unwrap()
        .into_iter()
        .map(|b| (b.id, b.state, b.head))
        .collect()
}

/// The leader merges br1 into main, br1 edits again (active), and the edit is cherry-picked
/// into main (merged again), one segment per step. Every follower derives the same states
/// whatever order the segments arrive in, verifies, and so does a rebuild.
#[test]
fn follower_recomputes_merged_state() {
    let e = is::env();
    let mut a = e.create("a");
    setup(&mut a);
    branch(&mut a, "b1", "feature", MAIN);
    edit(&mut a, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut a, "br1", "e2", "alice", vec![delete("c1")]);
    e.flush(&mut a);
    merge(&mut a, "m", "br1", MAIN);
    e.flush(&mut a);
    let mut f = e.open("f");
    e.poll(&mut f);
    assert_eq!(branch_state(&f, "br1"), BranchState::Merged);
    edit(&mut a, "br1", "e3", "alice", vec![trim("c2", -1)]);
    e.flush(&mut a);
    e.poll(&mut f);
    assert_eq!(branch_state(&f, "br1"), BranchState::Active);
    pick(&mut a, "k1", "e3", MAIN);
    e.flush(&mut a);
    e.poll(&mut f);
    assert_eq!(branch_state(&a, "br1"), BranchState::Merged);
    assert_eq!(states(&f), states(&a));
    assert_verifies(&f);

    let root = e.dir.join(".finisher").join("segments");
    let files: Vec<PathBuf> = all_segments(&e.dir)
        .into_iter()
        .map(|p| p.strip_prefix(&root).unwrap().to_path_buf())
        .collect();
    assert_eq!(files.len(), 5, "{files:?}");
    let first = is::segments(&e.dir)[0]
        .0
        .strip_prefix(&root)
        .unwrap()
        .to_path_buf();
    let rest: Vec<&PathBuf> = files.iter().filter(|p| **p != first).collect();
    let target = states(&a);
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
        is::copy_dir(&e.dir, &dir);
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
        assert_eq!(states(&x), target, "order {seen:?}");
        assert_verifies(&x);
    }
    // A rebuild from the folder alone derives the same.
    f.rebuild().unwrap();
    assert_eq!(states(&f), target);
    assert_verifies(&f);
}

/// Review B2: a merge copies records whose origin is on another branch than the source (an
/// ancestor's records on the source's lineage). Each branch on a copy's origin chain is
/// recomputed, so br1 becomes merged again when br2 brings its new edit along.
#[test]
fn merge_from_a_child_settles_the_parent() {
    let Env { _tmp, mut s } = project();
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    merge(&mut s, "m1", "br1", MAIN);
    assert_eq!(branch_state(&s, "br1"), BranchState::Merged);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c1", -1)]);
    assert_eq!(branch_state(&s, "br1"), BranchState::Active);
    let br2 = branch(&mut s, "b2", "child", "br1");
    edit(&mut s, &br2, "x1", "alice", vec![trim("c2", -1)]);
    let r = merge(&mut s, "m2", &br2, MAIN);
    assert_eq!(r.merged, ["e2", "x1"]);
    assert_eq!(branch_state(&s, "br1"), BranchState::Merged);
    assert_eq!(branch_state(&s, &br2), BranchState::Merged);
    assert_verifies(&s);
    // A merge that brings only the parent's records still makes the empty child merged.
    let br3 = branch(&mut s, "b3", "grandchild", "br1");
    edit(&mut s, "br1", "e3", "alice", vec![trim("c3", -1)]);
    let br4 = branch(&mut s, "b4", "late", "br1");
    let r = merge(&mut s, "m3", &br4, MAIN);
    assert_eq!(r.merged, ["e3"]);
    assert_eq!(branch_state(&s, &br4), BranchState::Merged);
    assert_eq!(branch_state(&s, "br1"), BranchState::Merged);
    assert_eq!(branch_state(&s, &br3), BranchState::Active, "never merged");
    assert_verifies(&s);
}

#[test]
fn follower_settles_the_parent_of_a_merged_child() {
    let e = is::env();
    let mut a = e.create("a");
    setup(&mut a);
    branch(&mut a, "b1", "feature", MAIN);
    edit(&mut a, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    merge(&mut a, "m1", "br1", MAIN);
    e.flush(&mut a);
    let mut f = e.open("f");
    e.poll(&mut f);
    edit(&mut a, "br1", "e2", "alice", vec![trim("c1", -1)]);
    let br2 = branch(&mut a, "b2", "child", "br1");
    edit(&mut a, &br2, "x1", "alice", vec![trim("c2", -1)]);
    merge(&mut a, "m2", &br2, MAIN);
    e.flush(&mut a);
    e.poll(&mut f);
    assert_eq!(branch_state(&a, "br1"), BranchState::Merged);
    assert_eq!(states(&f), states(&a));
    assert_verifies(&f);
}

/// Review of the B2 fix: on a follower, copies arrive before the records they copy, and
/// before the `branch` record of a child that has none of its own. Every follower ends with
/// br1, br2 and br3 merged, in every order the five segments can arrive in.
#[test]
fn merged_state_converges_when_copies_arrive_first() {
    let e = is::env();
    let mut a = e.create("a");
    setup(&mut a);
    branch(&mut a, "b1", "feature", MAIN);
    edit(&mut a, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    merge(&mut a, "m1", "br1", MAIN);
    e.flush(&mut a);
    edit(&mut a, "br1", "e2", "alice", vec![trim("c1", -1)]);
    let br2 = branch(&mut a, "b2", "child", "br1");
    edit(&mut a, &br2, "x1", "alice", vec![trim("c2", -1)]);
    edit(&mut a, "br1", "e6", "alice", vec![trim("c3", -1)]);
    let br4 = branch(&mut a, "b4", "late", "br1");
    e.flush(&mut a);
    merge(&mut a, "m2", &br2, MAIN);
    e.flush(&mut a);
    assert_eq!(branch_state(&a, "br1"), BranchState::Active);
    merge(&mut a, "m3", &br4, MAIN);
    e.flush(&mut a);
    assert_eq!(branch_state(&a, "br1"), BranchState::Merged);
    assert_eq!(branch_state(&a, &br2), BranchState::Merged);
    assert_eq!(branch_state(&a, &br4), BranchState::Merged);
    let root = e.dir.join(".finisher").join("segments");
    let files: Vec<PathBuf> = all_segments(&e.dir)
        .into_iter()
        .map(|p| p.strip_prefix(&root).unwrap().to_path_buf())
        .collect();
    assert_eq!(files.len(), 6, "{files:?}");
    let first = is::segments(&e.dir)[0]
        .0
        .strip_prefix(&root)
        .unwrap()
        .to_path_buf();
    let rest: Vec<&PathBuf> = files.iter().filter(|p| **p != first).collect();
    let target = states(&a);
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
    let mut bad = vec![];
    for (k, order) in orders.iter().enumerate() {
        let dir = e.root.join(format!("order-{k}"));
        is::copy_dir(&e.dir, &dir);
        for p in all_segments(&dir) {
            fs::remove_file(p).unwrap();
        }
        let put = |rel: &Path| {
            let to = dir.join(".finisher").join("segments").join(rel);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(root.join(rel), to).unwrap();
        };
        put(&first);
        let seen: Vec<&PathBuf> = order.iter().map(|&i| rest[i]).collect();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut x = e.try_open_at(&format!("x{k}"), &dir).unwrap();
            for &i in order {
                put(rest[i]);
                e.poll(&mut x);
            }
            let v = x.verify();
            (states(&x) == target && v.ok, format!("{:?}", states(&x)))
        }));
        match r {
            Ok((true, _)) => {}
            Ok((false, st)) => bad.push(format!("{seen:?}: {st}")),
            Err(_) => bad.push(format!("{seen:?}: PANIC")),
        }
        let _ = fs::remove_dir_all(&dir);
    }
    assert!(
        bad.is_empty(),
        "{} of {} orders differ from {target:?}:\n{}",
        bad.len(),
        orders.len(),
        bad.join("\n")
    );
}

/// Review of plan 4B Task 20: br2 gets a copy of br1's edit by a merge, then main gets the
/// edit too (br1 → main). br2 has nothing left to merge into main again, so it is merged: a
/// new copy of a root rechecks every branch holding a copy of it, not only the root's chain.
/// A follower importing the folder derives the same, and both verify.
#[test]
fn new_copy_of_a_root_settles_every_branch_holding_one() {
    let e = is::env();
    let mut s = e.create("a");
    setup(&mut s);
    let a = branch(&mut s, "b1", "one", MAIN);
    edit(&mut s, &a, "ea", "alice", vec![insert("$p", 0, 1)]);
    let b = branch(&mut s, "b2", "two", MAIN);
    edit(&mut s, &b, "eb", "alice", vec![insert("$q", 0, 1)]);
    merge(&mut s, "m1", &b, MAIN);
    assert_eq!(branch_state(&s, &b), BranchState::Merged);
    merge(&mut s, "m2", &a, &b);
    assert_eq!(
        branch_state(&s, &b),
        BranchState::Active,
        "main lacks br1's ea"
    );
    merge(&mut s, "m3", &a, MAIN);
    assert_eq!(branch_state(&s, &b), BranchState::Merged);
    assert_eq!(branch_state(&s, &a), BranchState::Merged);
    assert_verifies(&s);
    e.flush(&mut s);
    let mut f = e.open("f");
    e.poll(&mut f);
    assert_eq!(states(&f), states(&s));
    assert_verifies(&f);
}
