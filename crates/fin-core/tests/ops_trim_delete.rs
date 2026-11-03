mod common;

use common::*;
use fin_core::{ChangeKind, Edge, Op, Project, Rational, Shift};
use proptest::prelude::*;
use serde_json::json;

fn trim(clip: &str, edge: Edge, delta: Rational) -> Op {
    Op::ClipTrim { clip: clip.into(), edge, delta }
}

fn delete(clip: &str) -> Op {
    Op::ClipDelete { clip: clip.into() }
}

#[test]
fn trim_start_moves_source_in_and_ripples_later_clips() {
    let p = with_clips(&[5, 3]);
    let a = apply(&p, vec![trim("c1", Edge::Start, secs(1))]);
    let c1 = clip(&a.project, "c1");
    assert_eq!((c1.start, c1.source_in, c1.duration), (secs(0), secs(1), secs(4)));
    assert_eq!(clip(&a.project, "c2").start, secs(4));
    assert_eq!(a.diff.shifts, [Shift { track: "t1".into(), from: secs(5), by: secs(-1) }]);
    assert_eq!(a.diff.changes[0].change, ChangeKind::Updated);
    assert_eq!(a.inverse, [trim("c1", Edge::Start, secs(-1))]);
    assert_eq!(serde_json::to_value(trim("c1", Edge::Start, secs(1))).unwrap(), json!({"op": "clip.trim", "clip": "c1", "edge": "start", "delta": "1"}));
}

#[test]
fn trim_end_extends_and_ripples() {
    let p = with_clips(&[5, 3]);
    let a = apply(&p, vec![trim("c1", Edge::End, r("1/2"))]);
    assert_eq!(clip(&a.project, "c1").duration, r("11/2"));
    assert_eq!(clip(&a.project, "c2").start, r("11/2"));
    assert_eq!(undo(&a), p);
}

#[test]
fn trim_end_beyond_asset_reports_max() {
    let p = with_clips(&[5]);
    let e = try_apply(&p, vec![trim("c1", Edge::End, secs(96))]).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(e.details["max"], "95");
    assert_eq!(e.details["min"], "-149/30");
}

#[test]
fn trim_start_before_source_start_is_invalid() {
    let p = with_clips(&[5]);
    let e = try_apply(&p, vec![trim("c1", Edge::Start, secs(-1))]).unwrap_err();
    assert_eq!((e.code.as_str(), &e.details["min"]), ("INVALID_RANGE", &json!("0")));
}

#[test]
fn trim_to_zero_is_invalid() {
    let p = with_clips(&[5]);
    for op in [trim("c1", Edge::Start, secs(5)), trim("c1", Edge::End, secs(-5))] {
        let e = try_apply(&p, vec![op]).unwrap_err();
        assert_eq!(e.code, "INVALID_RANGE");
    }
    let e = try_apply(&p, vec![trim("c1", Edge::Start, secs(5))]).unwrap_err();
    assert_eq!(e.details["max"], "149/30");
}

#[test]
fn trim_unknown_clip_is_not_found() {
    let e = try_apply(&with_clips(&[5]), vec![trim("c9", Edge::End, secs(1))]).unwrap_err();
    assert_eq!((e.code.as_str(), &e.details["id"]), ("NOT_FOUND", &json!("c9")));
}

#[test]
fn delete_middle_clip_closes_the_gap() {
    let p = with_clips(&[5, 3, 2]);
    let a = apply(&p, vec![delete("c2")]);
    assert_eq!(starts(&a.project, 0), [("c1".into(), secs(0)), ("c3".into(), secs(5))]);
    assert_eq!(a.diff.shifts, [Shift { track: "t1".into(), from: secs(8), by: secs(-3) }]);
    assert_eq!(a.diff.changes[0].change, ChangeKind::Removed);
    assert!(matches!(&a.inverse[..], [Op::ClipRestore { clip, track, ripple: true }] if clip.id == "c2" && track == "t1"));
}

#[test]
fn delete_then_inverse_is_identity() {
    let p = with_clips(&[5, 3, 2]);
    assert_eq!(undo(&apply(&p, vec![delete("c2")])), p);
    assert_eq!(undo(&apply(&p, vec![delete("c1"), delete("c3")])), p);
}

#[test]
fn ops_in_one_tx_can_reference_new_clips() {
    let p = with_clips(&[5]);
    let a = apply(&p, vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(2)), trim("$x", Edge::End, secs(1)), delete("c1")]);
    assert_eq!(starts(&a.project, 0), [("c2".into(), secs(0))]);
    assert_eq!(clip(&a.project, "c2").duration, secs(3));
    assert_eq!(undo(&a), p);
}

#[derive(Debug, Clone)]
enum Step {
    Insert { boundary: usize, len: i64, src: i64 },
    Trim { clip: usize, start: bool, frames: i64 },
    Delete { clip: usize },
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (0usize..16, 1i64..240, 0i64..240).prop_map(|(boundary, len, src)| Step::Insert { boundary, len, src }),
        (0usize..16, any::<bool>(), -120i64..120).prop_map(|(clip, start, frames)| Step::Trim { clip, start, frames }),
        (0usize..16).prop_map(|clip| Step::Delete { clip }),
    ]
}

fn to_op(p: &Project, s: &Step) -> Option<Op> {
    let items = &p.timeline.tracks[0].items;
    let f = |n: i64| Rational::new(n, 30).unwrap();
    match *s {
        Step::Insert { boundary, len, src } => {
            let at = match items.get(boundary / 2) {
                Some(c) if boundary % 2 == 0 => c.start,
                Some(c) => (c.start + c.duration).unwrap(),
                None => items.last().map_or(Rational::zero(), |c| (c.start + c.duration).unwrap()),
            };
            Some(insert("$n", "t1", at, "a1", f(src), f(len)))
        }
        Step::Trim { clip, start, frames } => {
            let c = items.get(clip % items.len().max(1))?;
            Some(trim(&c.id, if start { Edge::Start } else { Edge::End }, f(frames)))
        }
        Step::Delete { clip } => Some(delete(&items.get(clip % items.len().max(1))?.id)),
    }
}

proptest! {
    #[test]
    fn random_edits_invert_and_stay_well_formed(steps in prop::collection::vec(step(), 1..16)) {
        let mut p = with_clips(&[2, 3]);
        for s in &steps {
            let Some(op) = to_op(&p, s) else { continue };
            match try_apply(&p, vec![op]) {
                Ok(a) => {
                    assert_well_formed(&a.project);
                    prop_assert_eq!(undo(&a), p.clone());
                    p = a.project;
                }
                Err(e) => prop_assert_eq!(e.code.as_str(), "INVALID_RANGE", "{:?}", e),
            }
        }
    }
}
