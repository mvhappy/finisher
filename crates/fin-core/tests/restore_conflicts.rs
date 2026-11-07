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

const LORA: &str = "Lora|400|normal";
const ROBOTO: &str = "Roboto|400|normal";

/// Inter 700 pinned to a file whose SHA-256 is `c` repeated.
fn inter(c: char) -> FontPin {
    face("Inter", 700, c)
}

/// `family` at `weight`, pinned to a file whose SHA-256 is `c` repeated.
fn face(family: &str, weight: u16, c: char) -> FontPin {
    FontPin {
        family: family.into(),
        weight,
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

/// A pin of `key` (a 400 face of `family`), placed at `fallback` in the fallback order.
fn pin_at(key: &str, family: &str, c: char, fallback: Option<u32>) -> Op {
    FontPinOp {
        key: key.into(),
        pin: face(family, 400, c),
        fallback,
    }
    .into()
}

fn add_track(id: &str, kind: TrackKind, index: Option<u32>) -> Op {
    TrackAdd {
        id: id.into(),
        timeline: "tl1".into(),
        kind,
        index,
        name: None,
    }
    .into()
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
        // Ordered lists (`ops::util::lists`): a put-back at its old index over another
        // author's placement in the same list.
        Case {
            what: "track order (track.restore at its old index)",
            setup: vec![add_track("$t3", TrackKind::Video, None)],
            remove: |_| {
                TrackRemove {
                    track: "t2".into(),
                    with_items: false,
                }
                .into()
            },
            edit: |_| add_track("$t4", TrackKind::Video, Some(1)),
            reason: "SAME_PARAM",
        },
        Case {
            what: "timeline order (timeline.restore at its old index)",
            setup: vec![timeline()],
            remove: remove_timeline,
            edit: |_| {
                TimelineAdd {
                    id: "$tl4".into(),
                    name: "Teaser".into(),
                    rate: secs(30),
                    resolution: Resolution {
                        width: 1080,
                        height: 1920,
                    },
                }
                .into()
            },
            reason: "SAME_PARAM",
        },
        Case {
            what: "fallback order (font.pin at its old index)",
            setup: vec![pin_at(LORA, "Lora", 'a', Some(0))],
            remove: |_| FontUnpin::new(LORA).into(),
            edit: |_| pin_at(ROBOTO, "Roboto", 'b', Some(0)),
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

/// The fallback order through a cherry-pick and a revert: main has `[Lora]`, alice pins Roboto
/// at 0 (`[Roboto, Lora]`); bob's unpin of Lora and its undo would put Lora back at 0, ahead of
/// her placement. The pick of the undo, or the revert of a merged unpin-and-undo after her
/// placement, is `SAME_PARAM` blamed on her pin; nothing is written.
#[test]
fn fallback_put_back_by_pick_or_revert_meets_a_placement() {
    let setup = || {
        let mut log = MemLog::new(with_clips(&[2, 2, 2]));
        log.commit(
            "main",
            "s1",
            "alice",
            vec![pin_at(LORA, "Lora", 'a', Some(0))],
        );
        log.fork("br1", "main", log.head_seq("main"));
        log.commit("br1", "r1", "bob", vec![FontUnpin::new(LORA).into()]);
        log.undo("br1", "bob", "u1").unwrap();
        log
    };
    let roboto = || pin_at(ROBOTO, "Roboto", 'b', Some(0));

    let mut log = setup();
    log.commit("main", "a1", "alice", vec![roboto()]);
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
    log.commit("main", "a1", "alice", vec![roboto()]);
    assert_eq!(log.state("main").fonts.fallback, [ROBOTO, LORA]);
    let before = log.state("main");
    let e = log
        .revert("g1", "main", "carol", "v1")
        .expect_err("a conflict");
    assert_eq!(blame(&e), ("SAME_PARAM".into(), "a1".into()));
    assert_eq!(log.state("main"), before);
}

/// The track order on one branch. Undo: alice removes t2 from `[t1, t2, t3]`, bob adds t4 at
/// index 1, and the undo would put t2 back at 1, ahead of t4. Redo: alice adds t3 at 1 and
/// undoes it, bob adds t4 at 1, and the redo would put t3 back at 1. Both are `SAME_PARAM`
/// blamed on bob, and nothing is written.
#[test]
fn track_put_back_by_undo_or_redo_meets_a_placement() {
    let base = || {
        let mut log = MemLog::new(with_clips(&[2, 2, 2]));
        log.commit(
            "main",
            "s1",
            "setup",
            vec![add_track("$t3", TrackKind::Video, None)],
        );
        log
    };
    let t4 = || add_track("$t4", TrackKind::Video, Some(1));

    let mut log = base();
    log.commit("main", "x1", "alice", vec![remove_t2()]);
    log.commit("main", "b1", "bob", vec![t4()]);
    let before = log.state("main");
    let e = log.undo("main", "alice", "u1").expect_err("a conflict");
    assert_eq!(blame(&e), ("SAME_PARAM".into(), "b1".into()));
    assert_eq!(log.state("main"), before);

    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    log.commit(
        "main",
        "x1",
        "alice",
        vec![add_track("$t3", TrackKind::Video, Some(1))],
    );
    log.undo("main", "alice", "u1").unwrap();
    log.commit("main", "b1", "bob", vec![t4()]);
    let before = log.state("main");
    let e = log.redo("main", "alice", "r1").expect_err("a conflict");
    assert_eq!(blame(&e), ("SAME_PARAM".into(), "b1".into()));
    assert_eq!(log.state("main"), before);
}

/// An undo of a split takes the right part back into the left part (a lift removal, then a
/// strict trim over its range), so a later edit of the right part by someone else is a
/// conflict: `SAME_PARAM`, or `SAME_EDGE` for its end, blamed on bob. Captions likewise. An
/// edit of the left part is not in the way.
#[test]
fn undo_of_split_meets_a_later_edit_of_the_right_part() {
    use common::gen4c::caption;
    use fin_core::{CaptionSetStyle, CaptionSetText, CaptionSplit, ItemSplit};
    type Edit = (&'static str, fn(&str) -> Op, &'static str);
    let opacity = |item: &str| -> Op {
        ParamSet {
            item: item.into(),
            path: "opacity".into(),
            value: Some(ParamValue::Number(0.5)),
        }
        .into()
    };
    let clips: [Edit; 3] = [
        ("opacity", opacity, "SAME_PARAM"),
        ("effect.add", |r| add_fx(r, "$g"), "SAME_PARAM"),
        ("end trim", |r| trim(r, Edge::End, frames(-9)), "SAME_EDGE"),
    ];
    let captions: [Edit; 2] = [
        (
            "caption.setText",
            |r| {
                CaptionSetText {
                    item: r.into(),
                    text: "new words".into(),
                }
                .into()
            },
            "SAME_PARAM",
        ),
        (
            "caption.setStyle",
            |r| {
                CaptionSetStyle {
                    item: r.into(),
                    style: "preset:punch".into(),
                    overrides: None,
                }
                .into()
            },
            "SAME_PARAM",
        ),
    ];
    let mut failed = Vec::new();
    for (caption_case, cases) in [(false, &clips[..]), (true, &captions[..])] {
        for (what, later, reason) in cases {
            let mut log = MemLog::new(with_clips(&[2, 2, 2]));
            let split: Op = if caption_case {
                let tc = add_track("$tc", TrackKind::Caption, None);
                let cap = caption(
                    "$cap",
                    "$tc",
                    secs(0),
                    &["one", "two", "three"],
                    "preset:bold",
                );
                let ids = log
                    .commit("main", "s0", "setup", vec![tc, cap])
                    .record
                    .bindings;
                CaptionSplit {
                    item: ids["$cap"].clone(),
                    word: 1,
                    id: "$r".into(),
                }
                .into()
            } else {
                ItemSplit {
                    item: "c1".into(),
                    offset: secs(1),
                    id: "$r".into(),
                }
                .into()
            };
            let right = log
                .commit("main", "s1", "alice", vec![split])
                .record
                .bindings["$r"]
                .clone();
            log.commit("main", "b1", "bob", vec![later(&right)]);
            let before = log.state("main");
            let got = log.undo("main", "alice", "u1").err().map(|e| blame(&e));
            if got != Some(((*reason).into(), "b1".into())) || log.state("main") != before {
                failed.push(format!("{what}: {got:?}"));
            }
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");

    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let split = ItemSplit {
        item: "c1".into(),
        offset: secs(1),
        id: "$r".into(),
    };
    log.commit("main", "s1", "alice", vec![split.into()]);
    log.commit("main", "b1", "bob", vec![opacity("c1")]);
    log.undo("main", "alice", "u1").unwrap();
    let c1 = &log.state("main").timelines[0].tracks[0].items[0];
    assert_eq!((c1.id.as_str(), c1.duration), ("c1", secs(2)));
    assert_eq!(c1.common.opacity, 0.5);
}

/// A re-run that fails is blamed on the step that changed the entity (spec §4.3): alice
/// unpins Inter, bob pins another file for it, and alice's undo is `FONT_PINNED` against bob.
#[test]
fn undo_of_unpin_after_another_pin_blames_it() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let pin = |c: char| -> Op {
        FontPinOp {
            key: INTER.into(),
            pin: inter(c),
            fallback: None,
        }
        .into()
    };
    log.commit("main", "s1", "setup", vec![pin('a')]);
    log.commit("main", "x1", "alice", vec![FontUnpin::new(INTER).into()]);
    log.commit("main", "b1", "bob", vec![pin('b')]);
    let e = log.undo("main", "alice", "u1").expect_err("a conflict");
    assert_eq!(blame(&e), ("FONT_PINNED".into(), "b1".into()), "{e:?}");
}

// Must merge: what the put-back rules leave alone (spec §4.3).

fn move_track(track: &str, index: u32) -> Op {
    fin_core::TrackMove {
        track: track.into(),
        index,
    }
    .into()
}

/// A second timeline `tl2` with tracks `ta`, `tb` (bound by the returned ids).
fn with_tl2(log: &mut MemLog) -> Ids {
    let tl2 = TrackAdd {
        id: "$ta".into(),
        timeline: "$tl".into(),
        kind: TrackKind::Video,
        index: None,
        name: None,
    };
    let tb = TrackAdd {
        id: "$tb".into(),
        ..tl2.clone()
    };
    let ops = vec![timeline(), tl2.into(), tb.into()];
    log.commit("main", "s0", "setup", ops).record.bindings
}

/// An op that writes nothing loses nothing: two identical pins placing one key (spec §4.3:
/// the same pin on both sides merges). Bob's stale pin re-runs as a no-op.
#[test]
fn identical_placing_pins_merge() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    log.commit(
        "main",
        "a1",
        "alice",
        vec![pin_at(LORA, "Lora", 'a', Some(0))],
    );
    let pin = pin_at(LORA, "Lora", 'a', Some(0));
    log.commit_at("main", "b1", "bob", 0, vec![pin]).unwrap();
    assert_eq!(log.state("main").fonts.fallback, [LORA]);
}

/// The undo of an overwrite insert has the shape of a split's undo (a lift removal, then a
/// strict trim growing the cut item back), but it removes alice's own new item: bob's edit
/// of it goes with it, as any removal of an edited entity (spec §4.3).
#[test]
fn undo_of_overwrite_insert_after_an_edit_of_it_applies() {
    let mut log = MemLog::new(with_clips(&[4]));
    let n = fin_core::ItemInsert {
        id: "$n".into(),
        track: "t1".into(),
        at: secs(2),
        duration: secs(2),
        mode: InsertMode::Overwrite,
        body: fin_core::ItemBody::Clip {
            asset: "a1".into(),
            source_in: secs(10),
            speed: secs(1),
        },
    };
    let n = log
        .commit("main", "a1", "alice", vec![n.into()])
        .record
        .bindings["$n"]
        .clone();
    let opacity = ParamSet {
        item: n,
        path: "opacity".into(),
        value: Some(ParamValue::Number(0.5)),
    };
    log.commit("main", "b1", "bob", vec![opacity.into()]);
    log.undo("main", "alice", "u1").unwrap();
    let items = &log.state("main").timelines[0].tracks[0].items;
    assert_eq!(items.len(), 1);
    assert_eq!((items[0].id.as_str(), items[0].duration), ("c1", secs(4)));
}

/// Track orders are per timeline (`<timeline>#tracks`): edits of disjoint timelines never
/// meet. A move on tl1 and a stale move on tl2; an append on tl2 and a stale move on tl1;
/// and an undo of removing t2 on tl1 after a track append on tl2: all apply.
#[test]
fn track_orders_of_different_timelines_merge() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let b = with_tl2(&mut log);
    let base = log.head_seq("main");
    log.commit("main", "a1", "alice", vec![move_track("t1", 1)]);
    log.commit_at("main", "b1", "bob", base, vec![move_track(&b["$ta"], 1)])
        .unwrap();
    let p = log.state("main");
    let ids = |tl: usize| -> Vec<String> {
        p.timelines[tl]
            .tracks
            .iter()
            .map(|t| t.id.clone())
            .collect()
    };
    assert_eq!(ids(0), ["t2", "t1"]);
    assert_eq!(ids(1), [b["$tb"].clone(), b["$ta"].clone()]);

    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let b = with_tl2(&mut log);
    let base = log.head_seq("main");
    let append = TrackAdd {
        id: "$tc".into(),
        timeline: b["$tl"].clone(),
        kind: TrackKind::Video,
        index: None,
        name: None,
    };
    log.commit("main", "a1", "alice", vec![append.clone().into()]);
    log.commit_at("main", "b1", "bob", base, vec![move_track("t1", 1)])
        .unwrap();
    assert_eq!(log.state("main").timelines[0].tracks[1].id, "t1");

    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let b = with_tl2(&mut log);
    log.commit("main", "x1", "alice", vec![remove_t2()]);
    let append = TrackAdd {
        timeline: b["$tl"].clone(),
        ..append
    };
    log.commit("main", "b1", "bob", vec![append.into()]);
    log.undo("main", "alice", "u1").unwrap();
    let p = log.state("main");
    let ids: Vec<&str> = p.timelines[0]
        .tracks
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(ids, ["t1", "t2"]);
    assert_eq!(p.timelines[1].tracks.len(), 3);
}

/// `c1`'s effect ids, in order.
fn fx_ids(log: &MemLog) -> Vec<String> {
    let p = log.state("main");
    let c1 = p.timelines[0].tracks[0].items.iter().find(|i| i.id == "c1");
    c1.unwrap()
        .common
        .effects
        .iter()
        .map(|e| e.id.clone())
        .collect()
}

/// A log whose `c1` has effects `fa`, `fb`, `fc` (bound by the returned ids).
fn three_fx() -> (MemLog, Ids) {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let ops = vec![
        add_fx("c1", "$fa"),
        add_fx("c1", "$fb"),
        add_fx("c1", "$fc"),
    ];
    let b = log.commit("main", "s0", "setup", ops).record.bindings;
    (log, b)
}

/// Indexes are carried by their neighbours (`rebase::places`), so appends and removals by
/// others elsewhere in the list never stop an index op, and the member goes back between the
/// neighbours it had (spec §4.3). Each case was refused at d50a083; the old absolute index got
/// the last two wrong.
#[test]
fn index_ops_merge_with_appends_and_removals_elsewhere() {
    let ids =
        |b: &Ids, names: &[&str]| -> Vec<String> { names.iter().map(|n| b[*n].clone()).collect() };
    // Your removal of fb, bob removes the later fc, you undo: [fa, fb].
    let (mut log, b) = three_fx();
    log.commit("main", "x1", "alice", vec![remove_fx("c1", &b["$fb"])]);
    log.commit("main", "b1", "bob", vec![remove_fx("c1", &b["$fc"])]);
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(fx_ids(&log), ids(&b, &["$fa", "$fb"]));
    // Your removal of fb, bob appends fd, you undo: [fa, fb, fc, fd].
    let (mut log, b) = three_fx();
    log.commit("main", "x1", "alice", vec![remove_fx("c1", &b["$fb"])]);
    let fd = log.commit("main", "b1", "bob", vec![add_fx("c1", "$fd")]);
    let fd = fd.record.bindings["$fd"].clone();
    log.undo("main", "alice", "u1").unwrap();
    let mut want = ids(&b, &["$fa", "$fb", "$fc"]);
    want.push(fd);
    assert_eq!(fx_ids(&log), want);
    // Bob appends, then your stale effect.add at index 0: first.
    let (mut log, b) = three_fx();
    let base = log.head_seq("main");
    log.commit("main", "b1", "bob", vec![add_fx("c1", "$fd")]);
    let first = EffectAdd {
        item: "c1".into(),
        id: "$fe".into(),
        kind: "blur".into(),
        at: Some(0),
        params: BTreeMap::new(),
    };
    let r = log
        .commit_at("main", "x1", "alice", base, vec![first.into()])
        .unwrap();
    assert_eq!(fx_ids(&log)[0], r.record.bindings["$fe"]);
    assert_eq!(fx_ids(&log)[1..4], ids(&b, &["$fa", "$fb", "$fc"]));
    // Your track move, bob appends a track on the same timeline, you undo.
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    log.commit("main", "x1", "alice", vec![move_track("t1", 1)]);
    log.commit(
        "main",
        "b1",
        "bob",
        vec![add_track("$t3", TrackKind::Video, None)],
    );
    log.undo("main", "alice", "u1").unwrap();
    let p = log.state("main");
    let order: Vec<&str> = p.timelines[0]
        .tracks
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(order, ["t1", "t2", "t3"]);
    // Bob removes the earlier fa, you undo your removal of fb: [fb, fc] (the old index put
    // fb after fc).
    let (mut log, b) = three_fx();
    log.commit("main", "x1", "alice", vec![remove_fx("c1", &b["$fb"])]);
    log.commit("main", "b1", "bob", vec![remove_fx("c1", &b["$fa"])]);
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(fx_ids(&log), ids(&b, &["$fb", "$fc"]));
    // The same through a merge: alice removes fa on main, bob removes fb on br1 and undoes.
    let (mut log, b) = three_fx();
    log.fork("br1", "main", log.head_seq("main"));
    log.commit("main", "a1", "alice", vec![remove_fx("c1", &b["$fa"])]);
    log.commit("br1", "r1", "bob", vec![remove_fx("c1", &b["$fb"])]);
    log.undo("br1", "bob", "u1").unwrap();
    log.merge("br1", "main", "carol", "g1", false).unwrap();
    assert_eq!(fx_ids(&log), ids(&b, &["$fb", "$fc"]));
}

/// `tl1`'s track ids, with alice's bound ids named by their placeholders.
fn track_names(log: &MemLog, bound: &Ids) -> Vec<String> {
    let p = log.state("main");
    p.timelines[0]
        .tracks
        .iter()
        .map(|t| {
            bound
                .iter()
                .find(|(_, id)| **id == t.id)
                .map_or_else(|| t.id.clone(), |(n, _)| n.clone())
        })
        .collect()
}

/// A neighbour that an earlier op of the same transaction made is named by its placeholder,
/// never by the id the planning frame gave it (which can name another author's track at the
/// head). tl1 `[t1, t2, t3]`; bob appends t4 and removes t2; alice's stale `[add $x at 1,
/// add $y at 2]` gives `[t1, $x, $y, t3, t4]`, stale or through a merge.
#[test]
fn placeholder_neighbours_are_carried_by_their_placeholders() {
    for merge in [false, true] {
        let mut log = MemLog::new(with_clips(&[2, 2, 2]));
        log.commit(
            "main",
            "s0",
            "setup",
            vec![add_track("$t3", TrackKind::Video, None)],
        );
        let base = log.head_seq("main");
        if merge {
            log.fork("br1", "main", base);
        }
        log.commit(
            "main",
            "b1",
            "bob",
            vec![add_track("$z", TrackKind::Video, None)],
        );
        log.commit("main", "b2", "bob", vec![remove_t2()]);
        let ops = vec![
            add_track("$x", TrackKind::Video, Some(1)),
            add_track("$y", TrackKind::Video, Some(2)),
        ];
        let bound = if merge {
            let rec = log.commit("br1", "a1", "alice", ops);
            log.merge("br1", "main", "carol", "g1", false).unwrap();
            rec.record.bindings
        } else {
            log.commit_at("main", "a1", "alice", base, ops)
                .unwrap()
                .record
                .bindings
        };
        assert_eq!(
            track_names(&log, &bound),
            ["t1", "$x", "$y", "t3", "t4"],
            "merge {merge}"
        );
    }
}

/// A `track.move` of a track made in the same transaction is carried by its neighbours too.
/// tl1 `[t1, t2, t3]`; bob removes t1; alice's stale `[add $n, move $n to 1]` (after t1, before
/// t2 in her frame) gives `[$n, t2, t3]`.
#[test]
fn move_of_a_new_track_is_carried_by_its_neighbours() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    log.commit(
        "main",
        "s0",
        "setup",
        vec![add_track("$t3", TrackKind::Video, None)],
    );
    let base = log.head_seq("main");
    let remove_t1 = TrackRemove {
        track: "t1".into(),
        with_items: true,
    };
    log.commit("main", "b1", "bob", vec![remove_t1.into()]);
    let ops = vec![add_track("$n", TrackKind::Video, None), move_track("$n", 1)];
    let bound = log
        .commit_at("main", "a1", "alice", base, ops)
        .unwrap()
        .record
        .bindings;
    assert_eq!(track_names(&log, &bound), ["$n", "t2", "t3"]);
}

/// Two pins of one key and file at different fallback places do not merge silently: the
/// stale one would leave the key where the other put it. `[Lora, Roboto]`; alice pins Inter at
/// 2; bob's stale identical pin at 0 is `SAME_PARAM` against alice.
#[test]
fn identical_pins_at_different_places_conflict() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let ops = vec![
        pin_at(LORA, "Lora", 'a', Some(0)),
        pin_at(ROBOTO, "Roboto", 'b', Some(1)),
    ];
    log.commit("main", "s0", "setup", ops);
    let base = log.head_seq("main");
    let inter_at = |i: u32| -> Op {
        FontPinOp {
            key: INTER.into(),
            pin: inter('c'),
            fallback: Some(i),
        }
        .into()
    };
    log.commit("main", "a1", "alice", vec![inter_at(2)]);
    let before = log.state("main");
    let e = log
        .commit_at("main", "b1", "bob", base, vec![inter_at(0)])
        .unwrap_err();
    assert_eq!(blame(&e), ("SAME_PARAM".into(), "a1".into()));
    assert_eq!(log.state("main"), before);
    // At the same place it is the same edit: it merges (written nothing).
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    log.commit(
        "main",
        "s0",
        "setup",
        vec![pin_at(LORA, "Lora", 'a', Some(0))],
    );
    let base = log.head_seq("main");
    log.commit("main", "a1", "alice", vec![inter_at(1)]);
    log.commit_at("main", "b1", "bob", base, vec![inter_at(1)])
        .unwrap();
    assert_eq!(log.state("main").fonts.fallback, [LORA, INTER]);
}

/// An end place whose only neighbour is gone goes to that end (the end is its other
/// neighbour). Your removal of the last fc, bob removes fb, you undo: `[fa, fc]`. Your
/// removal of the first fa, bob removes fb, you undo: `[fa, fc]`.
#[test]
fn end_place_whose_neighbour_is_gone_goes_to_that_end() {
    for (mine, theirs) in [("$fc", "$fb"), ("$fa", "$fb")] {
        let (mut log, b) = three_fx();
        log.commit("main", "x1", "alice", vec![remove_fx("c1", &b[mine])]);
        log.commit("main", "b1", "bob", vec![remove_fx("c1", &b[theirs])]);
        log.undo("main", "alice", "u1").unwrap();
        assert_eq!(fx_ids(&log), [b["$fa"].clone(), b["$fc"].clone()], "{mine}");
    }
}

/// tl1 `[t1, t2, s2, s3, s4]` (s2..s4 added after t2).
fn five_tracks() -> (MemLog, Ids) {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let ops = vec![
        add_track("$s2", TrackKind::Video, None),
        add_track("$s3", TrackKind::Video, None),
        add_track("$s4", TrackKind::Video, None),
    ];
    let b = log.commit("main", "s0", "setup", ops).record.bindings;
    (log, b)
}

fn remove_track(track: &str) -> Op {
    TrackRemove {
        track: track.into(),
        with_items: true,
    }
    .into()
}

/// A lone surviving neighbour is never trusted alone (spec §4.3): the nearest frame members on
/// both sides that are still there and that no other author moved since (list ends included)
/// must still be neighbours. So a placement keeps its frame order with every such member, and
/// is refused (`SAME_PARAM`, nothing written) when more than one place would. Each case landed
/// silently out of that order at b7f438e.
#[test]
fn surviving_neighbours_on_both_sides_must_still_meet() {
    let names = |log: &MemLog, b: &Ids, extra: &Ids| -> Vec<String> {
        let mut all = b.clone();
        all.extend(extra.clone());
        track_names(log, &all)
    };
    // Bob removes s2 and moves s3 to the end; alice's stale add at 3 (between s2 and s3). s3
    // moved, so x keeps its order with t2 and s4: `[t1, t2, $x, s4, s3]` (b7f438e put it
    // after s4).
    let (mut log, b) = five_tracks();
    let base = log.head_seq("main");
    log.commit("main", "b1", "bob", vec![remove_track(&b["$s2"])]);
    log.commit("main", "b2", "bob", vec![move_track(&b["$s3"], 3)]);
    let x = add_track("$x", TrackKind::Video, Some(3));
    let r = log.commit_at("main", "a1", "alice", base, vec![x]).unwrap();
    let got = names(&log, &b, &r.record.bindings);
    assert_eq!(got, ["t1", "t2", "$x", "$s4", "$s3"]);
    // Alice removes s2; bob removes t2 and moves s3 to the end; alice undoes: s2 goes back
    // before s4 (b7f438e flipped it past s4).
    let (mut log, b) = five_tracks();
    log.commit("main", "x1", "alice", vec![remove_track(&b["$s2"])]);
    log.commit("main", "b1", "bob", vec![remove_t2()]);
    log.commit("main", "b2", "bob", vec![move_track(&b["$s3"], 2)]);
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(names(&log, &b, &Ids::new()), ["t1", "$s2", "$s4", "$s3"]);
    // Bob inserts z between s2 and s3, then removes s2; alice's stale add at 3 (between s2
    // and s3), or at 2 (between t2 and s2): either side of z keeps her order, a conflict.
    for at in [3, 2] {
        let (mut log, b) = five_tracks();
        let base = log.head_seq("main");
        log.commit(
            "main",
            "b1",
            "bob",
            vec![add_track("$z", TrackKind::Video, Some(3))],
        );
        log.commit("main", "b2", "bob", vec![remove_track(&b["$s2"])]);
        let before = log.state("main");
        let x = add_track("$x", TrackKind::Video, Some(at));
        let e = log
            .commit_at("main", "a1", "alice", base, vec![x])
            .unwrap_err();
        assert_eq!(blame(&e).0, "SAME_PARAM", "at {at}: {e:?}");
        assert_eq!(log.state("main"), before, "at {at}");
    }
    // The end variant: alice removes the last s4; bob appends z and removes s3; alice undoes.
    let (mut log, b) = five_tracks();
    log.commit("main", "x1", "alice", vec![remove_track(&b["$s4"])]);
    log.commit(
        "main",
        "b1",
        "bob",
        vec![add_track("$z", TrackKind::Video, None)],
    );
    log.commit("main", "b2", "bob", vec![remove_track(&b["$s3"])]);
    let e = log.undo("main", "alice", "u1").unwrap_err();
    assert_eq!(blame(&e).0, "SAME_PARAM", "{e:?}");
    // `[A, B, C]` minus B still merges.
    let (mut log, b) = five_tracks();
    log.commit("main", "x1", "alice", vec![remove_track(&b["$s3"])]);
    log.commit("main", "b1", "bob", vec![remove_track(&b["$s2"])]);
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(names(&log, &b, &Ids::new()), ["t1", "t2", "$s3", "$s4"]);
}

/// A stale pin of a key that is pinned and placed already in its own frame, with the same
/// file still pinned at the head, is the same edit: it merges even when someone placed
/// another key since. `[Lora, Roboto]`; bob pins Inter at 0; alice's stale `pin Lora at 0`.
#[test]
fn a_no_op_identical_pin_merges() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let ops = vec![
        pin_at(LORA, "Lora", 'a', Some(0)),
        pin_at(ROBOTO, "Roboto", 'b', Some(1)),
    ];
    log.commit("main", "s0", "setup", ops);
    let base = log.head_seq("main");
    let inter = FontPinOp {
        key: INTER.into(),
        pin: inter('c'),
        fallback: Some(0),
    };
    log.commit("main", "b1", "bob", vec![inter.into()]);
    let again = pin_at(LORA, "Lora", 'a', Some(0));
    log.commit_at("main", "a1", "alice", base, vec![again])
        .unwrap();
    assert_eq!(log.state("main").fonts.fallback, [INTER, LORA, ROBOTO]);
}

/// Effect placements elsewhere in the list never meet (spec §4.3): no `effects` parameter
/// stands for the order any more. Bob adds an effect at the start; alice's stale move of fc
/// before fb and her stale add at the end both merge.
#[test]
fn effect_placements_elsewhere_merge() {
    let (mut log, b) = three_fx();
    let base = log.head_seq("main");
    let first = EffectAdd {
        item: "c1".into(),
        id: "$fd".into(),
        kind: "blur".into(),
        at: Some(0),
        params: BTreeMap::new(),
    };
    let fd = log
        .commit("main", "b1", "bob", vec![first.into()])
        .record
        .bindings["$fd"]
        .clone();
    let fc_before_fb = fin_core::EffectMove {
        item: "c1".into(),
        effect: b["$fc"].clone(),
        to: 1,
    };
    let last = EffectAdd {
        item: "c1".into(),
        id: "$fe".into(),
        kind: "blur".into(),
        at: Some(3),
        params: BTreeMap::new(),
    };
    let ops = vec![fc_before_fb.into(), last.into()];
    let r = log.commit_at("main", "a1", "alice", base, ops).unwrap();
    let fe = r.record.bindings["$fe"].clone();
    let want = [fd, b["$fa"].clone(), b["$fc"].clone(), b["$fb"].clone(), fe];
    assert_eq!(fx_ids(&log), want);
}
