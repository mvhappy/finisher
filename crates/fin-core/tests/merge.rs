//! Merge planning (spec §4.4, plan 4B Task 13): the common point of two lineages, which source
//! records are candidates, the steps a candidate is rebased over (mirrors through copies), and
//! the planner's conflicts, ids and authors.

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::history::{Lineage, LogView, candidate_steps, candidates, common_point};
use fin_core::rebase::reasons;
use fin_core::{CommittedTx, Edge, Link, MemIds, Op, Project, Rational};

/// `c1` [0,10), `c2` [10,20), `c3` [20,30) on `t1`, asset `a1` (100 s).
fn genesis() -> Project {
    with_clips(&[10, 10, 10])
}

fn log() -> MemLog {
    MemLog::new(genesis())
}

fn ins(at: i64, dur: i64) -> Op {
    insert("$x", "t1", secs(at), "a1", Rational::zero(), secs(dur))
}

/// A 1 s clip at the end of `t1` on `b`.
fn append_clip(log: &MemLog, b: &str) -> Op {
    let p = log.state(b);
    let end = p.timeline.tracks[0]
        .items
        .last()
        .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap());
    insert("$x", "t1", end, "a1", Rational::zero(), secs(1))
}

fn edit(log: &mut MemLog, b: &str, tx: &str) {
    let op = append_clip(log, b);
    log.commit(b, tx, "alice", vec![op]);
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

fn path(log: &MemLog, b: &str, after: u64) -> Vec<fin_core::history::LogRec> {
    log.records(b, after, log.head_seq(b)).unwrap()
}

#[test]
fn common_point_parent_child_and_siblings() {
    let mut log = log();
    for tx in ["m1", "m2", "m3"] {
        edit(&mut log, "main", tx);
    }
    log.fork("br1", "main", 2);
    log.fork("br2", "main", 3);
    edit(&mut log, "br1", "b1");
    edit(&mut log, "br1", "b2");
    log.fork("br3", "br1", 4);
    let l = |b: &str| log.lineage(b).unwrap();
    let cp = |a: &str, b: &str| common_point(&l(a), &l(b));
    assert_eq!(cp("main", "br1"), ("main".into(), 2));
    assert_eq!(cp("br1", "main"), ("main".into(), 2));
    assert_eq!(cp("br1", "br2"), ("main".into(), 2));
    assert_eq!(cp("br2", "br1"), ("main".into(), 2));
    assert_eq!(cp("br3", "br1"), ("br1".into(), 4));
    assert_eq!(cp("br1", "br3"), ("br1".into(), 4));
    assert_eq!(cp("br3", "br2"), ("main".into(), 2));
    assert_eq!(cp("br3", "main"), ("main".into(), 2));
    let root = Lineage {
        segs: vec![("x".into(), 0, None)],
    };
    assert_eq!(common_point(&root, &l("main")), (String::new(), 0));
    let err = log.merge("br3", "br3", "alice", "m", false).unwrap_err();
    assert_eq!(err.code, "INVALID_OP");
}

#[test]
fn candidates_exclude_merged_cherry_picked_and_merge_records() {
    let mut log = log();
    log.fork("br1", "main", 0);
    edit(&mut log, "main", "t1");
    edit(&mut log, "main", "t2");
    edit(&mut log, "br1", "e1");
    edit(&mut log, "br1", "e2");
    // br1 picks t1 from main and holds a merge copy of t2.
    let t1 = log.find("t1").unwrap().unwrap();
    let pick = Link::CherryPick {
        origin: "t1".into(),
        from: "main".into(),
        origin_hash: None,
    };
    log.copy("br1", "cp1", "alice", pick, &t1);
    let op = append_clip(&log, "br1");
    let m = Link::Merge {
        merge_id: "mt".into(),
        from: "main".into(),
        origin: "t2".into(),
    };
    log.append_link("br1", "mt-1", "alice", m, vec![op]);
    edit(&mut log, "br1", "e3");
    // main holds a merge copy of e1 and a pick of e2.
    let op = append_clip(&log, "main");
    let m = Link::Merge {
        merge_id: "mb".into(),
        from: "br1".into(),
        origin: "e1".into(),
    };
    log.append_link("main", "mb-1", "alice", m, vec![op]);
    let e2 = log.find("e2").unwrap().unwrap();
    let pick = Link::CherryPick {
        origin: "e2".into(),
        from: "br1".into(),
        origin_hash: None,
    };
    log.copy("main", "cp2", "alice", pick, &e2);

    let source = path(&log, "br1", 0);
    let target = path(&log, "main", 0);
    let c = candidates(&source, &target);
    let picked: Vec<&str> = c
        .picks
        .iter()
        .map(|&i| source[i].record.tx_id.as_str())
        .collect();
    assert_eq!(picked, ["e3"]);
    assert!(c.excluded.is_empty());
    // The other way: main's own edits only (its copies of br1's records are not content).
    let c = candidates(&target, &source);
    let picked: Vec<&str> = c
        .picks
        .iter()
        .map(|&i| target[i].record.tx_id.as_str())
        .collect();
    assert_eq!(
        picked,
        Vec::<&str>::new(),
        "t1 picked, t2 merged; copies are not offered"
    );
}

#[test]
fn source_undo_of_merge_from_target_is_excluded() {
    let mut log = log();
    log.fork("br1", "main", 0);
    edit(&mut log, "main", "t1");
    let before_br1 = log.state("br1");
    log.merge("main", "br1", "alice", "mt", false).unwrap();
    // On br1 alice undoes the merge of main's t1 and redoes it, then edits.
    let u = log.undo("br1", "alice", "u1").unwrap();
    assert_eq!(u.record.undoes, Some(vec!["mt".to_string()]));
    assert_eq!(log.state("br1"), before_br1);
    log.redo("br1", "alice", "r1").unwrap();
    log.undo("br1", "alice", "u2").unwrap();
    edit(&mut log, "br1", "e2");
    let plan = log.merge("br1", "main", "alice", "mb", false).unwrap();
    let excluded: Vec<(&str, &str)> = plan
        .excluded
        .iter()
        .map(|c| (c.tx_id.as_str(), c.reason.as_str()))
        .collect();
    assert_eq!(
        excluded,
        [
            ("u1", reasons::UNDOES_TARGET_RECORDS),
            ("r1", reasons::UNDOES_TARGET_RECORDS),
            ("u2", reasons::UNDOES_TARGET_RECORDS),
        ]
    );
    assert_eq!(plan.merged, ["e2"]);
    // main keeps its t1 clip, and gains e2's.
    assert_eq!(ids_of(&log.state("main")).len(), 5);
}

#[test]
fn candidate_steps_mirror_restores_inside_positions() {
    let mut log = log();
    log.fork("br1", "main", 0);
    // e1 inserts X [30,35) and Y [35,40); e2 inserts Z at 35, between them.
    let x = insert("$x", "t1", secs(30), "a1", secs(0), secs(5));
    let y = insert("$y", "t1", secs(35), "a1", secs(0), secs(5));
    log.commit("br1", "e1", "alice", vec![x, y]);
    log.commit("br1", "e2", "alice", vec![ins(35, 1)]);
    // Bob inserts W (2 s) at 0 on main: everything moves by 2.
    log.commit("main", "t1", "bob", vec![ins(0, 2)]);

    let source = path(&log, "br1", 0);
    let target = path(&log, "main", 0);
    let plan = fin_core::history::plan_merge(&log, "br1", "main", "carol", "m", false).unwrap();
    assert_eq!(plan.merged, ["e1", "e2"]);
    let copy = &plan.planned[0];
    let committed = CommittedTx {
        rev: 2,
        tx: copy.intent.clone(),
        diff: copy.applied.diff.clone(),
        bindings: copy.applied.bindings.clone(),
        link: Some(Box::new(copy.link.clone())),
    };
    let steps = candidate_steps(&source, 1, &target, &[(0, committed)]).unwrap();
    assert_eq!(steps.len(), 3);
    assert!(steps[0].invert, "e1 inverted first");
    assert_eq!(steps[0].tx_id, "e1");
    assert_eq!((steps[1].tx_id.as_str(), steps[1].mirror_of), ("t1", None));
    assert_eq!(
        steps[2].mirror_of,
        Some(0),
        "the copy mirrors its inverted original"
    );
    // Without a copy to mirror, the inverse stands alone.
    let alone = candidate_steps(&source, 1, &target, &[]).unwrap();
    assert!(alone.iter().all(|s| s.mirror_of.is_none()));

    log.merge("br1", "main", "carol", "m", false).unwrap();
    // W [0,2), c1..c3 moved by 2, X [32,37), Z [37,38), Y [38,43): Z stays between X and Y.
    assert_eq!(
        ids_of(&log.state("main")),
        at(&[
            ("c7", 0),
            ("c1", 2),
            ("c2", 12),
            ("c3", 22),
            ("c4", 32),
            ("c6", 37),
            ("c5", 38),
        ])
    );
    assert_well_formed(&log.state("main"));
}

#[test]
fn skipped_insert_makes_dependent_depends_on_skipped() {
    let mut log = log();
    log.fork("br1", "main", 0);
    // e1 trims c3 and inserts X after it; e2 trims X; e3 is independent.
    let trim = Op::ClipTrim {
        clip: "c3".into(),
        edge: Edge::End,
        delta: secs(-2),
    };
    log.commit("br1", "e1", "alice", vec![trim, ins(28, 2)]);
    let trim_x = Op::ClipTrim {
        clip: "c4".into(),
        edge: Edge::End,
        delta: secs(-1),
    };
    log.commit("br1", "e2", "alice", vec![trim_x]);
    log.commit("br1", "e3", "alice", vec![delete("c1")]);
    // main deletes c3: e1 cannot apply.
    log.commit("main", "t1", "bob", vec![delete("c3")]);
    let before = log.state("main");

    let err = log.merge("br1", "main", "alice", "m", false).unwrap_err();
    assert_eq!(err.code, "CONFLICT");
    let all: Vec<(&str, &str, Option<&str>)> = err.details["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["txId"].as_str().unwrap(),
                c["reason"].as_str().unwrap(),
                c["againstTx"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        all,
        [
            ("e1", reasons::REMOVED, Some("t1")),
            ("e2", reasons::DEPENDS_ON_SKIPPED, Some("e1")),
        ],
        "the full list, past the first conflict"
    );
    assert_eq!(log.state("main"), before, "nothing appended");

    let plan = log.merge("br1", "main", "alice", "m", true).unwrap();
    assert_eq!(plan.merged, ["e3"]);
    assert_eq!(plan.conflicts.len(), 2);
    assert_eq!(ids_of(&log.state("main")), at(&[("c2", 0)]));
    // A re-merge offers the skipped records again, and they conflict again.
    let again = log.merge("br1", "main", "alice", "m2", true).unwrap();
    assert!(again.merged.is_empty());
    assert_eq!(again.conflicts.len(), 2);
}

#[test]
fn merge_copies_keep_ids_and_take_merging_author() {
    let mut log = log();
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "bob", vec![ins(30, 1)]);
    log.commit("main", "t1", "alice", vec![ins(30, 2)]);
    let e1 = log.find("e1").unwrap().unwrap();
    let plan = log.merge("br1", "main", "carol", "mg", false).unwrap();
    let copy = log.find("mg-1").unwrap().unwrap();
    assert_eq!(plan.merge_id, "mg");
    assert_eq!(copy.record.bindings, e1.record.bindings, "the source's ids");
    assert_eq!(copy.record.bindings["$x"], "c4");
    assert_eq!(
        (
            copy.record.intent.author.as_str(),
            copy.record.author.as_str()
        ),
        ("carol", "carol")
    );
    assert_eq!(
        copy.record.link().unwrap(),
        Link::Merge {
            merge_id: "mg".into(),
            from: "br1".into(),
            origin: "e1".into(),
        }
    );
    assert_eq!(copy.record.intent.branch, "main");
    // c4 sits after main's own c5 [30,32).
    assert_eq!(
        ids_of(&log.state("main"))[3..],
        at(&[("c5", 30), ("c4", 32)])[..]
    );
    // The merge group is one unit of the merging author on the target.
    let s = log.stacks("main", "carol");
    assert_eq!(s.undo.len(), 1);
    assert_eq!(s.undo[0].id, "mg");
    // Merging again copies nothing new.
    let again = log.merge("br1", "main", "carol", "mg2", false).unwrap();
    assert!(again.planned.is_empty() && again.conflicts.is_empty());
}

#[test]
fn duplicate_id_is_already_present_or_id_collision() {
    // ALREADY_PRESENT: br2 and main both picked br1's e1 (same origin chain, same id).
    let mut log = log();
    log.fork("br1", "main", 0);
    log.fork("br2", "main", 0);
    log.commit("br1", "e1", "bob", vec![ins(30, 1)]);
    let e1 = log.find("e1").unwrap().unwrap();
    let pick = |from: &str| Link::CherryPick {
        origin: "e1".into(),
        from: from.into(),
        origin_hash: None,
    };
    log.copy("br2", "cp2", "bob", pick("br1"), &e1);
    log.copy("main", "cpm", "bob", pick("br1"), &e1);
    let err = log.merge("br2", "main", "bob", "m", false).unwrap_err();
    assert_eq!(
        err.details["conflicts"][0]["reason"],
        reasons::ALREADY_PRESENT
    );

    // ID_COLLISION: another server handed out the same id on main (Decision 33).
    let mut log = log_two_servers();
    let err = log.merge("br1", "main", "bob", "m", false).unwrap_err();
    assert_eq!(err.details["conflicts"][0]["reason"], reasons::ID_COLLISION);
}

/// br1's e1 and main's t1 both made `c4`, as two servers across a takeover may.
fn log_two_servers() -> MemLog {
    let mut log = log();
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "bob", vec![ins(30, 1)]);
    log.ids = MemIds::after(&genesis());
    log.commit("main", "t1", "alice", vec![ins(40, 1)]);
    log
}

#[test]
fn stale_edit_over_undone_merge_unit_is_mirrored() {
    // An undo of a whole merge group inside a stale interval mirrors the group's records, so an
    // insert at c3's start lands before c3, as at the head.
    let mut log = log();
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "bob", vec![delete("c3")]);
    log.commit("br1", "e2", "bob", vec![av_asset("$b", secs(50))]);
    log.merge("br1", "main", "alice", "m", false).unwrap();
    let u = log.undo("main", "alice", "u1").unwrap();
    assert_eq!(u.record.undoes, Some(vec!["m".to_string()]));
    assert_eq!(log.state("main"), genesis());
    let w = insert("$w", "t1", secs(20), "a1", secs(0), secs(5));
    log.commit_at("main", "b1", "bob", 0, vec![w]).unwrap();
    assert_eq!(
        ids_of(&log.state("main")),
        at(&[("c1", 0), ("c2", 10), ("c4", 20), ("c3", 25)])
    );
}

#[test]
fn merge_both_directions_converges() {
    let mut log = log();
    log.fork("br1", "main", 0);
    log.commit("main", "t1", "alice", vec![delete("c1")]);
    log.commit("br1", "e1", "bob", vec![ins(30, 1)]);
    log.merge("main", "br1", "bob", "m1", false).unwrap();
    edit(&mut log, "br1", "e2");
    let plan = log.merge("br1", "main", "alice", "m2", false).unwrap();
    assert_eq!(
        plan.merged,
        ["e1", "e2"],
        "the copy of t1 is not merged back"
    );
    assert_eq!(log.state("main"), log.state("br1"));
    // Nothing is left either way.
    assert!(
        log.merge("br1", "main", "alice", "m3", false)
            .unwrap()
            .planned
            .is_empty()
    );
    assert!(
        log.merge("main", "br1", "bob", "m4", false)
            .unwrap()
            .planned
            .is_empty()
    );
    assert_well_formed(&log.state("main"));
}
