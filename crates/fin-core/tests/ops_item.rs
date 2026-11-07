//! The `item.*` ops (plan 4C Task 7): goldens of apply, inverse and shifts for each mode, with
//! `inverse_shifts_mirror` (Decision 23); per-kind insert rules; content offsets (Decision 13);
//! track lock; the replay track check over touched tracks (B1); asset kinds; value shapes.

mod common;

use std::collections::BTreeMap;

use common::ids_json::ids_cover_every_id_in_json;
use common::*;
use fin_core::rebase::StepMap;
use fin_core::{
    Applied, AssetAdd, AssetKind, CaptionBlock, ChangeKind, DeleteMode, Edge, FontFace, FontFile,
    FontFormat, ImageInfo, InsertMode, Interp, Item, ItemBody, ItemDelete, ItemInsert, ItemRemove,
    ItemRestore, ItemTrim, Keyframe, LottieInfo, LutInfo, MemIds, Op, ParamValue, Project,
    Rational, Shift, Track, TrackKind, TrimMode, Word, apply_committed, apply_internal,
    apply_tx_mode,
};
use serde_json::json;

fn shift(track: &str, from: Rational, by: Rational, op: u32) -> Shift {
    Shift {
        track: track.into(),
        from,
        by,
        op,
    }
}

/// Decision 23: the inverse records the op's shifts inverted, in exact reverse order.
fn inverse_shifts_mirror(applied: &Applied) {
    let p = &applied.project;
    let back = apply_internal(p, &tx(applied.inverse.clone()), &mut MemIds::after(p)).unwrap();
    let want = StepMap::from_diff(&applied.diff).invert().unwrap();
    let strip = |s: &[Shift]| -> Vec<(String, Rational, Rational)> {
        s.iter().map(|s| (s.track.clone(), s.from, s.by)).collect()
    };
    assert_eq!(
        strip(&back.diff.shifts),
        strip(want.shifts()),
        "{applied:?}"
    );
}

/// Applies `ops`, checks the inverse restores `p` exactly and mirrors the shifts.
fn golden(p: &Project, ops: Vec<Op>) -> Applied {
    let a = apply(p, ops);
    assert_eq!(undo(&a), *p, "inverse restores the project");
    inverse_shifts_mirror(&a);
    a
}

/// `project()` with a caption track `t3`.
fn with_caption_track(mut p: Project) -> Project {
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    p
}

fn insert_body(id: &str, track: &str, at: Rational, duration: Rational, body: ItemBody) -> Op {
    ItemInsert {
        id: id.into(),
        track: track.into(),
        at,
        duration,
        mode: InsertMode::Ripple,
        body,
    }
    .into()
}

fn strict(item: &str, edge: Edge, delta: Rational) -> Op {
    ItemTrim {
        item: item.into(),
        edge,
        delta,
        mode: TrimMode::Strict,
        alone: false,
    }
    .into()
}

fn lift(item: &str) -> Op {
    ItemDelete {
        item: item.into(),
        mode: DeleteMode::Lift,
    }
    .into()
}

fn asset_of(kind: AssetKind, id: &str) -> AssetAdd {
    let mut a = AssetAdd::media(id, "/m/file", secs(10), None, None, "fp");
    a.kind = kind;
    a.duration = None;
    match kind {
        AssetKind::Media => {
            a.duration = Some(secs(10));
            a.video = Some(fin_core::VideoInfo {
                width: 2,
                height: 2,
                rate: secs(30),
            });
        }
        AssetKind::Image | AssetKind::Svg => {
            a.image = Some(ImageInfo {
                width: 640,
                height: 480,
            })
        }
        AssetKind::LottieTemplate => {
            a.duration = Some(secs(3));
            a.lottie = Some(LottieInfo {
                width: 1080,
                height: 1920,
                rate: secs(30),
                duration: secs(3),
                // The slots the graphic inserts below set (checked since Task 11).
                slots: vec![
                    fin_core::SlotSpec {
                        id: "title".into(),
                        kind: "text".into(),
                    },
                    fin_core::SlotSpec {
                        id: "size".into(),
                        kind: "number".into(),
                    },
                ],
                fonts: Vec::new(),
            });
        }
        AssetKind::Font => {
            a.font = Some(FontFile {
                sha256: "ab".repeat(32),
                size: 1234,
                format: FontFormat::Ttf,
                faces: vec![FontFace {
                    index: 0,
                    family: "Test Sans".into(),
                    weight: 400,
                    italic: false,
                    scripts: vec!["Latn".into()],
                }],
            })
        }
        AssetKind::Lut => a.lut = Some(LutInfo { size: 33 }),
    }
    a
}

fn caption_body(words: &[(&str, i64, i64)]) -> ItemBody {
    ItemBody::Caption {
        block: CaptionBlock {
            words: words
                .iter()
                .map(|(t, s, e)| Word {
                    text: (*t).into(),
                    start: secs(*s),
                    end: secs(*e),
                })
                .collect(),
            style: "preset:bold".into(),
            overrides: BTreeMap::new(),
            source: None,
        },
    }
}

// ---- item.insert ----

#[test]
fn insert_clip_golden() {
    let p = with_clips(&[5, 3]);
    let a = golden(
        &p,
        vec![insert("$c", "t1", secs(5), "a1", secs(1), r("5/2"))],
    );
    assert_eq!(a.bindings["$c"], "c3");
    assert_eq!(
        starts(&a.project, 0),
        [
            ("c1".into(), secs(0)),
            ("c3".into(), secs(5)),
            ("c2".into(), r("15/2"))
        ]
    );
    assert_eq!(a.diff.shifts, [shift("t1", secs(5), r("5/2"), 0)]);
    assert_eq!(
        (a.diff.changes[0].kind, a.diff.changes[0].change),
        (fin_core::EntityKind::Item, ChangeKind::Added)
    );
    assert_eq!(
        a.inverse,
        [ItemRemove {
            id: "c3".into(),
            mode: DeleteMode::Ripple
        }
        .into()]
    );
    assert_eq!(
        serde_json::to_value(insert("$c", "t1", r("1/2"), "a1", secs(0), secs(3))).unwrap(),
        json!({"op": "item.insert", "id": "$c", "track": "t1", "at": "1/2", "duration": "3",
               "mode": "ripple", "kind": "clip", "asset": "a1", "sourceIn": "0"})
    );
    assert_well_formed(&a.project);
}

#[test]
fn insert_each_allowed_kind_golden() {
    let p = with_caption_track(project());
    let setup = apply(
        &p,
        vec![
            asset_of(AssetKind::Image, "$img").into(),
            asset_of(AssetKind::Svg, "$svg").into(),
            asset_of(AssetKind::LottieTemplate, "$lot").into(),
        ],
    )
    .project;
    let cases: Vec<(&str, &str, ItemBody, &str)> = vec![
        ("t1", "$i", ItemBody::Image { asset: "a1".into() }, "img1"),
        ("t1", "$i", ItemBody::Image { asset: "a2".into() }, "img1"),
        (
            "t1",
            "$g",
            ItemBody::Graphic {
                asset: "a3".into(),
                slots: [("title".to_string(), ParamValue::Text("Hi".into()))].into(),
            },
            "g1",
        ),
        ("t1", "$x", ItemBody::Adjustment {}, "adj1"),
        ("t3", "$k", caption_body(&[("hi", 0, 1)]), "cap1"),
    ];
    for (track, ph, body, id) in cases {
        let a = golden(
            &setup,
            vec![insert_body(ph, track, secs(2), secs(2), body.clone())],
        );
        assert_eq!(a.bindings[ph], id, "{body:?}");
        let t = a.project.timelines[0]
            .tracks
            .iter()
            .find(|t| t.id == track)
            .unwrap();
        assert_eq!(t.items[0].id, id);
        assert_eq!(t.items[0].body, body);
        assert_eq!(a.diff.shifts, [shift(track, secs(2), secs(2), 0)]);
    }
}

#[test]
fn insert_rules_per_kind() {
    let p = with_caption_track(with_clips(&[]));
    let p = apply(
        &p,
        vec![
            asset_of(AssetKind::Image, "$img").into(),
            asset_of(AssetKind::Font, "$f").into(),
        ],
    )
    .project;
    let code = |track: &str, body: ItemBody| {
        try_apply(&p, vec![insert_body("$i", track, secs(0), secs(1), body)])
            .unwrap_err()
            .code
    };
    // Track kinds.
    assert_eq!(
        code("t2", ItemBody::Image { asset: "a2".into() }),
        "TRACK_KIND_MISMATCH"
    );
    assert_eq!(code("t3", ItemBody::Adjustment {}), "TRACK_KIND_MISMATCH");
    assert_eq!(
        code("t1", caption_body(&[("hi", 0, 1)])),
        "TRACK_KIND_MISMATCH"
    );
    let clip_on_caption = ItemBody::Clip {
        asset: "a1".into(),
        source_in: secs(0),
        speed: secs(1),
    };
    assert_eq!(code("t3", clip_on_caption), "TRACK_KIND_MISMATCH");
    // Asset kinds: an image shows an image or svg; a graphic a template; a clip media.
    assert_eq!(
        code("t1", ItemBody::Image { asset: "a1".into() }),
        "INVALID_OP"
    );
    let graphic = ItemBody::Graphic {
        asset: "a2".into(),
        slots: BTreeMap::new(),
    };
    assert_eq!(code("t1", graphic), "INVALID_OP");
    let font_clip = ItemBody::Clip {
        asset: "a3".into(),
        source_in: secs(0),
        speed: secs(1),
    };
    assert_eq!(code("t1", font_clip), "TRACK_KIND_MISMATCH");
    // Nested items arrive in sub-project 6.
    let e = try_apply(
        &p,
        vec![insert_body(
            "$n",
            "t1",
            secs(0),
            secs(1),
            ItemBody::Nested {
                timeline: "tl1".into(),
                source_in: secs(0),
            },
        )],
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_OP");
    assert!(e.fix.unwrap().contains("sub-project 6"));
    // A clip's speed is positive.
    for speed in [secs(0), secs(-1)] {
        let body = ItemBody::Clip {
            asset: "a1".into(),
            source_in: secs(0),
            speed,
        };
        assert_eq!(code("t1", body), "INVALID_RANGE");
    }
    // One frame at least, for every kind.
    let e = try_apply(
        &p,
        vec![insert_body(
            "$x",
            "t1",
            secs(0),
            r("1/60"),
            ItemBody::Adjustment {},
        )],
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["min"]),
        ("INVALID_RANGE", &json!("1/30"))
    );
}

#[test]
fn insert_with_speed_checks_the_source_extent() {
    let p = with_clips(&[]);
    let fast = |dur: Rational| {
        ItemBody::Clip {
            asset: "a1".into(),
            source_in: secs(90),
            speed: secs(2),
        }
        .into_insert(dur)
    };
    // 90 + 5 × 2 = 100: the whole asset.
    let a = golden(&p, vec![fast(secs(5))]);
    assert_eq!(clip(&a.project, "c1").speed, secs(2));
    let e = try_apply(&p, vec![fast(r("11/2"))]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["max"]),
        ("INVALID_RANGE", &json!("5"))
    );
}

trait IntoInsert {
    fn into_insert(self, duration: Rational) -> Op;
}

impl IntoInsert for ItemBody {
    fn into_insert(self, duration: Rational) -> Op {
        insert_body("$c", "t1", secs(0), duration, self)
    }
}

#[test]
fn insert_policy_and_placeholders() {
    let p = with_clips(&[5]);
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(4), "a1", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(e.code, "INSERT_INSIDE_CLIP");
    assert_eq!(
        (&e.details["clip"], &e.details["item"]),
        (&json!("c1"), &json!("c1"))
    );
    assert_eq!(e.details["nearest"], "5");
    assert!(e.fix.as_deref().unwrap().contains("nearest is 5"));
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(0), "a1", secs(98), secs(3))],
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["max"]),
        ("INVALID_RANGE", &json!("2"))
    );
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(-1), "a1", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    let e = try_apply(
        &p,
        vec![insert("$c", "t9", secs(0), "a1", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["id"]),
        ("NOT_FOUND", &json!("t9"))
    );
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(5), "$no", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(e.code, "UNBOUND_PLACEHOLDER");
    let e = try_apply(
        &p,
        vec![insert("c9", "t1", secs(5), "a1", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_OP");
    let before = p.clone();
    let e = try_apply(
        &p,
        vec![
            insert("$c", "t1", secs(5), "a1", secs(0), secs(1)),
            insert("$d", "t1", secs(1), "a1", secs(0), secs(1)),
        ],
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["opIndex"]),
        ("INSERT_INSIDE_CLIP", &json!(1))
    );
    assert_eq!(p, before);
}

#[test]
fn clients_cannot_send_internal_ops_or_strict_trims() {
    let p = with_clips(&[5]);
    for op in [
        ItemRemove {
            id: "c1".into(),
            mode: DeleteMode::Ripple,
        }
        .into(),
        strict("c1", Edge::End, secs(-1)),
    ] {
        let e = try_apply(&p, vec![op.clone()]).unwrap_err();
        assert_eq!(e.code, "INVALID_OP", "{op:?}");
    }
    // `asset.remove` is a client op since plan 4C Task 13 (a1 is in use here).
    let e = try_apply(&p, vec![Op::AssetRemove { id: "a1".into() }]).unwrap_err();
    assert_eq!(e.code, "ASSET_IN_USE");
}

// ---- item.delete, item.remove, item.restore ----

#[test]
fn delete_ripple_golden() {
    let p = with_clips(&[5, 3, 2]);
    let a = golden(&p, vec![delete("c2")]);
    assert_eq!(
        starts(&a.project, 0),
        [("c1".into(), secs(0)), ("c3".into(), secs(5))]
    );
    assert_eq!(a.diff.shifts, [shift("t1", secs(8), secs(-3), 0)]);
    assert_eq!(a.diff.changes[0].change, ChangeKind::Removed);
    let item = Item::clip("c2", "a1", secs(5), secs(0), secs(3));
    assert_eq!(
        a.inverse,
        [ItemRestore {
            item,
            track: "t1".into(),
            mode: DeleteMode::Ripple
        }
        .into()]
    );
    golden(&p, vec![delete("c1"), delete("c3")]);
}

#[test]
fn delete_lift_golden() {
    let p = with_clips(&[5, 3, 2]);
    let a = golden(&p, vec![lift("c2")]);
    assert_eq!(
        starts(&a.project, 0),
        [("c1".into(), secs(0)), ("c3".into(), secs(8))]
    );
    assert!(a.diff.shifts.is_empty(), "a lift records no shift");
    assert!(matches!(&a.inverse[..], [Op::ItemRestore(r)] if r.mode == DeleteMode::Lift));
    // The lift restore records none either, and puts the item back in its gap.
    let back = apply_internal(
        &a.project,
        &tx(a.inverse.clone()),
        &mut MemIds::after(&a.project),
    )
    .unwrap();
    assert!(back.diff.shifts.is_empty());
    assert_eq!(
        back.inverse,
        [ItemRemove {
            id: "c2".into(),
            mode: DeleteMode::Lift
        }
        .into()]
    );
}

#[test]
fn delete_any_kind_and_unknown_item() {
    let p = with_caption_track(project());
    let a = apply(
        &p,
        vec![
            insert_body("$x", "t1", secs(0), secs(2), ItemBody::Adjustment {}),
            insert_body("$k", "t3", secs(0), secs(2), caption_body(&[("a", 0, 1)])),
        ],
    );
    golden(&a.project, vec![delete("adj1"), lift("cap1")]);
    let e = try_apply(&a.project, vec![delete("c9")]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["kind"]),
        ("NOT_FOUND", &json!("item"))
    );
}

/// A busy item (effects, link, keyframes, style) round-trips through delete and restore, and
/// the restore lists every id it carries (Decision 24).
#[test]
fn restore_of_a_busy_item_is_exact() {
    let mut p = with_clips(&[5]);
    let item = &mut p.timelines[0].tracks[0].items[0];
    item.common.effects.push(fin_core::Effect {
        id: "fx4".into(),
        kind: "blur".into(),
        enabled: true,
        params: [("radius".to_string(), ParamValue::Number(0.1))].into(),
    });
    item.common.link = Some("lk2".into());
    item.common.keyframes.insert(
        "transform.position".into(),
        vec![Keyframe {
            at: r("1/3"),
            value: ParamValue::Vec2([1.5, -2.25]),
            interp: Interp::Bezier {
                x1: 0.25,
                y1: 0.1,
                x2: 0.25,
                y2: 1.0,
            },
        }],
    );
    let a = golden(&p, vec![delete("c1")]);
    let [restore] = &a.inverse[..] else {
        panic!("{:?}", a.inverse)
    };
    ids_cover_every_id_in_json(restore);
    assert!(restore.ids().contains(&"fx4") && restore.ids().contains(&"lk2"));
}

#[test]
fn restore_with_a_placeholder_allocates() {
    let p = with_clips(&[5]);
    let op: Op = ItemRestore {
        item: Item::clip("$r", "a1", secs(5), secs(0), secs(2)),
        track: "t1".into(),
        mode: DeleteMode::Ripple,
    }
    .into();
    assert_eq!(op.allocates(), Some("$r"));
    let a = apply_internal(&p, &tx(vec![op]), &mut MemIds::after(&p)).unwrap();
    assert_eq!(a.bindings["$r"], "c2");
    assert_eq!(clip(&a.project, "c2").start, secs(5));
}

// ---- item.trim ----

#[test]
fn trim_ripple_start_golden() {
    let p = with_clips(&[5, 3]);
    let a = golden(&p, vec![trim("c1", Edge::Start, secs(1))]);
    let c1 = clip(&a.project, "c1");
    assert_eq!(
        (c1.start, c1.source_in, c1.duration),
        (secs(0), secs(1), secs(4))
    );
    assert_eq!(clip(&a.project, "c2").start, secs(4));
    assert_eq!(a.diff.shifts, [shift("t1", secs(5), secs(-1), 0)]);
    let c = &a.diff.changes[0];
    assert_eq!(
        (c.change, c.edge, c.by),
        (ChangeKind::Updated, Some(Edge::Start), Some(secs(-1)))
    );
    assert_eq!(a.inverse, [trim_back("c1", Edge::Start, secs(-1))]);
    assert_eq!(
        serde_json::to_value(trim("c1", Edge::Start, secs(1))).unwrap(),
        json!({"op": "item.trim", "item": "c1", "edge": "start", "delta": "1", "mode": "ripple"})
    );
}

#[test]
fn trim_ripple_end_golden() {
    let p = with_clips(&[5, 3]);
    let a = golden(&p, vec![trim("c1", Edge::End, r("1/2"))]);
    assert_eq!(clip(&a.project, "c1").duration, r("11/2"));
    assert_eq!(clip(&a.project, "c2").start, r("11/2"));
    assert_eq!(a.diff.shifts, [shift("t1", secs(5), r("1/2"), 0)]);
    let c = &a.diff.changes[0];
    assert_eq!((c.edge, c.by), (Some(Edge::End), None));
}

#[test]
fn trim_limits() {
    let p = with_clips(&[5]);
    let e = try_apply(&p, vec![trim("c1", Edge::End, secs(96))]).unwrap_err();
    assert_eq!(
        (e.details["max"].clone(), e.details["min"].clone()),
        (json!("95"), json!("-149/30"))
    );
    let e = try_apply(&p, vec![trim("c1", Edge::Start, secs(-1))]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["min"]),
        ("INVALID_RANGE", &json!("0"))
    );
    for op in [
        trim("c1", Edge::Start, secs(5)),
        trim("c1", Edge::End, secs(-5)),
    ] {
        assert_eq!(try_apply(&p, vec![op]).unwrap_err().code, "INVALID_RANGE");
    }
    let e = try_apply(&p, vec![trim("c9", Edge::End, secs(1))]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["id"]),
        ("NOT_FOUND", &json!("c9"))
    );
}

/// Review of Task 6: the one-frame minimum is the item's timeline's frame, not the project's.
#[test]
fn trim_uses_the_timeline_rate() {
    let mut p = with_clips(&[5]);
    p.timelines[0].rate = secs(25);
    let e = try_apply(&p, vec![trim("c1", Edge::End, r("-299/60"))]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["min"]),
        ("INVALID_RANGE", &json!("-124/25"))
    );
    apply(&p, vec![trim("c1", Edge::End, r("-124/25"))]);
}

#[test]
fn trim_with_speed_moves_the_source_by_delta_times_speed() {
    let p = with_clips(&[]);
    let body = ItemBody::Clip {
        asset: "a1".into(),
        source_in: secs(10),
        speed: r("3/2"),
    };
    let p = apply(&p, vec![body.into_insert(secs(20))]).project;
    let a = golden(&p, vec![trim("c1", Edge::Start, secs(2))]);
    let c = clip(&a.project, "c1");
    assert_eq!((c.source_in, c.duration), (secs(13), secs(18)));
    // Start: down to sourceIn 0, i.e. -10 / (3/2).
    let e = try_apply(&p, vec![trim("c1", Edge::Start, secs(-7))]).unwrap_err();
    assert_eq!(e.details["min"], "-20/3");
    golden(&p, vec![trim("c1", Edge::Start, r("-20/3"))]);
    // End: up to the asset's end, (100 - 10) / (3/2) - 20 = 40.
    let e = try_apply(&p, vec![trim("c1", Edge::End, secs(41))]).unwrap_err();
    assert_eq!(e.details["max"], "40");
    golden(&p, vec![trim("c1", Edge::End, secs(40))]);
}

#[test]
fn trim_items_without_a_source() {
    let p = with_caption_track(project());
    let p = apply(
        &p,
        vec![insert_body(
            "$x",
            "t1",
            secs(0),
            secs(2),
            ItemBody::Adjustment {},
        )],
    )
    .project;
    // Extending the start of an item with no source has no lower limit in ripple mode.
    let a = golden(&p, vec![trim("adj1", Edge::Start, secs(-5))]);
    assert_eq!(a.project.timelines[0].tracks[0].items[0].duration, secs(7));
    let e = try_apply(&p, vec![trim("adj1", Edge::Start, secs(2))]).unwrap_err();
    assert_eq!(e.details["max"], "59/30");
}

#[test]
fn strict_trim_golden() {
    let mut p = with_clips(&[5, 3]);
    p.timelines[0].tracks[0].items[0] = Item::clip("c1", "a1", secs(1), secs(1), secs(3));
    let ops = vec![
        strict("c1", Edge::Start, secs(-1)),
        strict("c1", Edge::End, secs(1)),
    ];
    let a = apply_internal(&p, &tx(ops), &mut MemIds::after(&p)).unwrap();
    let c = clip(&a.project, "c1");
    assert_eq!(
        (c.start, c.source_in, c.duration),
        (secs(0), secs(0), secs(5))
    );
    assert!(a.diff.shifts.is_empty(), "strict moves nothing else");
    assert_eq!(a.diff.changes[0].by, Some(secs(1)));
    assert_eq!(clip(&a.project, "c2").start, secs(5));
    assert_eq!(
        a.inverse,
        [
            strict("c1", Edge::End, secs(-1)),
            strict("c1", Edge::Start, secs(1))
        ]
    );
    let back = apply_internal(
        &a.project,
        &tx(a.inverse.clone()),
        &mut MemIds::after(&a.project),
    )
    .unwrap();
    assert_eq!(back.project, p);
    assert!(back.diff.shifts.is_empty());
}

#[test]
fn strict_trim_refuses_overlap_in_every_mode() {
    let p = with_clips(&[5, 3]);
    let ops = vec![strict("c1", Edge::End, secs(1))];
    for mode in [fin_core::ApplyMode::Internal, fin_core::ApplyMode::Replay] {
        let e = apply_tx_mode(&p, &tx(ops.clone()), &mut MemIds::after(&p), mode).unwrap_err();
        assert_eq!(e.code, "CLIP_OVERLAP", "{mode:?}");
        assert_eq!(e.details["clips"], json!(["c1", "c2"]));
    }
    // A start edge cannot move over the previous item either (c2 has source to reveal).
    let mut p = p;
    p.timelines[0].tracks[0].items[1] = Item::clip("c2", "a1", secs(5), secs(2), secs(3));
    for mode in [fin_core::ApplyMode::Internal, fin_core::ApplyMode::Replay] {
        let ops = vec![strict("c2", Edge::Start, secs(-1))];
        let e = apply_tx_mode(&p, &tx(ops), &mut MemIds::after(&p), mode).unwrap_err();
        assert_eq!(e.code, "CLIP_OVERLAP", "{mode:?}");
    }
}

/// Decision 13: a start-edge move shifts keyframes and caption words by `−delta`, records
/// `by = −delta`, and its inverse moves them back exactly.
#[test]
fn start_trim_rebases_keyframes_and_words() {
    let mut p = with_caption_track(with_clips(&[5]));
    p.timelines[0].tracks[0].items[0].common.keyframes.insert(
        "opacity".into(),
        vec![
            Keyframe {
                at: secs(0),
                value: ParamValue::Number(0.0),
                interp: Interp::Linear,
            },
            Keyframe {
                at: secs(4),
                value: ParamValue::Number(1.0),
                interp: Interp::Hold,
            },
        ],
    );
    let p = apply(
        &p,
        vec![insert_body(
            "$k",
            "t3",
            secs(0),
            secs(4),
            caption_body(&[("a", 1, 2), ("b", 2, 3)]),
        )],
    )
    .project;
    let a = golden(
        &p,
        vec![
            trim("c1", Edge::Start, secs(1)),
            trim("cap1", Edge::Start, r("3/2")),
        ],
    );
    let ats: Vec<Rational> = a.project.timelines[0].tracks[0].items[0].common.keyframes["opacity"]
        .iter()
        .map(|k| k.at)
        .collect();
    // Kept even where it falls outside the item (eval clamps).
    assert_eq!(ats, [secs(-1), secs(3)]);
    let ItemBody::Caption { block } = &a.project.timelines[0].tracks[2].items[0].body else {
        panic!()
    };
    let times: Vec<(Rational, Rational)> = block.words.iter().map(|w| (w.start, w.end)).collect();
    assert_eq!(times, [(r("-1/2"), r("1/2")), (r("1/2"), r("3/2"))]);
    let bys: Vec<Option<Rational>> = a.diff.changes.iter().map(|c| c.by).collect();
    assert_eq!(bys, [Some(secs(-1)), Some(r("-3/2"))]);
}

// ---- track lock ----

#[test]
fn track_locked_refuses_item_ops() {
    let mut p = with_clips(&[5, 3]);
    p.timelines[0].tracks[0].lock = true;
    for op in [
        insert("$c", "t1", secs(8), "a1", secs(0), secs(1)),
        trim("c1", Edge::End, secs(1)),
        delete("c2"),
        lift("c2"),
    ] {
        let e = try_apply(&p, vec![op.clone()]).unwrap_err();
        assert_eq!(
            (e.code.as_str(), &e.details["track"]),
            ("TRACK_LOCKED", &json!("t1")),
            "{op:?}"
        );
    }
    // Internal ops are refused too (an undo meets the lock).
    let e = apply_internal(
        &p,
        &tx(vec![
            ItemRemove {
                id: "c1".into(),
                mode: DeleteMode::Ripple,
            }
            .into(),
        ]),
        &mut MemIds::after(&p),
    )
    .unwrap_err();
    assert_eq!(e.code, "TRACK_LOCKED");
    // Policy: replay applies.
    let intent = tx(vec![delete("c2")]);
    let a = apply_committed(&p, &intent, &BTreeMap::new()).unwrap();
    assert_eq!(a.project.timelines[0].tracks[0].items.len(), 1);
}

// ---- replay ----

/// B1 (Decision 25): replay checks every track the op's diff touched, not only shifted ones:
/// a hand-made lift restore that overlaps records no shift and is still refused.
#[test]
fn replay_checks_tracks_touched_by_changes() {
    let p = with_clips(&[5, 3]);
    let overlap: Op = ItemRestore {
        item: Item::clip("c9", "a1", secs(4), secs(0), secs(2)),
        track: "t1".into(),
        mode: DeleteMode::Lift,
    }
    .into();
    let e = apply_committed(&p, &tx(vec![overlap]), &BTreeMap::new()).unwrap_err();
    assert_eq!(e.code, "REPLAY_FAILED");
    assert_eq!(e.details["cause"]["code"], "CLIP_OVERLAP");
    // A strict trim records no shift either.
    let e = apply_committed(
        &p,
        &tx(vec![strict("c1", Edge::End, secs(2))]),
        &BTreeMap::new(),
    )
    .unwrap_err();
    assert_eq!(e.details["cause"]["code"], "CLIP_OVERLAP");
    // Clean ones replay.
    let ok: Op = ItemRestore {
        item: Item::clip("c9", "a1", secs(8), secs(0), secs(2)),
        track: "t1".into(),
        mode: DeleteMode::Lift,
    }
    .into();
    apply_committed(&p, &tx(vec![ok]), &BTreeMap::new()).unwrap();
}

#[test]
fn replay_refuses_aliases_and_internal_mode_too() {
    let p = with_clips(&[5]);
    let alias = Op::ClipDelete { clip: "c1".into() };
    let e = apply_committed(&p, &tx(vec![alias.clone()]), &BTreeMap::new()).unwrap_err();
    assert_eq!(e.details["cause"]["code"], "INVALID_OP");
    let e = apply_internal(&p, &tx(vec![alias.clone()]), &mut MemIds::after(&p)).unwrap_err();
    assert_eq!(e.code, "INVALID_OP");
    // Clients may send it: it applies as item.delete.
    let a = apply(&p, vec![alias]);
    assert_eq!(a.diff, apply(&p, vec![delete("c1")]).diff);
}

// ---- asset.add ----

#[test]
fn asset_add_validates_each_kind() {
    let p = project();
    for kind in [
        AssetKind::Media,
        AssetKind::Image,
        AssetKind::Svg,
        AssetKind::LottieTemplate,
        AssetKind::Font,
        AssetKind::Lut,
    ] {
        let a = golden(&p, vec![asset_of(kind, "$a").into()]);
        let got = &a.project.assets["a1"];
        assert_eq!(got.kind, kind);
        // The inverse of the inverse gives the asset back with all its fields.
        let [Op::AssetRemove { .. }] = &a.inverse[..] else {
            panic!()
        };
        let removed = apply_internal(
            &a.project,
            &tx(a.inverse.clone()),
            &mut MemIds::after(&a.project),
        )
        .unwrap();
        assert_eq!(
            removed.inverse,
            [Op::from(asset_of(kind, "a1"))],
            "{kind:?}"
        );
        // Another kind's field is refused.
        let mut bad = asset_of(kind, "$a");
        if kind == AssetKind::Lut {
            bad.image = Some(ImageInfo {
                width: 1,
                height: 1,
            });
        } else {
            bad.lut = Some(LutInfo { size: 2 });
        }
        assert_eq!(
            try_apply(&p, vec![bad.into()]).unwrap_err().code,
            "INVALID_OP",
            "{kind:?}"
        );
    }
    let code = |f: fn(&mut AssetAdd), kind: AssetKind| {
        let mut a = asset_of(kind, "$a");
        f(&mut a);
        try_apply(&p, vec![a.into()])
            .map(|_| ())
            .map_err(|e| e.code)
    };
    assert_eq!(
        code(|a| a.duration = Some(secs(0)), AssetKind::Media),
        Err("INVALID_RANGE".into())
    );
    assert_eq!(
        code(|a| a.duration = None, AssetKind::Media),
        Err("INVALID_OP".into())
    );
    assert_eq!(
        code(|a| a.video = None, AssetKind::Media),
        Err("INVALID_OP".into())
    );
    assert_eq!(
        code(|a| a.duration = Some(secs(1)), AssetKind::Image),
        Err("INVALID_OP".into())
    );
    assert_eq!(
        code(|a| a.lottie = None, AssetKind::LottieTemplate),
        Err("INVALID_OP".into())
    );
    assert_eq!(
        code(
            |a| a.font.as_mut().unwrap().sha256 = "AB".repeat(32),
            AssetKind::Font
        ),
        Err("INVALID_OP".into())
    );
    let short = |a: &mut AssetAdd| {
        a.font.as_mut().unwrap().sha256.pop();
    };
    assert_eq!(code(short, AssetKind::Font), Err("INVALID_OP".into()));
    assert_eq!(
        code(|a| a.font.as_mut().unwrap().faces.clear(), AssetKind::Font),
        Err("INVALID_OP".into())
    );
    assert_eq!(
        code(|a| a.lut = Some(LutInfo { size: 1 }), AssetKind::Lut),
        Err("INVALID_RANGE".into())
    );
    assert_eq!(
        code(|a| a.bin = Some("b1".into()), AssetKind::Image),
        Err("NOT_FOUND".into())
    );
    // A durationless asset can be removed now (Task 6 refused it).
    let a = apply(&p, vec![asset_of(AssetKind::Image, "$i").into()]);
    assert_eq!(undo(&a), p);
}

#[test]
fn v1_asset_add_reads_as_media() {
    let v1 = json!({ "op": "asset.add", "id": "$a", "path": "/m.mov", "duration": "600",
                     "video": { "width": 1920, "height": 1080, "rate": "30" },
                     "fingerprint": "fp" });
    let Op::AssetAdd(a) = serde_json::from_value::<Op>(v1).unwrap() else {
        panic!()
    };
    assert_eq!(a.kind, AssetKind::Media);
    assert_eq!(serde_json::to_value(Op::from(*a)).unwrap()["kind"], "media");
}

#[test]
fn asset_in_use_names_items() {
    let p = with_clips(&[5]);
    let e = apply_internal(
        &p,
        &tx(vec![Op::AssetRemove { id: "a1".into() }]),
        &mut MemIds::after(&p),
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["items"]),
        ("ASSET_IN_USE", &json!(["c1"]))
    );
}

// ---- value shapes (review of Task 6) ----

#[test]
fn negative_zero_is_stored_as_zero_and_non_finite_is_refused() {
    let p = with_clips(&[5]);
    let mut item = Item::clip("c9", "a1", secs(5), secs(0), secs(1));
    item.common.transform.rotation = -0.0;
    item.common.opacity = -0.0;
    item.common.keyframes.insert(
        "opacity".into(),
        vec![Keyframe {
            at: secs(0),
            value: ParamValue::Vec2([-0.0, 1.0]),
            interp: Interp::Linear,
        }],
    );
    let restore = |item: Item| -> Op {
        ItemRestore {
            item,
            track: "t1".into(),
            mode: DeleteMode::Ripple,
        }
        .into()
    };
    let a = apply_internal(&p, &tx(vec![restore(item.clone())]), &mut MemIds::after(&p)).unwrap();
    let got = &a.project.timelines[0].tracks[0].items[1];
    assert_eq!(got.common.transform.rotation.to_bits(), 0.0f64.to_bits());
    assert_eq!(got.common.opacity.to_bits(), 0.0f64.to_bits());
    let ParamValue::Vec2([x, _]) = got.common.keyframes["opacity"][0].value else {
        panic!()
    };
    assert_eq!(x.to_bits(), 0.0f64.to_bits());
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut item = item.clone();
        item.common.transform.position = [0.0, bad];
        let e = apply_internal(&p, &tx(vec![restore(item)]), &mut MemIds::after(&p)).unwrap_err();
        assert_eq!(
            (e.code.as_str(), &e.details["path"]),
            ("INVALID_RANGE", &json!("transform.position[1]"))
        );
    }
    // Insert bodies too: a graphic's slots.
    let p = apply(&p, vec![asset_of(AssetKind::LottieTemplate, "$l").into()]).project;
    let body = |v: f64| ItemBody::Graphic {
        asset: "a2".into(),
        slots: [("size".to_string(), ParamValue::Number(v))].into(),
    };
    let a = apply(
        &p,
        vec![insert_body("$g", "t1", secs(5), secs(1), body(-0.0))],
    );
    assert_eq!(a.project.timelines[0].tracks[0].items[1].body, body(0.0));
    let ItemBody::Graphic { slots, .. } = &a.project.timelines[0].tracks[0].items[1].body else {
        panic!()
    };
    assert_eq!(slots["size"], ParamValue::Number(0.0));
    assert!(matches!(slots["size"], ParamValue::Number(v) if v.to_bits() == 0));
    let e = try_apply(
        &p,
        vec![insert_body("$g", "t1", secs(5), secs(1), body(f64::NAN))],
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
}
