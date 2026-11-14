//! Track and timeline ops (plan 4C Task 12): `track.add|remove|restore|move|set` and
//! `timeline.add|set|remove|restore`. None records a shift; removals record every entity they
//! take out, and their restores bring it all back.

mod common;

use common::ids_json::ids_cover_every_id_in_json;
use common::memlog::MemLog;
use common::moves::link;
use common::stale::{conflict_reason, rebase_over};
use common::*;
use fin_core::rebase::{Touch, footprint, split_diff};
use fin_core::{
    ApplyMode, Change, ChangeKind, CoreError, Curve, EntityKind, Item, ItemBody, Marker, MemIds,
    Op, ParamValue, Project, Rational, Resolution, TimelineAdd, TimelineRemove, TimelineSet,
    TrackAdd, TrackKind, TrackMove, TrackRemove, TrackSet, TransitionKind, TransitionSet,
    apply_tx_mode,
};

fn add_track(timeline: &str, kind: TrackKind, index: Option<u32>, name: Option<&str>) -> Op {
    TrackAdd {
        id: "$t".into(),
        timeline: timeline.into(),
        kind,
        index,
        name: name.map(str::to_string),
    }
    .into()
}

fn remove_track(track: &str, with_items: bool) -> Op {
    TrackRemove {
        track: track.into(),
        with_items,
    }
    .into()
}

fn move_track(track: &str, index: u32) -> Op {
    TrackMove {
        track: track.into(),
        index,
    }
    .into()
}

fn set_track(track: &str, path: &str, value: Option<ParamValue>) -> Op {
    TrackSet {
        track: track.into(),
        path: path.into(),
        value,
    }
    .into()
}

fn add_timeline(id: &str, rate: i64) -> Op {
    TimelineAdd {
        id: id.into(),
        name: "B-roll".into(),
        rate: secs(rate),
        resolution: Resolution {
            width: 1920,
            height: 1080,
        },
    }
    .into()
}

fn set_timeline(timeline: &str, path: &str, value: Option<ParamValue>) -> Op {
    TimelineSet {
        timeline: timeline.into(),
        path: path.into(),
        value,
    }
    .into()
}

fn remove_timeline(timeline: &str) -> Op {
    TimelineRemove {
        timeline: timeline.into(),
    }
    .into()
}

fn change(kind: EntityKind, id: &str, change: ChangeKind, track: Option<&str>) -> Change {
    Change {
        kind,
        id: id.into(),
        change,
        op: 0,
        track: track.map(str::to_string),
        edge: None,
        path: None,
        by: None,
        timeline: None,
    }
}

fn param(kind: EntityKind, id: &str, path: &str) -> Change {
    Change {
        path: Some(path.into()),
        ..change(kind, id, ChangeKind::Updated, None)
    }
}

/// `c` naming the track's timeline `tl1` (`Change::timeline`: a `track.move` or `track.remove`).
fn on_tl1(c: Change) -> Change {
    Change {
        timeline: Some("tl1".into()),
        ..c
    }
}

fn text(s: &str) -> Option<ParamValue> {
    Some(ParamValue::Text(s.into()))
}

/// `(id, name)` of every track of timeline `tl`.
fn tracks(p: &Project, tl: usize) -> Vec<(String, String)> {
    p.timelines[tl]
        .tracks
        .iter()
        .map(|t| (t.id.clone(), t.name.clone()))
        .collect()
}

fn pair(id: &str, name: &str) -> (String, String) {
    (id.into(), name.into())
}

fn replay(p: &Project, ops: Vec<Op>) -> Result<fin_core::Applied, CoreError> {
    apply_tx_mode(p, &tx(ops), &mut MemIds::after(p), ApplyMode::Replay)
}

fn internal(p: &Project, ops: Vec<Op>) -> Result<fin_core::Applied, CoreError> {
    fin_core::apply_internal(p, &tx(ops), &mut MemIds::after(p))
}

fn code<T: std::fmt::Debug>(r: Result<T, CoreError>) -> String {
    r.unwrap_err().code
}

/// The footprint of the one op `a` applied (`op` resolved), from its own diff.
fn fp(op: &Op, a: &fin_core::Applied) -> fin_core::rebase::Footprint {
    footprint(
        op,
        &split_diff(std::slice::from_ref(op), &a.diff).unwrap()[0],
    )
}

/// `t1`: `c1` [0,4) from 0, `c2` [4,8) from 10, `c3` [8,12) from 20 with a 1 s dissolve `tr1`
/// at the `c1`|`c2` cut; `t2`: audio clip `c4` [0,4) linked with `c1` (`lk1`).
fn busy() -> Project {
    let p = with_sourced(&[(4, 0), (4, 10), (4, 20)]);
    let ops = vec![
        insert("$au", "t2", secs(0), "a1", secs(0), secs(4)),
        link(&["c1", "$au"], "$lk"),
        TransitionSet {
            id: "$tr".into(),
            track: "t1".into(),
            between: ["c1".into(), "c2".into()],
            kind: TransitionKind::Dissolve,
            duration: secs(1),
            curve: Curve::Linear,
        }
        .into(),
    ];
    let p = apply(&p, ops).project;
    assert_eq!(p.timelines[0].tracks[1].items[0].id, "c4");
    p
}

/// [`busy`] with timeline `tl2` (25 fps): video track `t3` holding clip `c5` [0,2), and
/// marker `m1` at 1 s.
fn two_timelines() -> Project {
    let ops = vec![
        add_timeline("$tl", 25),
        TrackAdd {
            id: "$v".into(),
            timeline: "$tl".into(),
            kind: TrackKind::Video,
            index: None,
            name: None,
        }
        .into(),
        insert("$c", "$v", secs(0), "a1", secs(0), secs(2)),
    ];
    let mut p = apply(&busy(), ops).project;
    p.timelines[1].markers.push(Marker {
        id: "m1".into(),
        at: secs(1),
        duration: None,
        label: "Cue".into(),
        color: Some("#ff0000".into()),
    });
    assert_eq!(tracks(&p, 1), [pair("t3", "V1")]);
    p
}

#[test]
fn track_add_appends_or_inserts() {
    let p = project();
    let a = golden_apply(&p, vec![add_track("tl1", TrackKind::Video, None, None)]);
    assert_eq!(a.bindings["$t"], "t3");
    assert_eq!(
        tracks(&a.project, 0),
        [pair("t1", "V1"), pair("t2", "A1"), pair("t3", "V2")]
    );
    assert_eq!(
        a.diff.changes,
        [change(EntityKind::Track, "t3", ChangeKind::Added, None)]
    );
    assert!(a.diff.shifts.is_empty());
    assert_eq!(a.inverse, [remove_track("t3", false)]);
    let f = fp(
        &TrackAdd {
            id: "t3".into(),
            timeline: "tl1".into(),
            kind: TrackKind::Video,
            index: None,
            name: None,
        }
        .into(),
        &a,
    );
    // The track order is an entity it uses (`ops::util::lists`); no index, so no put-back.
    assert_eq!(
        (f.creates, f.uses),
        (vec!["t3".into()], vec!["tl1".into(), "tl1#tracks".into()])
    );
    assert!(f.overwrites.is_empty());
    // At the bottom, named.
    let b = golden_apply(
        &p,
        vec![add_track("tl1", TrackKind::Caption, Some(0), Some("Subs"))],
    );
    assert_eq!(
        tracks(&b.project, 0),
        [pair("t3", "Subs"), pair("t1", "V1"), pair("t2", "A1")]
    );
    // Refusals.
    let past = add_track("tl1", TrackKind::Video, Some(3), None);
    assert_eq!(code(try_apply(&p, vec![past.clone()])), "INVALID_RANGE");
    assert_eq!(code(replay(&p, vec![past])), "INVALID_RANGE");
    let nowhere = add_track("tl9", TrackKind::Video, None, None);
    assert_eq!(code(try_apply(&p, vec![nowhere])), "NOT_FOUND");
    let long = "x".repeat(65);
    let long = add_track("tl1", TrackKind::Video, None, Some(&long));
    assert_eq!(code(try_apply(&p, vec![long])), "INVALID_RANGE");
}

#[test]
fn track_remove_takes_an_empty_track_off() {
    let p = with_clips(&[4]);
    let a = golden_apply(&p, vec![remove_track("t2", false)]);
    assert_eq!(tracks(&a.project, 0), [pair("t1", "V1")]);
    assert_eq!(
        a.diff.changes,
        [on_tl1(change(
            EntityKind::Track,
            "t2",
            ChangeKind::Removed,
            None
        ))]
    );
    let Op::TrackRestore(r) = &a.inverse[0] else {
        panic!("{:?}", a.inverse)
    };
    assert_eq!(r.track, p.timelines[0].tracks[1]);
    assert_eq!((r.timeline.as_str(), r.index), ("tl1", 1));
    ids_cover_every_id_in_json(&a.inverse[0]);
    // An undo then a redo.
    let f = fp(&remove_track("t2", false), &a);
    assert_eq!(f.removes, ["t2"]);
    assert_eq!(f.touches, [("t2".to_string(), Touch::Exists)]);
}

#[test]
fn track_remove_refusals() {
    let p = with_clips(&[4]);
    // A track holding items needs withItems, in every mode.
    for r in [
        try_apply(&p, vec![remove_track("t1", false)]),
        replay(&p, vec![remove_track("t1", false)]),
    ] {
        let e = r.unwrap_err();
        assert_eq!(e.code, "TRACK_NOT_EMPTY");
        assert_eq!(e.details["items"], serde_json::json!(["c1"]));
    }
    // A locked track is policy.
    let mut locked = p.clone();
    locked.timelines[0].tracks[1].lock = true;
    assert_eq!(
        code(try_apply(&locked, vec![remove_track("t2", false)])),
        "TRACK_LOCKED"
    );
    replay(&locked, vec![remove_track("t2", false)]).unwrap();
    assert_eq!(
        code(try_apply(&p, vec![remove_track("t9", false)])),
        "NOT_FOUND"
    );
}

#[test]
fn track_remove_with_items_inverse_restores_items_transitions_links() {
    let p = busy();
    let a = golden_apply(&p, vec![remove_track("t1", true)]);
    assert_eq!(tracks(&a.project, 0), [pair("t2", "A1")]);
    // The audio partner, left alone in its group, is unlinked (4C Tasks 9–10 review B2; with
    // two or more left, they keep their `link`).
    assert_eq!(a.project.timelines[0].tracks[0].items[0].common.link, None);
    let item = |id| change(EntityKind::Item, id, ChangeKind::Removed, Some("t1"));
    assert_eq!(
        a.diff.changes,
        [
            item("c1"),
            item("c2"),
            item("c3"),
            change(
                EntityKind::Transition,
                "tr1",
                ChangeKind::Removed,
                Some("t1")
            ),
            on_tl1(change(EntityKind::Track, "t1", ChangeKind::Removed, None)),
            change(EntityKind::Link, "lk1", ChangeKind::Removed, None),
            Change {
                path: Some("link".into()),
                ..change(EntityKind::Item, "c4", ChangeKind::Updated, Some("t2"))
            },
        ]
    );
    assert!(a.diff.shifts.is_empty());
    let f = fp(&remove_track("t1", true), &a);
    assert_eq!(f.removes, ["t1", "c1", "c2", "c3", "tr1", "lk1"]);
    // The undo restores the track into the group, then the partner joins it again.
    assert_eq!(a.inverse[1], link(&["c4"], "lk1"));
    // Decision 24: the restore lists every nested id, transitions and their items included.
    let restore = &a.inverse[0];
    ids_cover_every_id_in_json(restore);
    assert_eq!(
        restore.ids(),
        [
            "t1", "c1", "a1", "lk1", "c2", "a1", "c3", "a1", "tr1", "c1", "c2", "tl1"
        ]
    );
    // Undo and redo through history.
    let mut log = MemLog::new(p.clone());
    log.commit("main", "tx-1", "ann", vec![remove_track("t1", true)]);
    let removed = log.state("main");
    log.undo("main", "ann", "u-1").unwrap();
    assert_eq!(log.state("main"), p);
    log.redo("main", "ann", "r-1").unwrap();
    assert_eq!(log.state("main"), removed);
    log.undo("main", "ann", "u-2").unwrap();
    assert_eq!(log.state("main"), p);
}

#[test]
fn track_restore_checks_what_it_brings_back() {
    // c1's partner is unlinked, so the removal dissolves nothing and inverts to one restore.
    let mut p = busy();
    p.timelines[0].tracks[1].items[0].common.link = None;
    let a = apply(&p, vec![remove_track("t1", true)]);
    let restore = a.inverse[0].clone();
    // Its inverse removes the track with its items.
    let back = internal(&a.project, vec![restore.clone()]).unwrap();
    assert_eq!(back.project, p);
    assert_eq!(back.inverse, [remove_track("t1", true)]);
    // Clients cannot send it.
    assert_eq!(
        code(try_apply(&a.project, vec![restore.clone()])),
        "INVALID_OP"
    );
    // Twice is a duplicate.
    assert_eq!(
        code(internal(&a.project, vec![restore.clone(), restore.clone()])),
        "DUPLICATE_ID"
    );
    let edit = |f: &dyn Fn(&mut fin_core::TrackRestore)| -> Op {
        let mut op = restore.clone();
        if let Op::TrackRestore(r) = &mut op {
            f(r);
        }
        op
    };
    // Overlapping items fail in every mode (structural).
    let overlap = edit(&|r| r.track.items[1].start = secs(3));
    assert_eq!(code(replay(&a.project, vec![overlap])), "CLIP_OVERLAP");
    // A transition whose items do not touch.
    let apart = edit(&|r| r.track.transitions[0].between = ["c1".into(), "c3".into()]);
    assert_eq!(code(replay(&a.project, vec![apart])), "TRANSITION_INVALID");
    // Restores allocate nothing.
    let placeholder = edit(&|r| r.track.items[0].id = "$x".into());
    assert_eq!(code(internal(&a.project, vec![placeholder])), "INVALID_OP");
    let past = edit(&|r| r.index = 5);
    assert_eq!(code(internal(&a.project, vec![past])), "INVALID_RANGE");
    let missing = edit(&|r| {
        if let ItemBody::Clip { asset, .. } = &mut r.track.items[0].body {
            *asset = "a9".into();
        }
    });
    assert_eq!(code(replay(&a.project, vec![missing])), "NOT_FOUND");
    // -0.0 comes back as 0.0.
    let signed = edit(&|r| r.track.gain = -0.0);
    let got = internal(&a.project, vec![signed]).unwrap().project;
    assert_eq!(got.timelines[0].tracks[0].gain.to_bits(), 0f64.to_bits());
}

#[test]
fn track_move_reorders() {
    let p = project();
    let a = golden_apply(&p, vec![move_track("t1", 1)]);
    assert_eq!(tracks(&a.project, 0), [pair("t2", "A1"), pair("t1", "V1")]);
    assert_eq!(
        a.diff.changes,
        [on_tl1(param(EntityKind::Track, "t1", "index"))]
    );
    assert!(a.diff.shifts.is_empty());
    assert_eq!(a.inverse, [move_track("t1", 0)]);
    let f = fp(&move_track("t1", 1), &a);
    assert_eq!(
        f.touches,
        [("t1".to_string(), Touch::Param("index".into()))]
    );
    // In place.
    let same = golden_apply(&p, vec![move_track("t2", 1)]);
    assert_eq!(same.project, p);
    assert_eq!(
        code(try_apply(&p, vec![move_track("t1", 2)])),
        "INVALID_RANGE"
    );
}

#[test]
fn track_set_sets_each_setting() {
    let p = with_clips(&[4]);
    for (path, value, old) in [
        ("name", text("Main"), text("V1")),
        (
            "mute",
            Some(ParamValue::Bool(true)),
            Some(ParamValue::Bool(false)),
        ),
        (
            "solo",
            Some(ParamValue::Bool(true)),
            Some(ParamValue::Bool(false)),
        ),
        (
            "lock",
            Some(ParamValue::Bool(true)),
            Some(ParamValue::Bool(false)),
        ),
        (
            "gain",
            Some(ParamValue::Number(-6.0)),
            Some(ParamValue::Number(0.0)),
        ),
    ] {
        let a = golden_apply(&p, vec![set_track("t1", path, value)]);
        assert_eq!(a.diff.changes, [param(EntityKind::Track, "t1", path)]);
        assert_eq!(a.inverse, [set_track("t1", path, old)]);
        let f = fp(&set_track("t1", path, None), &a);
        assert_eq!(f.touches, [("t1".to_string(), Touch::Param(path.into()))]);
    }
    let t = |p: &Project| p.timelines[0].tracks[0].clone();
    // A reset goes back to the default; a name has none.
    let muted = apply(
        &p,
        vec![set_track("t1", "mute", Some(ParamValue::Bool(true)))],
    )
    .project;
    let a = golden_apply(&muted, vec![set_track("t1", "mute", None)]);
    assert!(!t(&a.project).mute);
    assert_eq!(
        a.inverse,
        [set_track("t1", "mute", Some(ParamValue::Bool(true)))]
    );
    assert_eq!(
        code(try_apply(&p, vec![set_track("t1", "name", None)])),
        "INVALID_OP"
    );
    // -0.0 is stored as 0.0.
    let z = apply(
        &p,
        vec![set_track("t1", "gain", Some(ParamValue::Number(-0.0)))],
    )
    .project;
    assert_eq!(t(&z).gain.to_bits(), 0f64.to_bits());
    // An unknown path and a wrong shape are structural; a range is policy.
    for op in [
        set_track("t1", "volume", Some(ParamValue::Number(1.0))),
        set_track("t1", "mute", Some(ParamValue::Number(1.0))),
    ] {
        let want = code(try_apply(&p, vec![op.clone()]));
        assert!(want == "PARAM_UNKNOWN" || want == "PARAM_TYPE", "{want}");
        assert_eq!(code(replay(&p, vec![op])), want);
    }
    let loud = set_track("t1", "gain", Some(ParamValue::Number(30.0)));
    let e = try_apply(&p, vec![loud.clone()]).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(e.details["path"], "gain");
    assert_eq!(e.details["max"], 24.0);
    assert_eq!(e.details["unit"], "dB");
    replay(&p, vec![loud]).unwrap();
    // A locked track's settings stay editable; its items do not.
    let locked = apply(
        &p,
        vec![set_track("t1", "lock", Some(ParamValue::Bool(true)))],
    )
    .project;
    apply(
        &locked,
        vec![set_track("t1", "mute", Some(ParamValue::Bool(true)))],
    );
    assert_eq!(code(try_apply(&locked, vec![delete("c1")])), "TRACK_LOCKED");
    apply(
        &locked,
        vec![set_track("t1", "lock", Some(ParamValue::Bool(false)))],
    );
}

#[test]
fn timeline_add_makes_an_empty_timeline() {
    let p = project();
    let a = golden_apply(&p, vec![add_timeline("$tl", 25)]);
    assert_eq!(a.bindings["$tl"], "tl2");
    let tl = &a.project.timelines[1];
    assert_eq!(
        (tl.id.as_str(), tl.name.as_str(), tl.rate),
        ("tl2", "B-roll", secs(25))
    );
    assert!(tl.tracks.is_empty() && tl.markers.is_empty());
    assert_eq!(
        a.diff.changes,
        [change(EntityKind::Timeline, "tl2", ChangeKind::Added, None)]
    );
    assert_eq!(a.inverse, [remove_timeline("tl2")]);
    let resolved = add_timeline("tl2", 25);
    assert_eq!(fp(&resolved, &a).creates, ["tl2"]);
    // A track on it in the same transaction.
    let b = golden_apply(
        &p,
        vec![
            add_timeline("$tl", 25),
            TrackAdd {
                id: "$v".into(),
                timeline: "$tl".into(),
                kind: TrackKind::Video,
                index: None,
                name: None,
            }
            .into(),
        ],
    );
    assert_eq!(tracks(&b.project, 1), [pair("t3", "V1")]);
    // A rate must be positive (structural); its range is policy.
    assert_eq!(
        code(replay(&p, vec![add_timeline("$tl", 0)])),
        "INVALID_RANGE"
    );
    assert_eq!(
        code(try_apply(&p, vec![add_timeline("$tl", 300)])),
        "INVALID_RANGE"
    );
    replay(&p, vec![add_timeline("$tl", 300)]).unwrap();
}

#[test]
fn timeline_set_sets_each_setting() {
    let p = two_timelines();
    let empty = apply(&p, vec![add_timeline("$tl", 30)]).project;
    for (path, value, old) in [
        ("name", text("Teaser"), text("B-roll")),
        (
            "resolution",
            Some(ParamValue::Vec2([1280.0, 720.0])),
            Some(ParamValue::Vec2([1920.0, 1080.0])),
        ),
        ("rate", text("30000/1001"), text("30")),
    ] {
        let a = golden_apply(&empty, vec![set_timeline("tl3", path, value)]);
        assert_eq!(a.diff.changes, [param(EntityKind::Timeline, "tl3", path)]);
        assert_eq!(a.inverse, [set_timeline("tl3", path, old)]);
        let f = fp(&set_timeline("tl3", path, None), &a);
        assert_eq!(f.touches, [("tl3".to_string(), Touch::Param(path.into()))]);
    }
    // The resolution resets to 1080×1920; the name and rate cannot be reset.
    let a = apply(&empty, vec![set_timeline("tl3", "resolution", None)]);
    assert_eq!(
        a.project.timelines[2].resolution,
        fin_core::DEFAULT_RESOLUTION
    );
    for path in ["name", "rate"] {
        assert_eq!(
            code(try_apply(&empty, vec![set_timeline("tl3", path, None)])),
            "INVALID_OP"
        );
    }
    // The rate changes only while the timeline holds no items (policy).
    let rate = set_timeline("tl2", "rate", text("30"));
    let e = try_apply(&p, vec![rate.clone()]).unwrap_err();
    assert_eq!(e.code, "TIMELINE_NOT_EMPTY");
    assert_eq!(e.details["items"], serde_json::json!(["c5"]));
    replay(&p, vec![rate]).unwrap();
    // Shapes are structural, ranges policy.
    let small = set_timeline("tl3", "resolution", Some(ParamValue::Vec2([8.0, 8.0])));
    assert_eq!(
        code(try_apply(&empty, vec![small.clone()])),
        "INVALID_RANGE"
    );
    replay(&empty, vec![small]).unwrap();
    for bad in [
        set_timeline("tl3", "resolution", Some(ParamValue::Vec2([1.5, 2.0]))),
        set_timeline("tl3", "rate", text("30/1")),
        set_timeline("tl3", "fps", text("30")),
    ] {
        let e = code(replay(&empty, vec![bad]));
        assert!(e == "PARAM_TYPE" || e == "PARAM_UNKNOWN", "{e}");
    }
}

#[test]
fn timeline_remove_inverse_restores_everything() {
    let p = two_timelines();
    let a = golden_apply(&p, vec![remove_timeline("tl2")]);
    assert_eq!(a.project.timelines.len(), 1);
    assert_eq!(
        a.diff.changes,
        [
            change(EntityKind::Item, "c5", ChangeKind::Removed, Some("t3")),
            change(EntityKind::Track, "t3", ChangeKind::Removed, None),
            change(EntityKind::Marker, "m1", ChangeKind::Removed, None),
            change(EntityKind::Timeline, "tl2", ChangeKind::Removed, None),
        ]
    );
    let f = fp(&remove_timeline("tl2"), &a);
    assert_eq!(f.removes, ["tl2", "c5", "t3", "m1"]);
    assert_eq!(f.touches, [("tl2".to_string(), Touch::Exists)]);
    let restore = &a.inverse[0];
    ids_cover_every_id_in_json(restore);
    assert_eq!(restore.ids(), ["tl2", "m1", "t3", "c5", "a1"]);
    // The restore's diff names it all, so replay checks its tracks.
    let back = internal(&a.project, a.inverse.clone()).unwrap();
    assert_eq!(back.project, p);
    assert_eq!(
        back.diff.changes,
        [
            change(EntityKind::Timeline, "tl2", ChangeKind::Added, None),
            change(EntityKind::Marker, "m1", ChangeKind::Added, None),
            change(EntityKind::Track, "t3", ChangeKind::Added, None),
            change(EntityKind::Item, "c5", ChangeKind::Added, Some("t3")),
        ]
    );
    assert_eq!(back.inverse, [remove_timeline("tl2")]);
    assert_eq!(
        code(internal(&p, a.inverse.clone())),
        "DUPLICATE_ID",
        "the timeline is still there"
    );
    // The main timeline can go while another stays.
    let b = golden_apply(&p, vec![remove_timeline("tl1")]);
    assert_eq!(b.project.timelines[0].id, "tl2");
}

#[test]
fn timeline_remove_refused_while_nested() {
    let mut p = apply(&project(), vec![add_timeline("$tl", 30)]).project;
    let nested = ItemBody::Nested {
        timeline: "tl2".into(),
        source_in: secs(0),
    };
    p.timelines[0].tracks[0]
        .items
        .push(Item::new("nst1", secs(0), secs(2), nested));
    for r in [
        try_apply(&p, vec![remove_timeline("tl2")]),
        replay(&p, vec![remove_timeline("tl2")]),
    ] {
        let e = r.unwrap_err();
        assert_eq!(e.code, "TIMELINE_NESTED");
        assert_eq!(e.details["items"], serde_json::json!(["nst1"]));
    }
    // The timeline holding the nested item can go (it takes the item along).
    apply(&p, vec![remove_timeline("tl1")]);
}

#[test]
fn last_timeline_is_kept() {
    let p = project();
    assert_eq!(
        code(try_apply(&p, vec![remove_timeline("tl1")])),
        "INVALID_OP"
    );
    assert_eq!(code(replay(&p, vec![remove_timeline("tl1")])), "INVALID_OP");
    let locked = {
        let mut q = two_timelines();
        q.timelines[1].tracks[0].lock = true;
        q
    };
    assert_eq!(
        code(try_apply(&locked, vec![remove_timeline("tl2")])),
        "TRACK_LOCKED"
    );
}

#[test]
fn stale_edit_on_removed_track_conflicts_removed() {
    let p = busy();
    let gone = vec![remove_track("t1", true)];
    for ours in [
        trim("c2", fin_core::Edge::End, secs(-1)),
        insert("$n", "t1", secs(12), "a1", secs(0), secs(1)),
        set_track("t1", "mute", Some(ParamValue::Bool(true))),
        move_track("t1", 1),
    ] {
        let e = rebase_over(&p, gone.clone(), vec![ours.clone()]).unwrap_err();
        assert_eq!(conflict_reason(&e), "REMOVED", "{}", ours.name());
        assert_eq!(e.details["againstTx"], "tx-a");
    }
    // Edits of a removed timeline's track.
    let p = two_timelines();
    let e = rebase_over(
        &p,
        vec![remove_timeline("tl2")],
        vec![insert("$n", "t3", secs(2), "a1", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "REMOVED");
    // Edits elsewhere merge.
    let r = rebase_over(&p, vec![remove_timeline("tl2")], vec![delete("c3")]).unwrap();
    assert_eq!(r.applied.project.timelines.len(), 1);
}

#[test]
fn concurrent_settings_conflict_by_path() {
    let p = project();
    let name = |n: &str| set_track("t1", "name", text(n));
    let e = rebase_over(&p, vec![name("Main")], vec![name("Cam")]).unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
    let r = rebase_over(
        &p,
        vec![name("Main")],
        vec![set_track("t1", "mute", Some(ParamValue::Bool(true)))],
    )
    .unwrap();
    let t = &r.applied.project.timelines[0].tracks[0];
    assert_eq!((t.name.as_str(), t.mute), ("Main", true));
    let e = rebase_over(&p, vec![move_track("t1", 1)], vec![move_track("t1", 1)]).unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
    // A stale removal of a track that was filled meanwhile fails at the head.
    let e = rebase_over(
        &with_clips(&[4]),
        vec![insert("$n", "t2", secs(0), "a1", secs(0), secs(1))],
        vec![remove_track("t2", false)],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "TRACK_NOT_EMPTY");
}

#[test]
fn undo_of_a_track_add_after_an_insert_fails() {
    let p = with_clips(&[4]);
    let added = apply(&p, vec![add_track("tl1", TrackKind::Audio, None, None)]);
    let filled = apply(
        &added.project,
        vec![insert("$n", "t3", secs(0), "a1", secs(0), secs(1))],
    );
    assert_eq!(
        code(internal(&filled.project, added.inverse.clone())),
        "TRACK_NOT_EMPTY"
    );
}

#[test]
fn ops_carry_their_ids() {
    let ops = [
        add_track("tl1", TrackKind::Video, Some(0), Some("Main")),
        remove_track("t1", true),
        move_track("t1", 0),
        set_track("t1", "name", text("Main")),
        add_timeline("$tl", 30),
        set_timeline("tl1", "rate", text("25")),
        remove_timeline("tl2"),
    ];
    for op in &ops {
        ids_cover_every_id_in_json(op);
    }
    assert_eq!(ops[0].allocates(), Some("$t"));
    assert_eq!(ops[4].allocates(), Some("$tl"));
    assert_eq!(ops[0].ids(), ["$t", "tl1"]);
    let r: Rational = "1/2".parse().unwrap();
    assert!(r.is_positive());
}
