//! Per-track, per-entity conflict rules (spec §4.3, Decision 17): footprints and
//! `entity_conflict`.

mod common;

use std::collections::BTreeMap;

use common::*;
use fin_core::rebase::{
    Conflict, Conflicts, Footprint, Touch, entity_conflict, footprint, reasons,
};
use fin_core::{Clip, CoreError, Edge, MemIds, Op, Project, Rational, apply_internal, apply_tx};
use serde_json::json;

fn trim(clip: &str, edge: Edge, delta: Rational) -> Op {
    Op::ClipTrim {
        clip: clip.into(),
        edge,
        delta,
    }
}

fn delete(clip: &str) -> Op {
    Op::ClipDelete { clip: clip.into() }
}

/// `op` with its new-entity placeholder replaced by the id it bound.
fn resolved(op: &Op, bindings: &BTreeMap<String, String>) -> Op {
    let mut op = op.clone();
    let bind = |id: &mut String| {
        if let Some(b) = bindings.get(id.as_str()) {
            *id = b.clone();
        }
    };
    match &mut op {
        Op::AssetAdd { id, .. } => bind(id),
        Op::ClipInsert { id, asset, .. } => {
            bind(id);
            bind(asset);
        }
        _ => {}
    }
    op
}

/// Applies `op` alone to `p` (internal ops internally) and returns its footprint, from the
/// resolved op and its own diff, plus the project after it.
fn fp(p: &Project, op: Op) -> (Footprint, Project) {
    let t = tx(vec![op.clone()]);
    let mut ids = MemIds::after(p);
    let a = if op.is_internal() {
        apply_internal(p, &t, &mut ids)
    } else {
        apply_tx(p, &t, &mut ids)
    }
    .unwrap_or_else(|e| panic!("{} does not apply: {e}", op.name()));
    (footprint(&resolved(&op, &a.bindings), &a.diff), a.project)
}

/// The conflict `ours` (judged at `p`) has with `committed` (applied in order from `p`).
fn judge(p: &Project, committed: Vec<Op>, ours: Op) -> Option<(&'static str, String)> {
    let mut cur = p.clone();
    let mut theirs = Vec::new();
    for (i, op) in committed.into_iter().enumerate() {
        let (f, next) = fp(&cur, op);
        theirs.push((format!("tx-{}", i + 1), f));
        cur = next;
    }
    let ours = fp(p, ours).0;
    let theirs: Vec<(&str, Footprint)> = theirs
        .iter()
        .map(|(t, f)| (t.as_str(), f.clone()))
        .collect();
    entity_conflict(&ours, &theirs)
}

/// `a1` (100 s) with c1 [0, 5) and c2 [5, 10) on t1 and c3 [0, 5) on t2.
fn base() -> Project {
    let p = with_clips(&[5, 5]);
    apply(
        &p,
        vec![insert("$c", "t2", secs(0), "a1", secs(0), secs(5))],
    )
    .project
}

fn removed(tx: &str) -> Option<(&'static str, String)> {
    Some((reasons::REMOVED, tx.to_string()))
}

#[test]
fn insert_on_another_track_never_conflicts() {
    let p = base();
    // c2 deleted, so its restore can be judged too.
    let deleted = apply(&p, vec![delete("c2")]);
    let restore = deleted.inverse[0].clone();
    let ours = insert("$n", "t2", secs(5), "a1", secs(0), secs(2));
    let committed: Vec<(Project, Op)> = vec![
        (
            p.clone(),
            insert("$x", "t1", secs(5), "a1", secs(0), secs(3)),
        ),
        (p.clone(), trim("c1", Edge::Start, secs(1))),
        (p.clone(), trim("c1", Edge::End, secs(-1))),
        (p.clone(), delete("c1")),
        (
            p.clone(),
            Op::ClipRemove {
                id: "c2".into(),
                ripple: false,
            },
        ),
        (deleted.project.clone(), restore),
    ];
    for (at, op) in committed {
        let name = op.name();
        let ours_fp = fp(&at, ours.clone()).0;
        let theirs = fp(&at, op).0;
        assert_eq!(
            entity_conflict(&ours_fp, &[("tx-1", theirs)]),
            None,
            "insert on t2 vs {name} on t1"
        );
    }
}

#[test]
fn trims_of_the_same_edge_conflict() {
    let p = base();
    assert_eq!(
        judge(
            &p,
            vec![trim("c1", Edge::End, secs(-1))],
            trim("c1", Edge::End, secs(-2))
        ),
        Some((reasons::SAME_EDGE, "tx-1".into()))
    );
    assert_eq!(
        judge(
            &p,
            vec![trim("c1", Edge::Start, secs(1))],
            trim("c1", Edge::End, secs(-1))
        ),
        None
    );
    assert_eq!(
        judge(
            &p,
            vec![trim("c2", Edge::End, secs(-1))],
            trim("c1", Edge::End, secs(-1))
        ),
        None
    );
}

#[test]
fn edits_of_a_removed_clip_conflict() {
    let p = base();
    assert_eq!(
        judge(&p, vec![delete("c1")], trim("c1", Edge::End, secs(-1))),
        removed("tx-1")
    );
    assert_eq!(
        judge(
            &p,
            vec![Op::ClipRemove {
                id: "c1".into(),
                ripple: true
            }],
            trim("c1", Edge::Start, secs(1))
        ),
        removed("tx-1")
    );
    assert_eq!(judge(&p, vec![delete("c1")], delete("c1")), removed("tx-1"));
    // The other way round, the delete still applies: nothing of ours is gone.
    assert_eq!(
        judge(&p, vec![trim("c1", Edge::End, secs(-1))], delete("c1")),
        None
    );
}

#[test]
fn removed_against_names_the_removing_tx() {
    let p = base();
    let committed = vec![trim("c2", Edge::End, secs(-1)), delete("c1")];
    assert_eq!(
        judge(&p, committed, trim("c1", Edge::End, secs(-1))),
        removed("tx-2")
    );
}

#[test]
fn removed_then_restored_is_not_removed() {
    let p = base();
    let deleted = apply(&p, vec![delete("c1")]);
    let restore = deleted.inverse[0].clone();
    assert_eq!(
        judge(
            &p,
            vec![delete("c1"), restore],
            trim("c1", Edge::End, secs(-1))
        ),
        None
    );
}

#[test]
fn removed_wins_over_same_edge() {
    let p = base();
    assert_eq!(
        judge(
            &p,
            vec![trim("c1", Edge::End, secs(-1)), delete("c1")],
            trim("c1", Edge::End, secs(-2))
        ),
        removed("tx-2")
    );
}

#[test]
fn insert_of_a_removed_asset_conflicts() {
    let p = apply(&project(), vec![av_asset("$a", secs(100))]).project;
    let ours = insert("$c", "t1", secs(0), "a1", secs(0), secs(5));
    assert_eq!(
        judge(&p, vec![Op::AssetRemove { id: "a1".into() }], ours.clone()),
        removed("tx-1")
    );
    // A restore uses its asset as an insert does.
    let q = apply(&p, vec![ours]);
    let deleted = apply(&q.project, vec![delete("c1")]);
    let restore = deleted.inverse[0].clone();
    assert_eq!(
        judge(
            &deleted.project,
            vec![Op::AssetRemove { id: "a1".into() }],
            restore
        ),
        removed("tx-1")
    );
}

#[test]
fn asset_ops_bind_no_track() {
    let p = apply(&project(), vec![av_asset("$a", secs(100))]).project;
    let (add, _) = fp(&project(), av_asset("$a", secs(100)));
    assert!(add.tracks.is_empty());
    assert_eq!(add.creates, vec!["a1".to_string()]);
    let (remove, _) = fp(&p, Op::AssetRemove { id: "a1".into() });
    assert!(remove.tracks.is_empty());
    assert_eq!(remove.removes, vec!["a1".to_string()]);
}

#[test]
fn trim_footprint_comes_from_its_diff() {
    let p = base();
    let (f, _) = fp(&p, trim("c3", Edge::End, secs(-1)));
    assert_eq!(f.tracks.iter().collect::<Vec<_>>(), vec!["t2"]);
    assert_eq!(f.touches, vec![("c3".to_string(), Touch::Edge(Edge::End))]);
}

/// One op of every kind, with a check that the match stays exhaustive: a new op must get a
/// footprint (and a case here) before this compiles.
#[test]
fn every_op_has_a_footprint() {
    let r = Rational::from_int;
    let clip = Clip {
        id: "c9".into(),
        asset: "a1".into(),
        start: r(0),
        source_in: r(0),
        duration: r(1),
    };
    let ops = [
        av_asset("a9", r(1)),
        Op::AssetRemove { id: "a1".into() },
        insert("c9", "t1", r(0), "a1", r(0), r(1)),
        trim("c1", Edge::End, r(1)),
        delete("c1"),
        Op::ClipRemove {
            id: "c1".into(),
            ripple: true,
        },
        Op::ClipRestore {
            clip,
            track: "t1".into(),
            ripple: true,
        },
    ];
    for op in ops {
        let f = footprint(&op, &Default::default());
        let want = match &op {
            Op::AssetAdd { id, .. } => f.creates == [id.clone()],
            Op::AssetRemove { id } => {
                f.removes == [id.clone()] && f.touches == [(id.clone(), Touch::Exists)]
            }
            Op::ClipInsert {
                id, track, asset, ..
            } => f.creates == [id.clone()] && f.uses.contains(asset) && f.tracks.contains(track),
            Op::ClipTrim { clip, edge, .. } => f.touches == [(clip.clone(), Touch::Edge(*edge))],
            Op::ClipDelete { clip } | Op::ClipRemove { id: clip, .. } => {
                f.removes == [clip.clone()] && f.touches == [(clip.clone(), Touch::Exists)]
            }
            Op::ClipRestore { clip, track, .. } => {
                f.creates == [clip.id.clone()]
                    && f.uses.contains(&clip.asset)
                    && f.tracks.contains(track)
            }
        };
        assert!(want, "{}: {f:?}", op.name());
    }
}

#[test]
fn conflict_into_core_error_keeps_4a_fields() {
    let first = Conflict::new("tx-late", 1, reasons::REMOVED, Some("tx-3".into()));
    let second = Conflict::new("tx-late", 2, reasons::SAME_EDGE, Some("tx-4".into()));
    let e = CoreError::from(Conflicts(vec![first.clone(), second.clone()]));
    assert_eq!(e.code, "CONFLICT");
    assert_eq!(e.details["reason"], "REMOVED");
    assert_eq!(e.details["opIndex"], 1);
    assert_eq!(e.details["againstTx"], "tx-3");
    assert_eq!(e.details["txId"], "tx-late");
    assert_eq!(e.fix.as_deref(), Some(first.fix.as_str()));
    assert!(e.message.contains("tx-3"), "{}", e.message);
    assert_eq!(
        e.details["conflicts"],
        json!([
            { "txId": "tx-late", "opIndex": 1, "reason": "REMOVED", "againstTx": "tx-3",
              "fix": first.fix },
            { "txId": "tx-late", "opIndex": 2, "reason": "SAME_EDGE", "againstTx": "tx-4",
              "fix": second.fix },
        ])
    );
    let round: Conflict = serde_json::from_value(e.details["conflicts"][1].clone()).unwrap();
    assert_eq!(round, second);
}

#[test]
fn conflict_without_against_tx() {
    let c = Conflict::new("m-1", 0, reasons::ALREADY_PRESENT, None);
    let e = CoreError::from(Conflicts(vec![c]));
    assert_eq!(e.details["againstTx"], serde_json::Value::Null);
    assert_eq!(
        e.details["conflicts"][0]["againstTx"],
        serde_json::Value::Null
    );
}

#[test]
fn rerun_conflict_takes_the_policy_code() {
    let e = CoreError::new("INSERT_INSIDE_CLIP", "inside c2").with_fix("insert at a boundary");
    let c = Conflict::from_rerun(&e, "tx-b", 3, Some("tx-2".into()));
    assert_eq!(
        c,
        Conflict {
            tx_id: "tx-b".into(),
            op_index: 3,
            reason: "INSERT_INSIDE_CLIP".into(),
            against_tx: Some("tx-2".into()),
            fix: "insert at a boundary".into(),
        }
    );
    // Without a fix of its own, the conflict still says what to do.
    let bare = Conflict::from_rerun(&CoreError::new("NOT_FOUND", "gone"), "tx-b", 0, None);
    assert_eq!(bare.reason, "NOT_FOUND");
    assert!(!bare.fix.is_empty());
}

#[test]
fn reason_codes() {
    assert_eq!(
        [
            reasons::REMOVED,
            reasons::SAME_EDGE,
            reasons::SAME_PARAM,
            reasons::DEPENDS_ON_SKIPPED,
            reasons::ALREADY_PRESENT,
            reasons::ID_COLLISION,
            reasons::UNDOES_TARGET_RECORDS,
        ],
        [
            "REMOVED",
            "SAME_EDGE",
            "SAME_PARAM",
            "DEPENDS_ON_SKIPPED",
            "ALREADY_PRESENT",
            "ID_COLLISION",
            "UNDOES_TARGET_RECORDS",
        ]
    );
}

fn on(tracks: &[&str], touches: Vec<(&str, Touch)>) -> Footprint {
    Footprint {
        tracks: tracks.iter().map(|t| t.to_string()).collect(),
        touches: touches
            .into_iter()
            .map(|(x, t)| (x.to_string(), t))
            .collect(),
        ..Footprint::default()
    }
}

#[test]
fn same_param_conflicts_and_disjoint_tracks_never_do() {
    let gain = || Touch::Param("gain".into());
    let ours = on(&["t1"], vec![("c1", gain())]);
    assert_eq!(
        entity_conflict(&ours, &[("tx-1", on(&["t1"], vec![("c1", gain())]))]),
        Some((reasons::SAME_PARAM, "tx-1".into()))
    );
    assert_eq!(
        entity_conflict(
            &ours,
            &[(
                "tx-1",
                on(&["t1"], vec![("c1", Touch::Param("pan".into()))])
            )]
        ),
        None
    );
    // Disjoint track sets never conflict, whatever the entities.
    assert_eq!(
        entity_conflict(&ours, &[("tx-1", on(&["t2"], vec![("c1", gain())]))]),
        None
    );
    let mut gone = on(&["t2"], vec![("c1", Touch::Exists)]);
    gone.removes = vec!["c1".into()];
    assert_eq!(entity_conflict(&ours, &[("tx-1", gone)]), None);
    // An op bound to no track (an asset op) is judged by its entities alone.
    let mut asset_gone = on(&[], vec![("a1", Touch::Exists)]);
    asset_gone.removes = vec!["a1".into()];
    let mut uses = on(&["t1"], vec![]);
    uses.uses = vec!["a1".into()];
    assert_eq!(
        entity_conflict(&uses, &[("tx-9", asset_gone)]),
        removed("tx-9")
    );
}
