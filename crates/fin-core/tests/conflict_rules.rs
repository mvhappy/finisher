//! Per-track, per-entity conflict rules (spec §4.3, Decision 17): footprints and
//! `entity_conflict`.

mod common;

use std::collections::BTreeSet;

use common::moves::{link, move_to, slide, unlink};
use common::*;
use fin_core::rebase::{
    Conflict, Conflicts, Footprint, Touch, committed_footprints, entity_conflict, reasons,
};
use fin_core::{
    AssetMove, BinAdd, BinMove, BinRemove, CommittedTx, CoreError, Edge, InsertMode, MarkerAdd,
    MarkerRemove, MarkerSet, MemIds, Op, Project, TimelineAdd, TimelineRemove, TimelineSet,
    TrackAdd, TrackKind, TrackMove, TrackRemove, TrackSet, apply_internal, apply_tx, resolve_ops,
};
use serde_json::json;

type Verdict = Option<(&'static str, String)>;

/// Commits `ops` at `p` (internally when any op is internal) as `tx_id`.
fn commit(p: &Project, ops: Vec<Op>, tx_id: &str) -> (CommittedTx, Project) {
    let mut t = tx(ops);
    t.tx_id = tx_id.into();
    let mut ids = MemIds::after(p);
    let a = if t.ops.iter().any(Op::is_internal) {
        apply_internal(p, &t, &mut ids)
    } else {
        apply_tx(p, &t, &mut ids)
    }
    .unwrap_or_else(|e| panic!("{tx_id} does not apply: {e}"));
    let c = CommittedTx {
        rev: 1,
        tx: t,
        diff: a.diff,
        bindings: a.bindings,
        link: None,
    };
    (c, a.project)
}

/// Applies `op` alone to `p`: the op resolved, its footprint and the project after it.
fn fp(p: &Project, op: Op) -> (Op, Footprint, Project) {
    let (c, next) = commit(p, vec![op], "tx-1");
    let resolved = resolve_ops(&c.tx.ops, &c.bindings).unwrap().remove(0);
    (resolved, committed_footprints(&c).unwrap().remove(0), next)
}

/// Per op of `ours` (a stale tx based at `p`, its ids allocated as at `p`), its conflict with
/// the records `committed` (applied in order from `p`, named `tx-1`, `tx-2`, ...). Entities an
/// earlier op of ours created are ours.
fn judge_tx(p: &Project, committed: Vec<Vec<Op>>, ours: Vec<Op>) -> Vec<Verdict> {
    let mut cur = p.clone();
    let mut theirs = Vec::new();
    for (i, ops) in committed.into_iter().enumerate() {
        let id = format!("tx-{}", i + 1);
        let (c, next) = commit(&cur, ops, &id);
        for f in committed_footprints(&c).unwrap() {
            theirs.push((id.clone(), f));
        }
        cur = next;
    }
    let theirs: Vec<(&str, Footprint)> = theirs
        .iter()
        .map(|(t, f)| (t.as_str(), f.clone()))
        .collect();
    let (c, _) = commit(p, ours, "tx-ours");
    let mut own = BTreeSet::new();
    let mut out = Vec::new();
    for f in committed_footprints(&c).unwrap() {
        out.push(entity_conflict(&f, &own, &theirs));
        own.extend(f.creates);
    }
    out
}

/// `judge_tx` with one op per committed record and a single op of ours.
fn judge(p: &Project, committed: Vec<Op>, ours: Op) -> Verdict {
    judge_tx(
        p,
        committed.into_iter().map(|o| vec![o]).collect(),
        vec![ours],
    )
    .remove(0)
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
            fin_core::ItemRemove {
                id: "c2".into(),
                mode: fin_core::DeleteMode::Lift,
            }
            .into(),
        ),
        (deleted.project.clone(), restore),
    ];
    for (at, op) in committed {
        let name = op.name();
        let ours_fp = fp(&at, ours.clone()).1;
        let theirs = fp(&at, op).1;
        assert_eq!(
            entity_conflict(&ours_fp, &BTreeSet::new(), &[("tx-1", theirs)]),
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
            vec![
                fin_core::ItemRemove {
                    id: "c1".into(),
                    mode: fin_core::DeleteMode::Ripple,
                }
                .into()
            ],
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
    let (_, add, _) = fp(&project(), av_asset("$a", secs(100)));
    assert!(add.tracks.is_empty());
    assert_eq!(add.creates, vec!["a1".to_string()]);
    let (_, remove, _) = fp(&p, Op::AssetRemove { id: "a1".into() });
    assert!(remove.tracks.is_empty());
    assert_eq!(remove.removes, vec!["a1".to_string()]);
}

#[test]
fn trim_footprint_comes_from_its_diff() {
    let p = base();
    let (_, f, _) = fp(&p, trim("c3", Edge::End, secs(-1)));
    assert_eq!(f.tracks.iter().collect::<Vec<_>>(), vec!["t2"]);
    assert_eq!(f.touches, vec![("c3".to_string(), Touch::Edge(Edge::End))]);
}

/// One op of every kind, applied for real so each footprint comes from its own diff, with a
/// check that the match stays exhaustive: a new op must get a footprint (and a case here)
/// before this compiles.
#[test]
fn every_op_has_a_footprint() {
    let p = base();
    let spare = apply(&p, vec![av_asset("$b", secs(10))]).project;
    let deleted = apply(&p, vec![delete("c3")]);
    // c2 (video) with effects fx1 and fx2, c3 with an opacity keyframe (Task 14).
    let fx = |kind: &str| -> Op {
        fin_core::EffectAdd {
            item: "c2".into(),
            id: format!("${kind}"),
            kind: kind.into(),
            at: None,
            params: Default::default(),
        }
        .into()
    };
    let keyed = |at: i64| -> Op {
        fin_core::KeyframeSet {
            item: "c3".into(),
            path: "opacity".into(),
            at: secs(at),
            value: fin_core::ParamValue::Number(0.5),
            interp: fin_core::Interp::Linear,
        }
        .into()
    };
    let busy = apply(&p, vec![fx("blur"), fx("lut"), keyed(1)]).project;
    let unfx = apply(
        &busy,
        vec![
            fin_core::EffectRemove {
                item: "c2".into(),
                effect: "fx1".into(),
            }
            .into(),
        ],
    );
    let cases = [
        (p.clone(), av_asset("$b", secs(10)), vec![]),
        (spare, Op::AssetRemove { id: "a2".into() }, vec![]),
        (
            p.clone(),
            insert("$n", "t1", secs(10), "a1", secs(0), secs(1)),
            vec!["t1"],
        ),
        (p.clone(), trim("c3", Edge::End, secs(-1)), vec!["t2"]),
        (p.clone(), delete("c3"), vec!["t2"]),
        (
            p.clone(),
            fin_core::ItemRemove {
                id: "c2".into(),
                mode: fin_core::DeleteMode::Ripple,
            }
            .into(),
            vec!["t1"],
        ),
        (
            deleted.project.clone(),
            deleted.inverse[0].clone(),
            vec!["t2"],
        ),
        // Ops that record no shift are on their item's track through their changes' `track`.
        (
            p.clone(),
            fin_core::ItemDelete {
                item: "c3".into(),
                mode: fin_core::DeleteMode::Lift,
            }
            .into(),
            vec!["t2"],
        ),
        (
            p.clone(),
            fin_core::ItemTrim {
                item: "c1".into(),
                edge: Edge::End,
                delta: secs(-1),
                mode: fin_core::TrimMode::Overwrite,
                alone: false,
            }
            .into(),
            vec!["t1"],
        ),
        (
            p.clone(),
            fin_core::ItemSlip {
                item: "c3".into(),
                delta: secs(1),
            }
            .into(),
            vec!["t2"],
        ),
        (
            p.clone(),
            fin_core::ItemSplit {
                item: "c3".into(),
                offset: secs(2),
                id: "$r".into(),
            }
            .into(),
            vec!["t2"],
        ),
        (
            p.clone(),
            fin_core::ParamSet {
                item: "c3".into(),
                path: "opacity".into(),
                value: Some(fin_core::ParamValue::Number(0.5)),
            }
            .into(),
            vec!["t2"],
        ),
        (p.clone(), fx("blur"), vec!["t1"]),
        (
            busy.clone(),
            fin_core::EffectRemove {
                item: "c2".into(),
                effect: "fx1".into(),
            }
            .into(),
            vec!["t1"],
        ),
        (
            busy.clone(),
            fin_core::EffectMove {
                item: "c2".into(),
                effect: "fx1".into(),
                to: 1,
            }
            .into(),
            vec!["t1"],
        ),
        (busy.clone(), keyed(2), vec!["t2"]),
        (
            busy.clone(),
            fin_core::KeyframeRemove {
                item: "c3".into(),
                path: "opacity".into(),
                at: secs(1),
            }
            .into(),
            vec!["t2"],
        ),
        (unfx.project.clone(), unfx.inverse[0].clone(), vec!["t1"]),
    ];
    let linked = apply(&p, vec![link(&["c1", "c3"], "$g")]).project;
    let more = [
        (
            p.clone(),
            move_to("c1", "t2", secs(5), InsertMode::Ripple),
            vec!["t1", "t2"],
        ),
        (p.clone(), slide("c2", secs(-1)), vec!["t1"]),
        (p.clone(), link(&["c1", "c3"], "$g"), vec!["t1", "t2"]),
        (linked, unlink("lk1"), vec!["t1", "t2"]),
    ];
    // Transitions need a cut with handles: c2 reads from 5 s on.
    let cut = with_sourced(&[(5, 0), (5, 5)]);
    let set = |id: &str| -> Op {
        fin_core::TransitionSet {
            id: id.into(),
            track: "t1".into(),
            between: ["c1".into(), "c2".into()],
            kind: fin_core::TransitionKind::Dissolve,
            duration: secs(1),
            curve: fin_core::Curve::Linear,
        }
        .into()
    };
    let dissolved = apply(&cut, vec![set("$t")]).project;
    let transitions = [
        (cut, set("$t"), vec!["t1"]),
        (dissolved.clone(), set("tr1"), vec!["t1"]),
        (
            dissolved,
            fin_core::TransitionRemove {
                transition: "tr1".into(),
            }
            .into(),
            vec!["t1"],
        ),
    ];
    let tl2 = apply(&p, vec![timeline_add()]).project;
    let no_t2 = apply(&p, vec![track_remove()]);
    let no_tl2 = apply(
        &tl2,
        vec![
            TimelineRemove {
                timeline: "tl2".into(),
            }
            .into(),
        ],
    );
    let structure = [
        (
            p.clone(),
            TrackAdd {
                id: "$t".into(),
                timeline: "tl1".into(),
                kind: TrackKind::Video,
                index: None,
                name: None,
            }
            .into(),
            vec!["t3"],
        ),
        (p.clone(), track_remove(), vec!["t2"]),
        (no_t2.project, no_t2.inverse[0].clone(), vec!["t2"]),
        (
            p.clone(),
            TrackMove {
                track: "t1".into(),
                index: 1,
            }
            .into(),
            vec!["t1"],
        ),
        (
            p.clone(),
            TrackSet {
                track: "t1".into(),
                path: "mute".into(),
                value: Some(fin_core::ParamValue::Bool(true)),
            }
            .into(),
            vec!["t1"],
        ),
        (p.clone(), timeline_add(), vec![]),
        (
            tl2.clone(),
            TimelineSet {
                timeline: "tl2".into(),
                path: "name".into(),
                value: Some(fin_core::ParamValue::Text("Teaser".into())),
            }
            .into(),
            vec![],
        ),
        (
            tl2,
            TimelineRemove {
                timeline: "tl2".into(),
            }
            .into(),
            vec![],
        ),
        (no_tl2.project, no_tl2.inverse[0].clone(), vec![]),
    ];
    let marker = || -> Op {
        MarkerAdd {
            id: "$m".into(),
            timeline: "tl1".into(),
            at: secs(1),
            duration: None,
            label: "Cue".into(),
            color: None,
        }
        .into()
    };
    let bin = |id: &str, parent: Option<&str>| -> Op {
        BinAdd {
            id: id.into(),
            name: "Bin".into(),
            parent: parent.map(str::to_string),
        }
        .into()
    };
    let lib = apply(&p, vec![marker(), bin("$b", None), bin("$c", None)]).project;
    let library = [
        (p.clone(), marker(), vec![]),
        (
            lib.clone(),
            MarkerSet {
                marker: "m1".into(),
                path: "label".into(),
                value: Some(fin_core::ParamValue::Text("Intro".into())),
            }
            .into(),
            vec![],
        ),
        (
            lib.clone(),
            MarkerRemove {
                marker: "m1".into(),
            }
            .into(),
            vec![],
        ),
        (p.clone(), bin("$b", None), vec![]),
        (
            lib.clone(),
            BinMove {
                bin: "b2".into(),
                parent: Some("b1".into()),
            }
            .into(),
            vec![],
        ),
        (lib.clone(), BinRemove { bin: "b2".into() }.into(), vec![]),
        (
            lib,
            AssetMove {
                asset: "a1".into(),
                bin: Some("b1".into()),
            }
            .into(),
            vec![],
        ),
    ];
    let styles_fonts = styles_and_fonts(&p).into_iter().chain(caption_edits(&p));
    let mut seen = BTreeSet::new();
    for (at, op, tracks) in cases
        .into_iter()
        .chain(more)
        .chain(transitions)
        .chain(structure)
        .chain(library)
        .chain(styles_fonts)
    {
        let (op, f, _) = fp(&at, op);
        seen.insert(op.name());
        assert_eq!(
            f.tracks.iter().map(String::as_str).collect::<Vec<_>>(),
            tracks,
            "{}",
            op.name()
        );
        let want = match &op {
            Op::AssetAdd(a) => f.creates == [a.id.clone()] && f.touches.is_empty(),
            Op::AssetRemove { id } => {
                f.removes == [id.clone()] && f.touches == [(id.clone(), Touch::Exists)]
            }
            Op::ClipInsert {
                id, track, asset, ..
            } => f.creates == [id.clone()] && f.uses == [asset.clone(), track.clone()],
            Op::ItemInsert(i) => {
                f.creates == [i.id.clone()]
                    && f.uses == [i.body.asset().unwrap().to_string(), i.track.clone()]
            }
            Op::ClipTrim { clip, edge, .. } => f.touches == [(clip.clone(), Touch::Edge(*edge))],
            Op::ItemTrim(t) => f.touches == [(t.item.clone(), Touch::Edge(t.edge))],
            Op::ClipDelete { clip: id }
            | Op::ItemDelete(fin_core::ItemDelete { item: id, .. })
            | Op::ItemRemove(fin_core::ItemRemove { id, .. }) => {
                f.removes == [id.clone()] && f.touches == [(id.clone(), Touch::Exists)]
            }
            Op::ItemRestore(r) => {
                f.creates == [r.item.id.clone()]
                    && f.uses == [r.item.asset().unwrap().to_string(), r.track.clone()]
            }
            Op::ItemSlip(s) => f.touches == [(s.item.clone(), Touch::Param("sourceIn".into()))],
            Op::ItemMove(m) => {
                f.touches == [(m.item.clone(), Touch::Param("start".into()))]
                    && f.uses == [m.track.clone()]
            }
            Op::ItemSlide(s) => {
                f.touches
                    == [
                        (s.item.clone(), Touch::Param("start".into())),
                        ("c1".to_string(), Touch::Edge(Edge::End)),
                    ]
            }
            Op::ItemLink(l) => {
                f.creates == [l.id.clone()]
                    && f.touches
                        == l.items
                            .iter()
                            .map(|i| (i.clone(), Touch::Param("link".into())))
                            .collect::<Vec<_>>()
            }
            Op::ItemUnlink(u) => {
                f.removes == [u.group.clone()]
                    && f.touches
                        == [
                            (u.group.clone(), Touch::Exists),
                            ("c1".to_string(), Touch::Param("link".into())),
                            ("c3".to_string(), Touch::Param("link".into())),
                        ]
            }
            Op::ItemSplit(s) => {
                f.touches
                    == [
                        (s.item.clone(), Touch::Exists),
                        (s.item.clone(), Touch::Edge(Edge::End)),
                    ]
                    && f.creates == [s.id.clone()]
            }
            Op::TransitionSet(s) => {
                let mut want = vec![
                    ("c1".to_string(), Touch::Param("transition.out".into())),
                    ("c2".to_string(), Touch::Param("transition.in".into())),
                ];
                // An update touches the transition; a create creates it.
                let update = f.creates.is_empty();
                if update {
                    want.push((s.id.clone(), Touch::Exists));
                }
                f.touches == want && f.uses == [s.track.clone()] && (update || f.creates == ["tr1"])
            }
            Op::TransitionRemove(r) => {
                f.removes == [r.transition.clone()]
                    && f.touches == [(r.transition.clone(), Touch::Exists)]
            }
            // Ordered lists are entities (`ops::util::lists`): every placement uses the list,
            // and a put-back at an old index puts the list back.
            Op::TrackAdd(a) => {
                f.creates == [a.id.clone()] && f.uses == [a.timeline.as_str(), "#tracks"]
            }
            Op::TrackRemove(r) => {
                f.removes == [r.track.clone(), "c3".into()]
                    && f.touches == [(r.track.clone(), Touch::Exists)]
                    && f.uses == ["#tracks"]
            }
            Op::TrackRestore(r) => {
                f.creates == [r.track.id.clone(), "c3".into()]
                    && f.uses == ["tl1", "#tracks", "a1"]
                    && f.overwrites.contains(&"#tracks".to_string())
            }
            Op::TrackMove(m) => {
                f.touches == [(m.track.clone(), Touch::Param("index".into()))]
                    && f.uses == ["#tracks"]
            }
            Op::TrackSet(s) => f.touches == [(s.track.clone(), Touch::Param(s.path.clone()))],
            Op::TimelineAdd(a) => {
                f.creates == [a.id.clone()] && f.touches.is_empty() && f.uses == ["#timelines"]
            }
            Op::TimelineSet(s) => f.touches == [(s.timeline.clone(), Touch::Param(s.path.clone()))],
            Op::TimelineRemove(r) => {
                f.removes == [r.timeline.clone()]
                    && f.touches == [(r.timeline.clone(), Touch::Exists)]
                    && f.uses == ["#timelines"]
            }
            Op::TimelineRestore(r) => {
                f.creates == [r.timeline.id.clone()]
                    && f.overwrites.contains(&"#timelines".to_string())
            }
            Op::MarkerAdd(a) => f.creates == [a.id.clone()] && f.uses == [a.timeline.clone()],
            Op::MarkerSet(s) => f.touches == [(s.marker.clone(), Touch::Param(s.path.clone()))],
            Op::MarkerRemove(r) => {
                f.removes == [r.marker.clone()] && f.touches == [(r.marker.clone(), Touch::Exists)]
            }
            Op::BinAdd(a) => f.creates == [a.id.clone()] && f.uses.is_empty(),
            Op::BinMove(m) => {
                f.touches == [(m.bin.clone(), Touch::Param("parent".into()))] && f.uses == ["b1"]
            }
            Op::BinRemove(r) => {
                f.removes == [r.bin.clone()] && f.touches == [(r.bin.clone(), Touch::Exists)]
            }
            Op::AssetMove(m) => {
                f.touches == [(m.asset.clone(), Touch::Param("bin".into()))] && f.uses == ["b1"]
            }
            Op::ParamSet(s) => f.touches == [(s.item.clone(), Touch::Param(s.path.clone()))],
            Op::EffectAdd(a) => {
                f.creates == [format!("c2/{}", a.id)]
                    && f.touches == [("c2".to_string(), Touch::Exists)]
                    && f.uses == ["c2#effects"]
            }
            Op::EffectRemove(r) => {
                f.touches
                    == [
                        (format!("c2/{}", r.effect), Touch::Exists),
                        ("c2".to_string(), Touch::Exists),
                    ]
                    && f.removes == [format!("c2/{}", r.effect)]
                    && f.uses == ["c2#effects"]
            }
            Op::EffectMove(m) => {
                f.touches == [("c2".to_string(), Touch::Param("effects".into()))]
                    && f.uses == [format!("c2/{}", m.effect), "c2#effects".into()]
            }
            Op::KeyframeSet(k) => {
                f.touches == [("c3".to_string(), Touch::Param(format!("opacity@{}", k.at)))]
            }
            // The path's last keyframe: the path itself changes too.
            Op::KeyframeRemove(k) => {
                f.touches
                    == [
                        ("c3".to_string(), Touch::Param(format!("opacity@{}", k.at))),
                        ("c3".to_string(), Touch::Param("opacity".into())),
                    ]
            }
            Op::EffectRestore(r) => {
                f.creates == [format!("c2/{}", r.effect.id)]
                    && f.touches == [("c2".to_string(), Touch::Param("effects".into()))]
                    && f.overwrites.contains(&"c2#effects".to_string())
            }
            Op::StyleAdd(a) => f.creates == [a.id.clone()] && f.uses.is_empty(),
            Op::StyleSet(s) => f.touches == [(s.style.clone(), Touch::Param(s.path.clone()))],
            Op::StyleRemove(r) => {
                f.removes == [r.style.clone()] && f.touches == [(r.style.clone(), Touch::Exists)]
            }
            Op::FontPin(p) => {
                let placed: &[&str] = if p.fallback.is_some() {
                    &["#fonts.fallback"]
                } else {
                    &[]
                };
                f.creates == [format!("font/{}", p.key)] && f.uses == placed
            }
            Op::FontUnpin(u) => {
                // Its inverse pins the old file back: it meets a repin or a reorder (spec §4.3).
                let e = format!("font/{}", u.key);
                f.removes == [e.clone()]
                    && f.touches
                        == [
                            (e.clone(), Touch::Exists),
                            (e.clone(), Touch::Param("sha256".into())),
                            (e, Touch::Param("fallback".into())),
                        ]
                    && f.uses == ["#fonts.fallback"]
            }
            Op::FontRepin(r) => {
                f.touches == [(format!("font/{}", r.key), Touch::Param("sha256".into()))]
            }
            Op::CaptionSetText(fin_core::CaptionSetText { item, .. })
            | Op::CaptionSetWords(fin_core::CaptionSetWords { item, .. }) => {
                f.touches == [(item.clone(), Touch::Param("caption.words".into()))]
            }
            Op::CaptionSplit(s) => {
                f.touches
                    == [
                        (s.item.clone(), Touch::Exists),
                        (s.item.clone(), Touch::Edge(Edge::End)),
                        (s.item.clone(), Touch::Param("caption.words".into())),
                        (s.item.clone(), Touch::Param("caption.style".into())),
                    ]
                    && f.creates == [s.id.clone()]
            }
            Op::CaptionMerge(m) => {
                f.touches
                    == [
                        (m.item.clone(), Touch::Exists),
                        (m.with.clone(), Touch::Exists),
                        (m.item.clone(), Touch::Edge(Edge::End)),
                        (m.item.clone(), Touch::Param("caption.words".into())),
                    ]
                    && f.removes == [m.with.clone()]
            }
            Op::CaptionSetStyle(s) => {
                f.touches == [(s.item.clone(), Touch::Param("caption.style".into()))]
                    && f.uses.is_empty()
            }
        };
        assert!(want, "{}: {f:?}", op.name());
        assert!(
            !f.creates.iter().chain(&f.uses).any(|x| x.starts_with('$')),
            "{f:?}"
        );
    }
    assert_eq!(seen.len(), 49);
}

/// The style and font ops (plan 4C Task 18), each at a project where it applies; none is on
/// a track.
fn styles_and_fonts(p: &Project) -> Vec<(Project, Op, Vec<&'static str>)> {
    use fin_core::{FontFormat, FontPin, FontPinOp, FontRepin, FontSource, FontUnpin};
    use fin_core::{StyleAdd, StyleRemove, StyleSet};
    let file = |c: char| FontPin {
        family: "Inter".into(),
        weight: 700,
        italic: false,
        face: 0,
        sha256: std::iter::repeat_n(c, 64).collect(),
        size: 9,
        format: FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: FontSource::Catalog {
            entry: "inter".into(),
            style: "bold".into(),
            catalog: "2026.11.1".into(),
        },
    };
    let key = "Inter|700|normal".to_string();
    let add: Op = StyleAdd {
        id: "$st".into(),
        name: "S".into(),
        from: Some("preset:bold".into()),
        set: Default::default(),
    }
    .into();
    let pin: Op = FontPinOp {
        key: key.clone(),
        pin: file('a'),
        fallback: None,
    }
    .into();
    let styled = apply(p, vec![add.clone()]).project;
    let pinned = apply(p, vec![pin.clone()]).project;
    vec![
        (p.clone(), add, vec![]),
        (
            styled.clone(),
            StyleSet {
                style: "st1".into(),
                path: "color".into(),
                value: None,
            }
            .into(),
            vec![],
        ),
        (
            styled,
            StyleRemove {
                style: "st1".into(),
            }
            .into(),
            vec![],
        ),
        (p.clone(), pin, vec![]),
        (pinned.clone(), FontUnpin::new(key.clone()).into(), vec![]),
        (
            pinned,
            FontRepin {
                key,
                pin: file('b'),
            }
            .into(),
            vec![],
        ),
    ]
}

fn timeline_add() -> Op {
    TimelineAdd {
        id: "$tl".into(),
        name: "B-roll".into(),
        rate: secs(30),
        resolution: fin_core::Resolution {
            width: 1080,
            height: 1920,
        },
    }
    .into()
}

fn track_remove() -> Op {
    TrackRemove {
        track: "t2".into(),
        with_items: true,
    }
    .into()
}

#[test]
fn conflict_into_core_error_keeps_4a_fields() {
    let first = Conflict::new("tx-late", 1, reasons::REMOVED, Some("tx-3".into()));
    let second = Conflict::new("tx-late", 2, reasons::SAME_EDGE, Some("tx-4".into()));
    let e = CoreError::from(Conflicts::new(vec![first.clone(), second.clone()]).unwrap());
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
    let e = CoreError::from(Conflicts::one(c));
    assert_eq!(e.details["againstTx"], serde_json::Value::Null);
    assert_eq!(
        e.details["conflicts"][0]["againstTx"],
        serde_json::Value::Null
    );
}

#[test]
fn rerun_conflict_takes_the_policy_code() {
    let e = CoreError::new("INSERT_INSIDE_CLIP", "inside c2")
        .with_details(json!({ "clip": "c2", "opIndex": 3 }))
        .with_fix("insert at a boundary");
    let c = Conflict::from_rerun(&e, "tx-b", 3, Some("tx-2".into()));
    assert_eq!(
        c,
        Conflict {
            tx_id: "tx-b".into(),
            op_index: 3,
            reason: "INSERT_INSIDE_CLIP".into(),
            against_tx: Some("tx-2".into()),
            fix: "insert at a boundary".into(),
            details: Some(Box::new(json!({ "clip": "c2" }))),
        }
    );
    // Entity conflicts carry no details, and none are serialised.
    let removed = Conflict::new("tx-b", 0, reasons::REMOVED, None);
    assert!(
        serde_json::to_value(&removed)
            .unwrap()
            .get("details")
            .is_none()
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

fn none() -> BTreeSet<String> {
    BTreeSet::new()
}

#[test]
fn same_param_conflicts() {
    let gain = || Touch::Param("gain".into());
    let ours = on(&["t1"], vec![("c1", gain())]);
    assert_eq!(
        entity_conflict(
            &ours,
            &none(),
            &[("tx-1", on(&["t1"], vec![("c1", gain())]))]
        ),
        Some((reasons::SAME_PARAM, "tx-1".into()))
    );
    let pan = on(&["t1"], vec![("c1", Touch::Param("pan".into()))]);
    assert_eq!(entity_conflict(&ours, &none(), &[("tx-1", pan)]), None);
}

/// An entity's edits meet wherever it is: once items move between tracks (4C), an edit on t1
/// and one on t2 can share an entity, and the entity rules still apply.
#[test]
fn entity_rules_ignore_tracks() {
    let gain = || Touch::Param("gain".into());
    let ours = on(&["t1"], vec![("c1", gain())]);
    assert_eq!(
        entity_conflict(
            &ours,
            &none(),
            &[("tx-1", on(&["t2"], vec![("c1", gain())]))]
        ),
        Some((reasons::SAME_PARAM, "tx-1".into()))
    );
    let mut gone = on(&["t2"], vec![("c1", Touch::Exists)]);
    gone.removes = vec!["c1".into()];
    assert_eq!(
        entity_conflict(&ours, &none(), &[("tx-1", gone)]),
        removed("tx-1")
    );
    // An op bound to no track (an asset op) is judged by its entities too.
    let mut asset_gone = on(&[], vec![("a1", Touch::Exists)]);
    asset_gone.removes = vec!["a1".into()];
    let mut uses = on(&["t1"], vec![]);
    uses.uses = vec!["a1".into()];
    assert_eq!(
        entity_conflict(&uses, &none(), &[("tx-9", asset_gone)]),
        removed("tx-9")
    );
}

#[test]
fn earliest_standing_removal_is_named_across_entities() {
    let ours = on(&["t1"], vec![("c1", Touch::Exists), ("c2", Touch::Exists)]);
    let gone = |x: &str| {
        let mut f = on(&["t1"], vec![(x, Touch::Exists)]);
        f.removes = vec![x.into()];
        f
    };
    // c2 went first, though ours names c1 first.
    assert_eq!(
        entity_conflict(
            &ours,
            &none(),
            &[("tx-1", gone("c2")), ("tx-2", gone("c1"))]
        ),
        removed("tx-1")
    );
}

#[test]
fn remove_restore_remove_names_the_last_removal() {
    let p = base();
    let deleted = apply(&p, vec![delete("c1")]);
    let restore = deleted.inverse[0].clone();
    assert_eq!(
        judge(
            &p,
            vec![delete("c1"), restore, delete("c1")],
            trim("c1", Edge::End, secs(-1))
        ),
        removed("tx-3")
    );
}

#[test]
fn of_two_same_edge_records_the_earliest_is_named() {
    let p = base();
    assert_eq!(
        judge(
            &p,
            vec![
                trim("c1", Edge::End, secs(-1)),
                trim("c1", Edge::End, secs(-1))
            ],
            trim("c1", Edge::End, secs(-1))
        ),
        Some((reasons::SAME_EDGE, "tx-1".into()))
    );
}

/// At a stale base our placeholders bind the ids the intervening records allocated (the same
/// counters): `$n` binds c4 there, as their insert did. Our own entity is never judged against
/// theirs.
#[test]
fn own_new_clip_is_not_their_removed_clip() {
    let p = base();
    let theirs_insert = vec![insert("$x", "t1", secs(10), "a1", secs(0), secs(3))];
    let undo = apply(&p, theirs_insert.clone()).inverse;
    let ours = vec![
        insert("$n", "t1", secs(0), "a1", secs(0), secs(4)),
        trim("$n", Edge::End, secs(-1)),
    ];
    assert_eq!(
        judge_tx(&p, vec![theirs_insert, undo], ours),
        vec![None, None]
    );
}

#[test]
fn own_new_clip_is_not_their_trimmed_clip() {
    let p = base();
    let theirs = vec![
        insert("$x", "t1", secs(10), "a1", secs(0), secs(3)),
        trim("$x", Edge::End, secs(-1)),
    ];
    let ours = vec![
        insert("$n", "t1", secs(0), "a1", secs(0), secs(4)),
        trim("$n", Edge::End, secs(-1)),
    ];
    assert_eq!(judge_tx(&p, vec![theirs], ours), vec![None, None]);
}

#[test]
fn own_new_asset_is_not_their_removed_asset() {
    let p = base();
    let theirs_add = vec![av_asset("$b", secs(10))];
    let undo = apply(&p, theirs_add.clone()).inverse;
    let ours = vec![
        av_asset("$a", secs(10)),
        insert("$n", "t1", secs(10), "$a", secs(0), secs(4)),
    ];
    assert_eq!(judge_tx(&p, vec![theirs_add, undo], ours), vec![None, None]);
}

#[test]
fn an_existing_entity_ours_names_is_still_judged() {
    let p = base();
    // Ours inserts c4 and then trims c1 (not ours): c1's removal still counts.
    let ours = vec![
        insert("$n", "t1", secs(10), "a1", secs(0), secs(4)),
        trim("c1", Edge::End, secs(-1)),
    ];
    assert_eq!(
        judge_tx(&p, vec![vec![delete("c1")]], ours),
        vec![None, removed("tx-1")]
    );
}

/// The caption edits (plan 4C Task 19) on caption track `t9`: `cap1` "a b" [0, 2) and `cap2`
/// "c" [2, 3).
fn caption_edits(p: &Project) -> Vec<(Project, Op, Vec<&'static str>)> {
    use fin_core::{CaptionBlock, ItemBody, ItemInsert, Word};
    use fin_core::{CaptionMerge, CaptionSetStyle, CaptionSetText, CaptionSetWords, CaptionSplit};
    let mut q = p.clone();
    q.timelines[0]
        .tracks
        .push(fin_core::Track::new("t9", TrackKind::Caption, "C9"));
    let word = |t: &str, s: i64, e: i64| Word {
        text: t.into(),
        start: secs(s),
        end: secs(e),
    };
    let cap = |id: &str, at: i64, words: Vec<Word>| -> Op {
        let duration = words.last().map_or(secs(1), |w| w.end);
        ItemInsert {
            id: id.into(),
            track: "t9".into(),
            at: secs(at),
            duration,
            mode: InsertMode::Overwrite,
            body: ItemBody::Caption {
                block: CaptionBlock {
                    words,
                    style: "preset:bold".into(),
                    overrides: Default::default(),
                    source: None,
                },
            },
        }
        .into()
    };
    let q = apply(
        &q,
        vec![
            cap("$a", 0, vec![word("a", 0, 1), word("b", 1, 2)]),
            cap("$b", 2, vec![word("c", 0, 1)]),
        ],
    )
    .project;
    vec![
        (
            q.clone(),
            CaptionSetText {
                item: "cap1".into(),
                text: "x y".into(),
            }
            .into(),
            vec!["t9"],
        ),
        (
            q.clone(),
            CaptionSetWords {
                item: "cap1".into(),
                words: vec![word("z", 0, 2)],
                source: None,
            }
            .into(),
            vec!["t9"],
        ),
        (
            q.clone(),
            CaptionSplit {
                item: "cap1".into(),
                word: 1,
                id: "$cap".into(),
            }
            .into(),
            vec!["t9"],
        ),
        (
            q.clone(),
            CaptionMerge {
                item: "cap1".into(),
                with: "cap2".into(),
            }
            .into(),
            vec!["t9"],
        ),
        (
            q,
            CaptionSetStyle {
                item: "cap1".into(),
                style: "preset:clean".into(),
                overrides: None,
            }
            .into(),
            vec!["t9"],
        ),
    ]
}
