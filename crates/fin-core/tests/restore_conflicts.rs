//! Ops that put an entity back whole (spec §4.3, *as built*: `Footprint::overwrites`) meet a
//! later change of it, or of an entity nested in it, as a conflict instead of writing their
//! snapshot over it. A fixed-case table over what the history properties do not generate:
//! effect edits under an undo of a move, and a merged removal-and-undo of each kind of
//! entity (effect, marker, track, timeline, asset, bin, style) over the target's edit of it.

mod common;

use std::collections::BTreeMap;

use common::memlog::MemLog;
use common::moves::move_to;
use common::*;
use fin_core::{
    AssetMove, BinAdd, BinMove, BinRemove, CoreError, Edge, EffectAdd, EffectRemove, FontFormat,
    FontPin, FontPinOp, FontRepin, FontSource, FontUnpin, InsertMode, MarkerAdd, MarkerRemove,
    MarkerSet, Op, ParamSet, ParamValue, Rational, Resolution, StyleAdd, StyleRemove, StyleSet,
    TimelineAdd, TimelineRemove, TimelineSet, TrackAdd, TrackKind, TrackRemove,
};

type Ids = BTreeMap<String, String>;
/// A named later edit, built from the effect's id.
type Later = (&'static str, fn(&str) -> Op);

fn frames(n: i64) -> Rational {
    Rational::new(n, 30).unwrap()
}

fn text(s: &str) -> Option<ParamValue> {
    Some(ParamValue::Text(s.into()))
}

fn add_fx(item: &str, id: &str) -> Op {
    EffectAdd {
        item: item.into(),
        id: id.into(),
        kind: "blur".into(),
        at: None,
        params: BTreeMap::new(),
    }
    .into()
}

fn fx_off(item: &str, fx: &str) -> Op {
    ParamSet {
        item: item.into(),
        path: format!("effects.{fx}.enabled"),
        value: Some(ParamValue::Bool(false)),
    }
    .into()
}

fn remove_fx(item: &str, fx: &str) -> Op {
    EffectRemove {
        item: item.into(),
        effect: fx.into(),
    }
    .into()
}

const INTER: &str = "Inter|700|normal";

/// Inter 700 pinned to a file whose SHA-256 is `c` repeated.
fn inter(c: char) -> FontPin {
    FontPin {
        family: "Inter".into(),
        weight: 700,
        italic: false,
        face: 0,
        sha256: std::iter::repeat_n(c, 64).collect(),
        size: 1000,
        format: FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: FontSource::Catalog {
            entry: "inter".into(),
            style: "bold".into(),
            catalog: "2026.11.1".into(),
        },
    }
}

fn remove_t2() -> Op {
    TrackRemove {
        track: "t2".into(),
        with_items: true,
    }
    .into()
}

/// A timeline `$tl` (bound to `tl2`) to remove.
fn timeline() -> Op {
    TimelineAdd {
        id: "$tl".into(),
        name: "B-roll".into(),
        rate: secs(30),
        resolution: Resolution {
            width: 1920,
            height: 1080,
        },
    }
    .into()
}

fn remove_timeline(b: &Ids) -> Op {
    TimelineRemove {
        timeline: b["$tl"].clone(),
    }
    .into()
}

/// The conflict's reason and blamed record.
fn blame(e: &CoreError) -> (String, String) {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    let s = |k: &str| e.details[k].as_str().unwrap_or_default().to_string();
    (s("reason"), s("againstTx"))
}

/// Alice adds an effect to c1 and moves c1 to the end of t1; bob then edits c1's effects. The undo's
/// `item.restore` would put c1 back with its effects as they were, dropping bob's edit:
/// `SAME_PARAM`, blamed on bob, and nothing is written.
#[test]
fn undo_of_move_meets_later_effect_edits() {
    let cases: [Later; 4] = [
        ("param.set of the effect", |fx| fx_off("c1", fx)),
        ("effect.remove", |fx| remove_fx("c1", fx)),
        ("effect.add", |_| add_fx("c1", "$g")),
        ("keyframe of an effect param", |fx| {
            fin_core::KeyframeSet {
                item: "c1".into(),
                path: format!("effects.{fx}.radius"),
                at: Rational::zero(),
                value: ParamValue::Number(3.0),
                interp: fin_core::Interp::Linear,
            }
            .into()
        }),
    ];
    let mut failed = Vec::new();
    for (what, later) in cases {
        let mut log = MemLog::new(with_clips(&[2, 2, 2]));
        let fx = log.commit("main", "e1", "alice", vec![add_fx("c1", "$fx")]);
        let fx = fx.record.bindings["$fx"].clone();
        // To the end of its own (video) track: the inverse is still a remove and a restore.
        let to_end = move_to("c1", "t1", secs(6), InsertMode::Ripple);
        log.commit("main", "m1", "alice", vec![to_end]);
        log.commit("main", "b1", "bob", vec![later(&fx)]);
        let before = log.state("main");
        let got = log.undo("main", "alice", "u1").err().map(|e| blame(&e));
        if got != Some(("SAME_PARAM".into(), "b1".into())) || log.state("main") != before {
            failed.push(format!("{what}: {got:?}"));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// An effect edit of another item is not in the way: the undo of the move applies.
#[test]
fn undo_of_move_after_another_items_effect_edit_applies() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let to_end = move_to("c1", "t1", secs(6), InsertMode::Ripple);
    log.commit("main", "m1", "alice", vec![to_end]);
    log.commit("main", "b1", "bob", vec![add_fx("c2", "$g")]);
    log.undo("main", "alice", "u1").unwrap();
    let p = log.state("main");
    let t1 = &p.timelines[0].tracks[0].items;
    let order: Vec<&str> = t1.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(order, ["c1", "c2", "c3"]);
    assert_eq!(t1[1].common.effects.len(), 1);
}

/// One merge case: `setup` on main before br1 forks; on br1 bob runs `remove` and undoes it;
/// on main alice runs `edit`. Each op is built from the setup's bindings.
struct Case {
    what: &'static str,
    setup: Vec<Op>,
    remove: fn(&Ids) -> Op,
    edit: fn(&Ids) -> Op,
    reason: &'static str,
}

fn cases() -> Vec<Case> {
    let bin = |id: &str| -> Op {
        BinAdd {
            id: id.into(),
            name: id.trim_start_matches('$').into(),
            parent: None,
        }
        .into()
    };
    vec![
        Case {
            what: "effect (effect.restore)",
            setup: vec![add_fx("c1", "$fx")],
            remove: |b| remove_fx("c1", &b["$fx"]),
            edit: |b| fx_off("c1", &b["$fx"]),
            reason: "SAME_PARAM",
        },
        Case {
            what: "marker (marker.add)",
            setup: vec![
                MarkerAdd {
                    id: "$m".into(),
                    timeline: "tl1".into(),
                    at: secs(1),
                    duration: None,
                    label: "Intro".into(),
                    color: None,
                }
                .into(),
            ],
            remove: |b| {
                MarkerRemove {
                    marker: b["$m"].clone(),
                }
                .into()
            },
            edit: |b| {
                MarkerSet {
                    marker: b["$m"].clone(),
                    path: "label".into(),
                    value: text("Outro"),
                }
                .into()
            },
            reason: "SAME_PARAM",
        },
        Case {
            what: "track with items (track.restore)",
            setup: vec![insert("$y", "t2", secs(0), "a1", secs(0), secs(2))],
            remove: |_| remove_t2(),
            edit: |b| trim(&b["$y"], Edge::End, frames(-9)),
            reason: "SAME_EDGE",
        },
        Case {
            what: "timeline (timeline.restore)",
            setup: vec![timeline()],
            remove: remove_timeline,
            edit: |b| {
                TimelineSet {
                    timeline: b["$tl"].clone(),
                    path: "name".into(),
                    value: text("Teaser"),
                }
                .into()
            },
            reason: "SAME_PARAM",
        },
        Case {
            what: "asset (asset.add)",
            setup: vec![av_asset("$a2", secs(10)), bin("$b")],
            remove: |b| Op::AssetRemove {
                id: b["$a2"].clone(),
            },
            edit: |b| {
                AssetMove {
                    asset: b["$a2"].clone(),
                    bin: Some(b["$b"].clone()),
                }
                .into()
            },
            reason: "SAME_PARAM",
        },
        Case {
            what: "bin (bin.add)",
            setup: vec![bin("$b"), bin("$c")],
            remove: |b| {
                BinRemove {
                    bin: b["$b"].clone(),
                }
                .into()
            },
            edit: |b| {
                BinMove {
                    bin: b["$b"].clone(),
                    parent: Some(b["$c"].clone()),
                }
                .into()
            },
            reason: "SAME_PARAM",
        },
        Case {
            what: "style (style.add)",
            setup: vec![
                StyleAdd {
                    id: "$s".into(),
                    name: "Title".into(),
                    from: None,
                    set: BTreeMap::new(),
                }
                .into(),
            ],
            remove: |b| {
                StyleRemove {
                    style: b["$s"].clone(),
                }
                .into()
            },
            edit: |b| {
                StyleSet {
                    style: b["$s"].clone(),
                    path: "stroke.width".into(),
                    value: Some(ParamValue::Number(4.0)),
                }
                .into()
            },
            reason: "SAME_PARAM",
        },
        Case {
            what: "insert onto the track (track.restore)",
            setup: vec![insert("$y", "t2", secs(0), "a1", secs(0), secs(2))],
            remove: |_| remove_t2(),
            edit: |_| insert("$w", "t2", secs(2), "a1", secs(0), secs(1)),
            reason: "SAME_PARAM",
        },
        Case {
            what: "move onto the track (track.restore)",
            setup: vec![insert("$y", "t2", secs(0), "a1", secs(0), secs(2))],
            remove: |_| remove_t2(),
            edit: |_| move_to("c1", "t2", secs(2), InsertMode::Ripple),
            reason: "SAME_PARAM",
        },
        Case {
            what: "marker on the timeline (timeline.restore)",
            setup: vec![timeline()],
            remove: remove_timeline,
            edit: |b| {
                MarkerAdd {
                    id: "$m".into(),
                    timeline: b["$tl"].clone(),
                    at: secs(1),
                    duration: None,
                    label: "Intro".into(),
                    color: None,
                }
                .into()
            },
            reason: "SAME_PARAM",
        },
        Case {
            what: "track.add on the timeline (timeline.restore)",
            setup: vec![timeline()],
            remove: remove_timeline,
            edit: |b| {
                TrackAdd {
                    id: "$t".into(),
                    timeline: b["$tl"].clone(),
                    kind: TrackKind::Video,
                    index: None,
                    name: None,
                }
                .into()
            },
            reason: "SAME_PARAM",
        },
        Case {
            what: "font pin (font.unpin, then font.pin)",
            setup: vec![
                FontPinOp {
                    key: INTER.into(),
                    pin: inter('a'),
                    fallback: None,
                }
                .into(),
            ],
            remove: |_| FontUnpin::new(INTER).into(),
            edit: |_| {
                FontRepin {
                    key: INTER.into(),
                    pin: inter('b'),
                }
                .into()
            },
            reason: "SAME_PARAM",
        },
    ]
}

/// Each kind of entity: the copy of bob's undo would put the entity back as bob removed it,
/// over alice's edit on main. The merge is a conflict of that reason, blamed on her edit, and
/// writes nothing; without her edit it merges (the removal and its undo cancel). Every case
/// runs, and the failures are reported together.
#[test]
fn merged_removal_and_undo_meets_the_targets_edit() {
    let mut failed = Vec::new();
    for edited in [true, false] {
        for c in cases() {
            let mut log = MemLog::new(with_clips(&[2, 2, 2]));
            let ids = log.commit("main", "s1", "alice", c.setup).record.bindings;
            log.fork("br1", "main", log.head_seq("main"));
            if edited {
                log.commit("main", "a1", "alice", vec![(c.edit)(&ids)]);
            }
            log.commit("br1", "r1", "bob", vec![(c.remove)(&ids)]);
            log.undo("br1", "bob", "u1").unwrap();
            let before = log.state("main");
            let merged = log.merge("br1", "main", "carol", "g1", false);
            let want = edited.then(|| (c.reason.to_string(), "a1".to_string()));
            let got = merged.as_ref().err().map(blame);
            let unchanged = !edited || log.state("main") == before;
            if got != want || !unchanged || !edited && merged.is_err() {
                failed.push(format!("{} (edited {edited}): {got:?}", c.what));
            }
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// The same through a cherry-pick and a revert. Pick: bob's track removal comes over first
/// (taking alice's new item with the track: a removal of an edited entity is no conflict);
/// the pick of his undo would bring back the track without it, so it is `SAME_PARAM`. Revert:
/// carol merges bob's removal and undo (no edit yet), alice then inserts on t2; reverting the
/// group would remove t2 and put back the snapshot without her item.
#[test]
fn track_put_back_by_pick_or_revert_meets_an_insert_onto_it() {
    let setup = || {
        let mut log = MemLog::new(with_clips(&[2, 2, 2]));
        let y = insert("$y", "t2", secs(0), "a1", secs(0), secs(2));
        log.commit("main", "s1", "alice", vec![y]);
        log.fork("br1", "main", log.head_seq("main"));
        log.commit("br1", "r1", "bob", vec![remove_t2()]);
        log.undo("br1", "bob", "u1").unwrap();
        log
    };
    let onto_t2 = || insert("$w", "t2", secs(2), "a1", secs(0), secs(1));

    let mut log = setup();
    log.commit("main", "a1", "alice", vec![onto_t2()]);
    log.pick("r1", "main", "carol", "p1")
        .unwrap()
        .expect("a copy");
    let before = log.state("main");
    let e = log
        .pick("u1", "main", "carol", "p2")
        .expect_err("a conflict");
    assert_eq!(blame(&e), ("SAME_PARAM".into(), "a1".into()));
    assert_eq!(log.state("main"), before);

    let mut log = setup();
    log.merge("br1", "main", "carol", "g1", false).unwrap();
    log.commit("main", "a1", "alice", vec![onto_t2()]);
    let before = log.state("main");
    let e = log
        .revert("g1", "main", "carol", "v1")
        .expect_err("a conflict");
    assert_eq!(blame(&e), ("SAME_PARAM".into(), "a1".into()));
    assert_eq!(log.state("main"), before);
}
