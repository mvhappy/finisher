//! Content offsets in step maps (plan 4C Decision 16, blocker B3): a step map carries the `by`
//! of its start-edge moves, an inverse negates them, a mapping adds them up, and a step and its
//! mirror cancel.

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::rebase::{Mapping, StepMap, Unmapped};
use fin_core::{
    Change, ChangeKind, Diff, Edge, EntityKind, ItemSplit, Op, Rational, Shift, TrimMode,
};

fn by_change(id: &str, by: Option<Rational>) -> Change {
    Change {
        kind: EntityKind::Item,
        id: id.into(),
        change: ChangeKind::Updated,
        op: 0,
        track: Some("t1".into()),
        edge: Some(Edge::Start),
        path: None,
        by,
    }
}

fn split(item: &str, offset: Rational) -> Op {
    ItemSplit {
        item: item.into(),
        offset,
        id: "$r".into(),
    }
    .into()
}

#[test]
fn step_map_content_comes_from_by_changes() {
    let diff = Diff {
        changes: vec![
            by_change("c1", Some(secs(-2))),
            by_change("c2", None),
            by_change("c1", Some(secs(-1))),
        ],
        shifts: vec![Shift {
            track: "t1".into(),
            from: secs(8),
            by: secs(-2),
            op: 0,
        }],
    };
    let m = StepMap::from_diff(&diff);
    assert_eq!(
        m.content(),
        [("c1".to_string(), secs(-2)), ("c1".to_string(), secs(-1))]
    );
    assert_eq!(StepMap::from_shifts(diff.shifts.clone()).content(), []);
    // A real start trim records it.
    let a = apply(&with_clips(&[10]), vec![trim("c1", Edge::Start, secs(2))]);
    assert_eq!(
        StepMap::from_diff(&a.diff).content(),
        [("c1".to_string(), secs(-2))]
    );
}

#[test]
fn content_map_inverse_negates() {
    let m = StepMap::from_parts(
        vec![],
        vec![("c1".into(), secs(-2)), ("c2".into(), r("1/2"))],
    );
    let inv = m.invert().unwrap();
    assert_eq!(
        inv.content(),
        [("c2".to_string(), r("-1/2")), ("c1".to_string(), secs(2))]
    );
    assert_eq!(inv.invert().unwrap(), m, "involutive");
}

#[test]
fn map_content_adds_offsets_from_start() {
    let mut mapping = Mapping::new();
    mapping.push(StepMap::from_parts(vec![], vec![("c1".into(), secs(-2))]));
    mapping.push(StepMap::from_parts(vec![], vec![("c2".into(), secs(5))]));
    mapping.push(StepMap::from_parts(vec![], vec![("c1".into(), secs(-1))]));
    assert_eq!(mapping.map_content(0, "c1", secs(5)), Ok(secs(2)));
    assert_eq!(mapping.map_content(1, "c1", secs(5)), Ok(secs(4)));
    assert_eq!(mapping.map_content(0, "c3", secs(5)), Ok(secs(5)));
    assert_eq!(mapping.map_content(3, "c1", secs(5)), Ok(secs(5)));
    let mut big = Mapping::new();
    big.push(StepMap::from_parts(
        vec![],
        vec![("c1".into(), Rational::new(i64::MAX, 1).unwrap())],
    ));
    assert_eq!(big.map_content(0, "c1", secs(1)), Err(Unmapped::Overflow));
}

#[test]
fn mirrored_trim_and_undo_cancel() {
    // In a mapping: a start trim's map and its inverse, mirrored, add up to nothing.
    let a = apply(&with_clips(&[10]), vec![trim("c1", Edge::Start, secs(2))]);
    let map = StepMap::from_diff(&a.diff);
    let mut mapping = Mapping::new();
    let i = mapping.push(map.clone());
    let j = mapping.push(map.invert().unwrap());
    mapping.set_mirror(i, j);
    assert_eq!(mapping.map_content(0, "c1", secs(5)), Ok(secs(5)));
    // Through the planners: a split written before a trim and its undo keeps its offset.
    let mut log = MemLog::new(with_clips(&[10]));
    log.commit(
        "main",
        "e1",
        "alice",
        vec![trim("c1", Edge::Start, secs(2))],
    );
    log.undo("main", "alice", "u1").unwrap();
    let r = log
        .commit_at("main", "b1", "bob", 0, vec![split("c1", secs(5))])
        .unwrap();
    assert_eq!(r.record.intent.ops, [split("c1", secs(5))]);
    assert_eq!(clip(&log.state("main"), "c2").source_in, secs(5));
    // And one written between them maps back through the undo.
    let mut log = MemLog::new(with_clips(&[10]));
    log.commit(
        "main",
        "e1",
        "alice",
        vec![trim("c1", Edge::Start, secs(2))],
    );
    log.undo("main", "alice", "u1").unwrap();
    let r = log
        .commit_at("main", "b1", "bob", 1, vec![split("c1", secs(3))])
        .unwrap();
    assert_eq!(r.record.intent.ops, [split("c1", secs(5))]);
    // A roll's start-edge neighbour carries `by` like a trim does.
    let p = with_sourced(&[(10, 10), (10, 10)]);
    let roll: Op = fin_core::ItemTrim {
        item: "c1".into(),
        edge: Edge::End,
        delta: secs(2),
        mode: TrimMode::Roll,
        alone: false,
    }
    .into();
    let a = apply(&p, vec![roll]);
    assert_eq!(
        StepMap::from_diff(&a.diff).content(),
        [("c2".to_string(), secs(-2))]
    );
}
