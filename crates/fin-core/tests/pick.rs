//! Cherry-pick, revert and the merged state, planned in memory (plan 4B Task 14): picks name
//! the origin root and are no-ops when the target holds it, reverts invert a merge group
//! newest first and are mirrored in later stale rebases, and `left_to_merge` lists what a
//! merge would still copy.

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::branch::{BranchIndex, BranchInfo};
use fin_core::history::{Lineage, left_to_merge, lineage_in};
use fin_core::rebase::reasons;
use fin_core::{Link, Op, Project, Rational};

/// `c1` [0,10), `c2` [10,20), `c3` [20,30) on `t1`, asset `a1`.
fn genesis() -> Project {
    with_clips(&[10, 10, 10])
}

fn ins(at: i64, dur: i64) -> Op {
    insert("$x", "t1", secs(at), "a1", Rational::zero(), secs(dur))
}

fn delete(clip: &str) -> Op {
    Op::ClipDelete { clip: clip.into() }
}

fn ids_of(p: &Project) -> Vec<(String, Rational)> {
    starts(p, 0)
}

fn at(list: &[(&str, i64)]) -> Vec<(String, Rational)> {
    list.iter()
        .map(|(c, s)| (c.to_string(), secs(*s)))
        .collect()
}

#[test]
fn pick_names_the_origin_root_and_repeats_as_no_op() {
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.fork("br2", "main", 0);
    let e1 = log.commit("br1", "e1", "alice", vec![ins(30, 1)]);
    log.commit("main", "t1", "bob", vec![ins(0, 5)]);
    let k1 = log.pick("e1", "main", "dave", "k1").unwrap().unwrap();
    assert_eq!(
        k1.record.link().unwrap(),
        Link::CherryPick {
            origin: "e1".into(),
            from: "br1".into(),
            origin_hash: None,
        }
    );
    assert_eq!(k1.record.bindings, e1.record.bindings);
    assert_eq!(k1.record.intent.author, "dave");
    assert_eq!(
        ids_of(&log.state("main")),
        at(&[("c5", 0), ("c1", 5), ("c2", 15), ("c3", 25), ("c4", 35)])
    );
    // Again, or the copy itself: main holds that root.
    assert!(log.pick("e1", "main", "dave", "k2").unwrap().is_none());
    assert!(log.pick("k1", "main", "dave", "k3").unwrap().is_none());
    // A pick of the copy onto br2 names the root, e1 on br1.
    let k4 = log.pick("k1", "br2", "dave", "k4").unwrap().unwrap();
    assert!(matches!(
        k4.record.link().unwrap(),
        Link::CherryPick { origin, from, .. } if origin == "e1" && from == "br1"
    ));
    // br1 holds the root: picking either copy back is nothing to do.
    assert!(log.pick("k4", "br1", "dave", "k5").unwrap().is_none());
    let err = log.pick("nope", "main", "dave", "k6").unwrap_err();
    assert_eq!(
        (err.code.as_str(), &err.details["kind"]),
        ("NOT_FOUND", &serde_json::json!("tx"))
    );
}

#[test]
fn pick_of_a_record_tied_to_the_target_is_excluded() {
    // main's t1 merged into br1, undone there: picking that undo into main would undo main's
    // own edit.
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("main", "t1", "bob", vec![delete("c1")]);
    log.merge("main", "br1", "alice", "m", false).unwrap();
    log.undo("br1", "alice", "u1").unwrap();
    let err = log.pick("u1", "main", "alice", "k1").unwrap_err();
    assert_eq!(
        err.details["conflicts"][0]["reason"],
        reasons::UNDOES_TARGET_RECORDS
    );
}

#[test]
fn revert_inverts_the_group_newest_first() {
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "alice", vec![ins(30, 1)]);
    log.commit("br1", "e2", "alice", vec![delete("c1")]);
    log.merge("br1", "main", "carol", "m", false).unwrap();
    let v = log.revert("m", "main", "carol", "v1").unwrap().unwrap();
    assert_eq!(v.record.reverts.as_deref(), Some("m"));
    assert!(matches!(v.record.intent.ops[0], Op::ClipRestore { .. }));
    assert_eq!(log.state("main"), genesis());
    assert!(log.revert("m", "main", "carol", "v2").unwrap().is_none());
    // An undo of the revert brings the group back; it may be reverted again.
    log.undo("main", "carol", "u1").unwrap();
    assert_eq!(log.state("main"), log.state("br1"));
    assert!(log.revert("m", "main", "carol", "v3").unwrap().is_some());
    let err = log.revert("m", "br1", "carol", "v4").unwrap_err();
    assert_eq!(err.details["kind"], "merge");
}

#[test]
fn stale_edit_over_a_reverted_merge_is_mirrored() {
    // A revert inside a stale interval mirrors the group's records, so an insert at c3's start
    // lands before c3, as at the head.
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "bob", vec![delete("c3")]);
    log.commit("br1", "e2", "bob", vec![av_asset("$b", secs(50))]);
    log.merge("br1", "main", "alice", "m", false).unwrap();
    log.revert("m", "main", "alice", "v1").unwrap().unwrap();
    assert_eq!(log.state("main"), genesis());
    let w = insert("$w", "t1", secs(20), "a1", secs(0), secs(5));
    log.commit_at("main", "b1", "bob", 0, vec![w]).unwrap();
    assert_eq!(
        ids_of(&log.state("main")),
        at(&[("c1", 0), ("c2", 10), ("c4", 20), ("c3", 25)])
    );
}

#[test]
fn left_to_merge_lists_what_a_merge_would_copy() {
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "alice", vec![ins(30, 1)]);
    log.commit("br1", "e2", "alice", vec![delete("c1")]);
    assert_eq!(left_to_merge(&log, "br1", "main").unwrap(), ["e1", "e2"]);
    log.pick("e2", "main", "dave", "k1").unwrap().unwrap();
    assert_eq!(left_to_merge(&log, "br1", "main").unwrap(), ["e1"]);
    log.merge("br1", "main", "carol", "m", false).unwrap();
    assert!(left_to_merge(&log, "br1", "main").unwrap().is_empty());
    // The other way round: main's pick of a br1 record and its merge records are no content
    // of its own to merge; its own edit is.
    assert!(left_to_merge(&log, "main", "br1").unwrap().is_empty());
    log.commit("main", "t1", "bob", vec![delete("c2")]);
    assert_eq!(left_to_merge(&log, "main", "br1").unwrap(), ["t1"]);
}

#[test]
fn lineage_in_index_matches_the_log() {
    let mut log = MemLog::new(genesis());
    log.commit("main", "t1", "bob", vec![ins(30, 1)]);
    log.commit("main", "t2", "bob", vec![ins(31, 1)]);
    log.fork("br1", "main", 2);
    log.commit("br1", "e1", "alice", vec![ins(32, 1)]);
    log.fork("br2", "br1", 3);
    let info = |id: &str, parent: &str, fork: u64| BranchInfo {
        id: id.into(),
        name: id.into(),
        parent: Some(parent.into()),
        fork_seq: fork,
        ..BranchInfo::main()
    };
    let ix = BranchIndex::from_infos([info("br1", "main", 2), info("br2", "br1", 3)]);
    use fin_core::history::LogView;
    for b in ["main", "br1", "br2"] {
        assert_eq!(lineage_in(&ix, b), log.lineage(b).unwrap(), "{b}");
    }
    assert_eq!(
        lineage_in(&ix, "loose"),
        Lineage {
            segs: vec![("loose".into(), 0, None)]
        }
    );
}
