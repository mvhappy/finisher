//! The new modes in the rebase engine and the history planners (plan 4C Task 8): footprints from
//! each op's own diff (Decision 20), stale overwrites and splits mapped over intervening edits,
//! and undos of carving edits that meet later edits as conflicts instead of carving (B4).

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::rebase::steps_with_units;
use fin_core::{
    CommittedTx, CoreError, Edge, InsertMode, ItemBody, ItemInsert, ItemSplit, ItemTrim, MemIds,
    Op, Project, Rational, TrimMode, Tx, apply_tx, rebase_tx,
};

fn commit(p: &Project, ops: Vec<Op>, rev: u64, tx_id: &str) -> (Project, CommittedTx) {
    let mut t = tx(ops);
    t.tx_id = tx_id.into();
    t.base_rev = rev - 1;
    let a = apply_tx(p, &t, &mut MemIds::after(p)).unwrap_or_else(|e| panic!("{tx_id}: {e}"));
    (
        a.project,
        CommittedTx {
            rev,
            tx: t,
            diff: a.diff,
            bindings: a.bindings,
            link: None,
        },
    )
}

fn stale(ops: Vec<Op>) -> Tx {
    let mut t = tx(ops);
    t.tx_id = "tx-b".into();
    t
}

fn rebase(
    base: &Project,
    head: &Project,
    t: &Tx,
    since: &[CommittedTx],
) -> Result<fin_core::rebase::Rebased, CoreError> {
    rebase_tx(base, head, t, since, &mut MemIds::after(head))
}

/// The `CONFLICT` reason and blamed tx of `e`.
fn conflict(e: &CoreError) -> (String, Option<String>) {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    (
        e.details["reason"].as_str().unwrap().to_string(),
        e.details["againstTx"].as_str().map(str::to_string),
    )
}

fn overwrite(id: &str, at: Rational, duration: Rational) -> Op {
    ItemInsert {
        id: id.into(),
        track: "t1".into(),
        at,
        duration,
        mode: InsertMode::Overwrite,
        body: ItemBody::Clip {
            asset: "a1".into(),
            source_in: Rational::zero(),
            speed: secs(1),
        },
    }
    .into()
}

fn lift(item: &str) -> Op {
    fin_core::ItemDelete {
        item: item.into(),
        mode: fin_core::DeleteMode::Lift,
    }
    .into()
}

fn trim_in(item: &str, edge: Edge, delta: Rational, mode: TrimMode) -> Op {
    ItemTrim {
        item: item.into(),
        edge,
        delta,
        mode,
    }
    .into()
}

fn split(item: &str, offset: Rational, id: &str) -> Op {
    ItemSplit {
        item: item.into(),
        offset,
        id: id.into(),
    }
    .into()
}

fn spans(p: &Project) -> Vec<(String, Rational, Rational)> {
    p.timelines[0].tracks[0]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start, c.duration))
        .collect()
}

fn s(id: &str, start: i64, duration: i64) -> (String, Rational, Rational) {
    (id.to_string(), secs(start), secs(duration))
}

#[test]
fn overwrite_vs_trim_of_overwritten_item_conflicts() {
    let p = with_clips(&[10, 10]);
    let (pa, a) = commit(&p, vec![trim("c1", Edge::End, secs(-2))], 1, "tx-a");
    // At its base the overwrite trims c1's end: the diff's edge change is its footprint.
    let b = stale(vec![overwrite("$x", secs(5), secs(7))]);
    let e = rebase(&p, &pa, &b, &[a]).unwrap_err();
    assert_eq!(conflict(&e), ("SAME_EDGE".into(), Some("tx-a".into())));
}

#[test]
fn overwrite_vs_delete_of_covered_item() {
    let p = with_clips(&[10, 4, 6]);
    let (pa, a) = commit(&p, vec![lift("c2")], 1, "tx-a");
    // Both remove c2: not a conflict; the overwrite still carves what is there at the head.
    let b = stale(vec![overwrite("$x", secs(8), secs(8))]);
    let rb = rebase(&p, &pa, &b, &[a]).unwrap();
    assert_eq!(
        spans(&rb.applied.project),
        [s("c1", 0, 8), s("c4", 8, 8), s("c3", 16, 4)]
    );
    assert_well_formed(&rb.applied.project);
}

#[test]
fn roll_vs_neighbour_start_trim_same_edge() {
    let p = with_sourced(&[(10, 10), (10, 10)]);
    let (pa, a) = commit(&p, vec![trim("c2", Edge::Start, secs(1))], 1, "tx-a");
    // The roll of c1's end moves c2's start: its diff says so.
    let b = stale(vec![trim_in("c1", Edge::End, secs(2), TrimMode::Roll)]);
    let e = rebase(&p, &pa, &b, &[a]).unwrap_err();
    assert_eq!(conflict(&e), ("SAME_EDGE".into(), Some("tx-a".into())));
}

#[test]
fn stale_overwrite_maps_over_ripple_insert() {
    let p = with_clips(&[10]);
    let (pa, a) = commit(
        &p,
        vec![insert("$y", "t1", secs(0), "a1", secs(0), secs(3))],
        1,
        "tx-a",
    );
    let b = stale(vec![overwrite("$x", secs(5), secs(5))]);
    let rb = rebase(&p, &pa, &b, &[a]).unwrap();
    assert_eq!(rb.tx.ops, [overwrite("$x", secs(8), secs(5))]);
    // c1 moved with the insert; the overwrite takes the same part of it as at the base.
    assert_eq!(
        spans(&rb.applied.project),
        [s("c2", 0, 3), s("c1", 3, 5), s("c3", 8, 5)]
    );
}

/// B3: a split's offset is content-anchored; a concurrent start trim moves it by `by`.
#[test]
fn stale_split_after_start_trim_maps_offset() {
    let p = with_clips(&[10]);
    let (pa, a) = commit(&p, vec![trim("c1", Edge::Start, secs(2))], 1, "tx-a");
    let b = stale(vec![split("c1", secs(5), "$r")]);
    let rb = rebase(&p, &pa, &b, &[a]).unwrap();
    assert_eq!(rb.tx.ops, [split("c1", secs(3), "$r")]);
    // The cut is at the same source time as at the base (5 s into the asset).
    assert_eq!(clip(&rb.applied.project, "c2").source_in, secs(5));
    assert_eq!(spans(&rb.applied.project), [s("c1", 0, 3), s("c2", 3, 5)]);
}

/// B3: our own earlier start trim of the item (named by id or by our placeholder) cancels
/// against its rebased copy.
#[test]
fn own_earlier_start_trim_keeps_own_split_offset() {
    let p = with_clips(&[10]);
    let (pa, a) = commit(
        &p,
        vec![insert("$y", "t2", secs(0), "a1", secs(0), secs(3))],
        1,
        "tx-a",
    );
    let b = stale(vec![
        trim("c1", Edge::Start, secs(2)),
        split("c1", secs(3), "$r"),
        insert("$n", "t1", secs(8), "a1", secs(0), secs(10)),
        trim("$n", Edge::Start, secs(1)),
        split("$n", secs(4), "$q"),
    ]);
    let rb = rebase(&p, &pa, &b, &[a]).unwrap();
    assert_eq!(rb.tx.ops, b.ops);
}

/// B4: the undo of an overwrite trims back strictly and restores in lift mode, so a later
/// insert into the range it took is a conflict, and nothing of it is carved.
#[test]
fn undo_of_overwrite_conflicts_with_later_insert() {
    let mut log = MemLog::new(with_clips(&[10, 10]));
    log.commit(
        "main",
        "e1",
        "alice",
        vec![overwrite("$x", secs(5), secs(10))],
    );
    assert_eq!(
        spans(&log.state("main")),
        [s("c1", 0, 5), s("c3", 5, 10), s("c2", 15, 5)]
    );
    log.commit(
        "main",
        "b1",
        "bob",
        vec![insert("$y", "t1", secs(5), "a1", secs(0), secs(2))],
    );
    let before = log.state("main");
    let e = log.undo("main", "alice", "u1").unwrap_err();
    assert_eq!(conflict(&e).0, "CLIP_OVERLAP");
    assert_eq!(
        log.state("main"),
        before,
        "nothing carved, nothing appended"
    );
}

#[test]
fn undo_of_overwrite_and_split_are_exact() {
    let genesis = with_clips(&[10, 10]);
    let mut log = MemLog::new(genesis.clone());
    log.commit(
        "main",
        "e1",
        "alice",
        vec![overwrite("$x", secs(5), secs(10))],
    );
    log.commit("main", "e2", "alice", vec![split("c1", secs(2), "$r")]);
    log.undo("main", "alice", "u2").unwrap();
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(log.state("main"), genesis);
    log.redo("main", "alice", "r1").unwrap();
    log.undo("main", "alice", "u3").unwrap();
    assert_eq!(log.state("main"), genesis);
}

/// B4: the undo of a split trims the left part back strictly: a neighbour extended over the
/// right part's range since is a conflict, not carved.
#[test]
fn undo_of_split_after_neighbour_extend_conflicts() {
    let mut log = MemLog::new(with_sourced(&[(10, 0), (10, 10)]));
    log.commit("main", "e1", "alice", vec![split("c1", secs(5), "$r")]);
    log.commit(
        "main",
        "b1",
        "bob",
        vec![trim_in("c2", Edge::Start, secs(-2), TrimMode::Overwrite)],
    );
    assert_eq!(
        spans(&log.state("main")),
        [s("c1", 0, 5), s("c3", 5, 3), s("c2", 8, 12)]
    );
    let before = log.state("main");
    let e = log.undo("main", "alice", "u1").unwrap_err();
    assert_eq!(conflict(&e).0, "CLIP_OVERLAP");
    assert_eq!(log.state("main"), before);
}

/// An undo of one record mirrors it whatever its op count (an overwrite inverts to several
/// ops), so a stale edit over the pair is not judged against either.
#[test]
fn undo_of_an_overwrite_mirrors_its_record() {
    let p = with_clips(&[10, 10]);
    let mut log = MemLog::new(p.clone());
    log.commit(
        "main",
        "e1",
        "alice",
        vec![overwrite("$x", secs(5), secs(10))],
    );
    log.undo("main", "alice", "u1").unwrap();
    let recs: Vec<CommittedTx> = fin_core::history::LogView::records(&log, "main", 0, 2)
        .unwrap()
        .iter()
        .map(|r| r.committed().unwrap())
        .collect();
    let (steps, _) = steps_with_units(&recs.iter().collect::<Vec<_>>()).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1].mirror_of, Some(0));
    // Bob's trim of c1's end, written before both, meets neither as SAME_EDGE.
    let r = log
        .commit_at(
            "main",
            "b1",
            "bob",
            0,
            vec![trim_in("c1", Edge::End, secs(-1), TrimMode::Overwrite)],
        )
        .unwrap();
    assert_eq!(r.record.intent.ops.len(), 1);
    assert_eq!(spans(&log.state("main"))[0], s("c1", 0, 9));
}

fn slip(item: &str, delta: Rational) -> Op {
    fin_core::ItemSlip {
        item: item.into(),
        delta,
    }
    .into()
}

/// Task 8 review B1: a partial `undo --to` (a stale base between two of its units) is cut by
/// each unit's inverse op count, not its forward one: rA's overwrite inverts to two ops, so a
/// part sized by rA's two forward ops would take rB's restore of c4 and pair the wrong ripple
/// restore with rA.
#[test]
fn partial_undo_to_cuts_at_inverse_op_boundaries() {
    let genesis = with_clips(&[4, 4, 4, 4, 4]);
    let mut log = MemLog::new(genesis.clone());
    let cp = log.checkpoint("main", "cp", "alice", "v0");
    log.commit("main", "rC", "alice", vec![slip("c1", secs(1))]);
    let base = log.head_seq("main");
    let at_base = log.state("main");
    log.commit(
        "main",
        "rB",
        "alice",
        vec![delete("c2"), slip("c5", secs(1))],
    );
    log.commit(
        "main",
        "rA",
        "alice",
        vec![delete("c4"), overwrite("$x", secs(8), secs(4))],
    );
    log.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(spans(&log.state("main")), spans(&genesis));
    // Bob's overwrite, written after rC, lands where it would have at its base.
    log.commit_at(
        "main",
        "b1",
        "bob",
        base,
        vec![overwrite("$y", secs(6), secs(4))],
    )
    .unwrap();
    let mut fresh = MemLog::new(at_base);
    fresh.commit("main", "b1", "bob", vec![overwrite("$y", secs(6), secs(4))]);
    let want: Vec<_> = spans(&fresh.state("main"))
        .into_iter()
        .map(|(_, start, d)| (start, d))
        .collect();
    let got: Vec<_> = spans(&log.state("main"))
        .into_iter()
        .map(|(_, start, d)| (start, d))
        .collect();
    assert_eq!(got, want);
    assert_eq!(
        spans(&log.state("main"))[1..4],
        [s("c2", 4, 2), s("c7", 6, 4), s("c3", 10, 2)]
    );
}

/// Task 8 review: `compose::overwrite`'s split follows a concurrent start trim of the item
/// (its offset is content-anchored) while the insert's `at` does not, so the insert lands
/// inside the right part: `OVERWRITE_SPLITS_ITEM`, blamed on the trim through the item our
/// split cut.
#[test]
fn composed_overwrite_after_start_trim_blames_the_trim() {
    let p = with_clips(&[10]);
    let (pa, a) = commit(&p, vec![trim("c1", Edge::Start, secs(2))], 1, "tx-a");
    let body = ItemBody::Clip {
        asset: "a1".into(),
        source_in: Rational::zero(),
        speed: secs(1),
    };
    let ops = fin_core::compose::overwrite(&p, "t1", secs(4), secs(2), body, "$x")
        .unwrap()
        .ops;
    let e = rebase(&p, &pa, &stale(ops), &[a]).unwrap_err();
    assert_eq!(
        conflict(&e),
        ("OVERWRITE_SPLITS_ITEM".into(), Some("tx-a".into()))
    );
}

/// Task 8 review: an overwrite refused `OVERWRITE_SPLITS_ITEM` takes no id.
#[test]
fn refused_overwrite_allocates_nothing() {
    let p = with_clips(&[10]);
    let mut ids = MemIds::after(&p);
    let e = apply_tx(&p, &tx(vec![overwrite("$x", secs(2), secs(2))]), &mut ids).unwrap_err();
    assert_eq!(e.code, "OVERWRITE_SPLITS_ITEM");
    assert_eq!(ids, MemIds::after(&p));
}

/// Task 8 review: a split moves the left part's end, so a stale trim of that end is
/// `SAME_EDGE`.
#[test]
fn stale_end_trim_after_split_is_same_edge() {
    let p = with_clips(&[10]);
    let (pa, a) = commit(&p, vec![split("c1", secs(4), "$r")], 1, "tx-a");
    let e = rebase(&p, &pa, &stale(vec![trim("c1", Edge::End, secs(-1))]), &[a]).unwrap_err();
    assert_eq!(conflict(&e), ("SAME_EDGE".into(), Some("tx-a".into())));
}
