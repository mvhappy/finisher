mod common;

use common::*;
use fin_core::{CommittedTx, CoreError, Edge, MemIds, Op, Project, Rational, Tx, apply_tx, rebase_tx};
use proptest::prelude::*;

/// Applies `ops` to `p` and returns the result with its committed record at `rev`.
fn commit(p: &Project, ops: Vec<Op>, rev: u64, tx_id: &str) -> (Project, CommittedTx) {
    commit_with(p, ops, rev, tx_id, &mut MemIds::after(p))
}

fn commit_with(p: &Project, ops: Vec<Op>, rev: u64, tx_id: &str, ids: &mut MemIds) -> (Project, CommittedTx) {
    let mut t = tx(ops);
    t.tx_id = tx_id.into();
    t.base_rev = rev - 1;
    let a = apply_tx(p, &t, ids).unwrap();
    (a.project, CommittedTx { rev, tx: t, diff: a.diff, bindings: a.bindings })
}

fn stale(ops: Vec<Op>) -> Tx {
    let mut t = tx(ops);
    t.tx_id = "tx-b".into();
    t
}

fn at_of(op: &Op) -> Rational {
    match op {
        Op::ClipInsert { at, .. } => *at,
        other => panic!("not an insert: {other:?}"),
    }
}

fn trim(clip: &str, edge: Edge, delta: Rational) -> Op {
    Op::ClipTrim { clip: clip.into(), edge, delta }
}

#[test]
fn rebase_insert_over_insert_same_track() {
    let p = with_clips(&[5]);
    let (pa, a) = commit(&p, vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(3))], 1, "tx-a");
    let b = stale(vec![insert("$y", "t1", secs(5), "a1", secs(0), secs(2)), insert("$z", "t1", secs(0), "a1", secs(0), secs(1))]);
    let rb = rebase_tx(&b, std::slice::from_ref(&a)).unwrap();
    assert_eq!(rb.base_rev, 1);
    assert_eq!(at_of(&rb.ops[0]), secs(8), "at >= earlier at shifts by the earlier duration");
    assert_eq!(at_of(&rb.ops[1]), secs(0), "at before the earlier insert stays");
    let done = try_apply(&pa, rb.ops).unwrap().project;
    assert_well_formed(&done);
    assert_eq!(done.timeline.tracks[0].items.len(), 4);
}

#[test]
fn insert_on_another_track_is_unchanged() {
    let p = with_clips(&[5]);
    let (_, a) = commit(&p, vec![insert("$x", "t1", secs(0), "a1", secs(0), secs(3))], 1, "tx-a");
    let b = stale(vec![insert("$y", "t2", secs(0), "a1", secs(0), secs(2))]);
    assert_eq!(rebase_tx(&b, &[a]).unwrap().ops, b.ops);
}

#[test]
fn insert_after_a_deleted_clip_moves_left() {
    let p = with_clips(&[5, 3]);
    let (pa, a) = commit(&p, vec![Op::ClipDelete { clip: "c1".into() }], 1, "tx-a");
    let b = stale(vec![insert("$y", "t1", secs(8), "a1", secs(0), secs(2))]);
    let rb = rebase_tx(&b, &[a]).unwrap();
    assert_eq!(at_of(&rb.ops[0]), secs(3));
    assert_well_formed(&try_apply(&pa, rb.ops).unwrap().project);
}

#[test]
fn edit_of_deleted_clip_conflicts() {
    let p = with_clips(&[5, 3]);
    let (_, a) = commit(&p, vec![Op::ClipDelete { clip: "c2".into() }], 1, "tx-a");
    let b = stale(vec![insert("$y", "t1", secs(0), "a1", secs(0), secs(1)), trim("c2", Edge::End, secs(1))]);
    let c = rebase_tx(&b, std::slice::from_ref(&a)).unwrap_err();
    assert_eq!((c.reason.as_str(), c.op_index, c.against_tx.as_str()), ("clip removed", 1, "tx-a"));
    let e = CoreError::from(c);
    assert_eq!(e.code, "CONFLICT");
    assert_eq!((e.details["againstTx"].as_str(), e.details["opIndex"].as_u64()), (Some("tx-a"), Some(1)));
    assert_eq!(e.details["reason"], "clip removed");
    let del = stale(vec![Op::ClipDelete { clip: "c2".into() }]);
    assert_eq!(rebase_tx(&del, &[a]).unwrap_err().reason, "clip removed");
}

#[test]
fn double_trim_on_the_same_edge_conflicts() {
    let p = with_clips(&[5, 3]);
    let (pa, a) = commit(&p, vec![trim("c1", Edge::End, secs(1))], 1, "tx-a");
    let same = stale(vec![trim("c1", Edge::End, secs(-1))]);
    assert_eq!(CoreError::from(rebase_tx(&same, std::slice::from_ref(&a)).unwrap_err()).code, "CONFLICT");
    let other = stale(vec![trim("c1", Edge::Start, secs(1))]);
    let rb = rebase_tx(&other, &[a]).unwrap();
    let done = try_apply(&pa, rb.ops).unwrap().project;
    assert_eq!((clip(&done, "c1").duration, clip(&done, "c2").start), (secs(5), secs(5)));
}

#[test]
fn later_ops_see_concurrent_shifts_through_earlier_inserts() {
    // B inserts 10s at 0 (c1 moves to 10) and then inserts at c1's new start (10).
    let p = with_clips(&[5]);
    let (pa, a) = commit(&p, vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(3))], 1, "tx-a");
    let b = stale(vec![insert("$y", "t1", secs(0), "a1", secs(0), secs(10)), insert("$z", "t1", secs(10), "a1", secs(0), secs(1))]);
    let rb = rebase_tx(&b, &[a]).unwrap();
    assert_eq!((at_of(&rb.ops[0]), at_of(&rb.ops[1])), (secs(0), secs(10)));
    let done = try_apply(&pa, rb.ops).unwrap();
    assert_well_formed(&done.project);
    let z = &done.bindings["$z"];
    assert_eq!(clip(&done.project, z).start, secs(10));
    assert_eq!(clip(&done.project, "c1").start, secs(11), "z lands before c1, as B intended");
}

#[test]
fn positions_after_our_own_trim_or_delete_conflict_when_the_track_moved() {
    let p = with_clips(&[5, 3]);
    let (_, a) = commit(&p, vec![insert("$x", "t1", secs(0), "a1", secs(0), secs(3))], 1, "tx-a");
    let b = stale(vec![Op::ClipDelete { clip: "c1".into() }, insert("$y", "t1", secs(3), "a1", secs(0), secs(1))]);
    let c = rebase_tx(&b, &[a]).unwrap_err();
    assert_eq!(c.op_index, 1);
}

#[test]
fn only_newer_revisions_apply_in_rev_order() {
    let p = with_clips(&[5]);
    let (p1, c1) = commit(&p, vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(3))], 1, "tx-1");
    let (_, c2) = commit(&p1, vec![insert("$x", "t1", secs(0), "a1", secs(0), secs(2))], 2, "tx-2");
    let mut b = stale(vec![insert("$y", "t1", secs(5), "a1", secs(0), secs(1))]);
    let all = rebase_tx(&b, &[c2.clone(), c1.clone()]).unwrap();
    assert_eq!((all.base_rev, at_of(&all.ops[0])), (2, secs(10)));
    b.base_rev = 1;
    b.ops = vec![insert("$y", "t1", secs(8), "a1", secs(0), secs(1))];
    let newer = rebase_tx(&b, &[c1, c2]).unwrap();
    assert_eq!((newer.base_rev, at_of(&newer.ops[0])), (2, secs(10)));
    assert_eq!(rebase_tx(&b, &[]).unwrap(), b);
}

#[derive(Debug, Clone)]
enum Edit {
    Insert { boundary: usize, len: i64 },
    Trim { clip: usize, start: bool, frames: i64 },
    Delete { clip: usize },
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (0usize..12, 1i64..150).prop_map(|(boundary, len)| Edit::Insert { boundary, len }),
        (0usize..6, any::<bool>(), -60i64..60).prop_map(|(clip, start, frames)| Edit::Trim { clip, start, frames }),
        (0usize..6).prop_map(|clip| Edit::Delete { clip }),
    ]
}

fn to_op(p: &Project, e: &Edit, name: &str) -> Op {
    let items = &p.timeline.tracks[0].items;
    let f = |n: i64| Rational::new(n, 30).unwrap();
    match *e {
        Edit::Insert { boundary, len } => {
            let at = match items.get(boundary / 2) {
                Some(c) if boundary % 2 == 0 => c.start,
                Some(c) => (c.start + c.duration).unwrap(),
                None => items.last().map_or(Rational::zero(), |c| (c.start + c.duration).unwrap()),
            };
            insert(name, "t1", at, "a1", secs(10), f(len))
        }
        Edit::Trim { clip, start, frames } => {
            trim(&items[clip % items.len()].id, if start { Edge::Start } else { Edge::End }, f(frames))
        }
        Edit::Delete { clip } => Op::ClipDelete { clip: items[clip % items.len()].id.clone() },
    }
}

proptest! {
    #[test]
    fn rebased_pairs_never_overlap_and_keep_both_edits(
        lens in prop::collection::vec(1i64..6, 1..5),
        ea in edit(),
        eb in edit(),
    ) {
        let p = with_clips(&lens);
        let (oa, ob) = (to_op(&p, &ea, "$a"), to_op(&p, &eb, "$b"));
        prop_assume!(try_apply(&p, vec![oa.clone()]).is_ok() && try_apply(&p, vec![ob.clone()]).is_ok());
        // One allocator for both, like the store's durable counter (ids are never reused).
        let mut ids = MemIds::after(&p);
        let (pa, a) = commit_with(&p, vec![oa.clone()], 1, "tx-a", &mut ids);
        let Ok(rb) = rebase_tx(&stale(vec![ob.clone()]), std::slice::from_ref(&a)) else { return Ok(()) };
        match apply_tx(&pa, &tx(rb.ops.clone()), &mut ids) {
            Ok(done) => {
                assert_well_formed(&done.project);
                if let Op::ClipInsert { .. } = oa {
                    clip(&done.project, &a.bindings["$a"]);
                }
                if let Op::ClipInsert { duration, .. } = ob {
                    prop_assert_eq!(clip(&done.project, &done.bindings["$b"]).duration, duration);
                }
                if let Op::ClipDelete { clip: id } = &oa {
                    prop_assert!(done.project.timeline.tracks[0].items.iter().all(|c| &c.id != id));
                }
            }
            // Two trims of one clip can jointly exceed its bounds; the apply step says so.
            Err(e) => {
                prop_assert!(matches!(ob, Op::ClipTrim { .. }), "{:?} over {:?}: {:?}", rb.ops, oa, e);
                prop_assert_eq!(e.code.as_str(), "INVALID_RANGE");
            }
        }
    }
}

/// Builds a stale multi-op tx from `edits`, each written against the project as changed by the
/// earlier ones (clips created earlier in the tx are referenced by their placeholders).
fn stale_sequence(p: &Project, edits: &[Edit]) -> Option<Vec<Op>> {
    let mut local = p.clone();
    let mut ids = MemIds::after(p);
    let mut names = std::collections::BTreeMap::<String, String>::new();
    let mut ops = Vec::new();
    for (i, e) in edits.iter().enumerate() {
        if local.timeline.tracks[0].items.is_empty() && !matches!(e, Edit::Insert { .. }) {
            continue;
        }
        let mut op = to_op(&local, e, &format!("$b{i}"));
        let a = apply_tx(&local, &tx(vec![op.clone()]), &mut ids).ok()?;
        if let Op::ClipTrim { clip, .. } | Op::ClipDelete { clip } = &mut op
            && let Some(name) = names.get(clip.as_str())
        {
            *clip = name.clone();
        }
        names.extend(a.bindings.into_iter().map(|(k, v)| (v, k)));
        local = a.project;
        ops.push(op);
    }
    Some(ops)
}

proptest! {
    #[test]
    fn rebased_sequences_stay_coherent(
        lens in prop::collection::vec(1i64..6, 1..5),
        ea in prop::collection::vec(edit(), 1..3),
        eb in prop::collection::vec(edit(), 1..4),
    ) {
        let p = with_clips(&lens);
        let (Some(oa), Some(ob)) = (stale_sequence(&p, &ea), stale_sequence(&p, &eb)) else { return Ok(()) };
        prop_assume!(!oa.is_empty() && try_apply(&p, ob.clone()).is_ok());
        let mut ids = MemIds::after(&p);
        let (pa, a) = commit_with(&p, oa, 1, "tx-a", &mut ids);
        let Ok(rb) = rebase_tx(&stale(ob.clone()), std::slice::from_ref(&a)) else { return Ok(()) };
        match apply_tx(&pa, &tx(rb.ops.clone()), &mut ids) {
            Ok(done) => assert_well_formed(&done.project),
            Err(e) => prop_assert_eq!(e.code.as_str(), "INVALID_RANGE", "{:?} over {:?}: {:?}", rb.ops, a.tx.ops, e),
        }
    }
}
