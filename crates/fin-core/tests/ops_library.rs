//! Markers, bins and assets (plan 4C Task 13): `marker.add|set|remove`, `bin.add|move|remove`,
//! `asset.move`, and `asset.remove` as a client op. Markers never ripple (Decision 26); an
//! asset an item, an effect or a font pin uses cannot go.

mod common;

use std::collections::BTreeMap;

use common::ids_json::ids_cover_every_id_in_json;
use common::stale::{conflict_reason, rebase_over};
use common::*;
use fin_core::rebase::{Touch, footprint, split_diff};
use fin_core::{
    ApplyMode, AssetAdd, AssetKind, AssetMove, BinAdd, BinMove, BinRemove, Change, ChangeKind,
    CoreError, Effect, EntityKind, FontFace, FontFile, FontFormat, FontPin, FontSource, ImageInfo,
    InsertMode, Item, ItemBody, ItemInsert, LottieInfo, LutInfo, MarkerAdd, MarkerRemove,
    MarkerSet, MemIds, Op, ParamValue, Project, Rational, apply_tx_mode,
};
use serde_json::json;

fn add_marker(at: Rational, label: &str) -> MarkerAdd {
    MarkerAdd {
        id: "$m".into(),
        timeline: "tl1".into(),
        at,
        duration: None,
        label: label.into(),
        color: None,
    }
}

fn set_marker(marker: &str, path: &str, value: Option<ParamValue>) -> Op {
    MarkerSet {
        marker: marker.into(),
        path: path.into(),
        value,
    }
    .into()
}

fn remove_marker(marker: &str) -> Op {
    MarkerRemove {
        marker: marker.into(),
    }
    .into()
}

fn add_bin(id: &str, name: &str, parent: Option<&str>) -> Op {
    BinAdd {
        id: id.into(),
        name: name.into(),
        parent: parent.map(str::to_string),
    }
    .into()
}

fn move_bin(bin: &str, parent: Option<&str>) -> Op {
    BinMove {
        bin: bin.into(),
        parent: parent.map(str::to_string),
    }
    .into()
}

fn remove_bin(bin: &str) -> Op {
    BinRemove { bin: bin.into() }.into()
}

fn move_asset(asset: &str, bin: Option<&str>) -> Op {
    AssetMove {
        asset: asset.into(),
        bin: bin.map(str::to_string),
    }
    .into()
}

fn remove_asset(id: &str) -> Op {
    Op::AssetRemove { id: id.into() }
}

fn text(s: &str) -> Option<ParamValue> {
    Some(ParamValue::Text(s.into()))
}

fn change(kind: EntityKind, id: &str, change: ChangeKind) -> Change {
    Change {
        kind,
        id: id.into(),
        change,
        op: 0,
        track: None,
        edge: None,
        path: None,
        by: None,
    }
}

fn param(kind: EntityKind, id: &str, path: &str) -> Change {
    Change {
        path: Some(path.into()),
        ..change(kind, id, ChangeKind::Updated)
    }
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

/// `(id, at)` of the markers of the main timeline, in order.
fn markers(p: &Project) -> Vec<(String, Rational)> {
    p.timelines[0]
        .markers
        .iter()
        .map(|m| (m.id.clone(), m.at))
        .collect()
}

fn at(id: &str, t: i64) -> (String, Rational) {
    (id.into(), secs(t))
}

/// An asset of `kind` with its own fields only.
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
                slots: Vec::new(),
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

#[test]
fn marker_add_keeps_markers_ordered() {
    let p = project();
    let mut first = add_marker(secs(5), "Intro");
    first.color = Some("#ff8800".into());
    first.duration = Some(r("1/2"));
    let a = golden_apply(&p, vec![first.into()]);
    assert_eq!(a.bindings["$m"], "m1");
    let m = &a.project.timelines[0].markers[0];
    assert_eq!(
        (m.label.as_str(), m.color.as_deref(), m.duration),
        ("Intro", Some("#ff8800"), Some(r("1/2")))
    );
    assert_eq!(
        a.diff.changes,
        [change(EntityKind::Marker, "m1", ChangeKind::Added)]
    );
    assert!(a.diff.shifts.is_empty());
    assert_eq!(a.inverse, [remove_marker("m1")]);
    let mut resolved = add_marker(secs(5), "Intro");
    resolved.id = "m1".into();
    let f = fp(&resolved.into(), &a);
    assert_eq!((f.creates, f.uses), (vec!["m1".into()], vec!["tl1".into()]));
    // Ordered by `at`, then id.
    let ops = vec![
        add_marker(secs(5), "a").into(),
        MarkerAdd {
            id: "$n".into(),
            ..add_marker(secs(2), "b")
        }
        .into(),
        MarkerAdd {
            id: "$o".into(),
            ..add_marker(secs(5), "c")
        }
        .into(),
    ];
    let b = golden_apply(&p, ops);
    assert_eq!(markers(&b.project), [at("m2", 2), at("m1", 5), at("m3", 5)]);
}

#[test]
fn marker_add_refusals() {
    let p = project();
    // A negative time and a colour of another form are structural.
    for (op, want) in [
        (add_marker(secs(-1), "x"), "INVALID_RANGE"),
        (
            MarkerAdd {
                color: Some("#FFF".into()),
                ..add_marker(secs(1), "x")
            },
            "PARAM_TYPE",
        ),
        (
            MarkerAdd {
                duration: Some(secs(-2)),
                ..add_marker(secs(1), "x")
            },
            "INVALID_RANGE",
        ),
    ] {
        assert_eq!(code(try_apply(&p, vec![op.clone().into()])), want);
        assert_eq!(code(replay(&p, vec![op.into()])), want);
    }
    // A label's length is policy.
    let long = add_marker(secs(1), &"x".repeat(257));
    assert_eq!(
        code(try_apply(&p, vec![long.clone().into()])),
        "INVALID_RANGE"
    );
    replay(&p, vec![long.into()]).unwrap();
    // Clients name new markers with placeholders.
    let literal = MarkerAdd {
        id: "m7".into(),
        ..add_marker(secs(1), "x")
    };
    assert_eq!(code(try_apply(&p, vec![literal.into()])), "INVALID_OP");
    let nowhere = MarkerAdd {
        timeline: "tl9".into(),
        ..add_marker(secs(1), "x")
    };
    assert_eq!(code(try_apply(&p, vec![nowhere.into()])), "NOT_FOUND");
}

#[test]
fn marker_set_sets_each_field() {
    let p = apply(
        &project(),
        vec![
            add_marker(secs(5), "Intro").into(),
            MarkerAdd {
                id: "$n".into(),
                ..add_marker(secs(8), "Outro")
            }
            .into(),
        ],
    )
    .project;
    for (path, value, old) in [
        ("label", text("Start"), text("Intro")),
        ("color", text("#00ff00aa"), None),
        ("at", text("9"), text("5")),
        ("duration", text("3/2"), None),
    ] {
        let a = golden_apply(&p, vec![set_marker("m1", path, value)]);
        assert_eq!(a.diff.changes, [param(EntityKind::Marker, "m1", path)]);
        assert_eq!(a.inverse, [set_marker("m1", path, old)]);
        let f = fp(&set_marker("m1", path, None), &a);
        assert_eq!(f.touches, [("m1".to_string(), Touch::Param(path.into()))]);
    }
    // Setting `at` reorders.
    let moved = apply(&p, vec![set_marker("m1", "at", text("9"))]).project;
    assert_eq!(markers(&moved), [at("m2", 8), at("m1", 9)]);
    // A colour or duration clears; a label or position cannot be reset.
    let coloured = apply(&p, vec![set_marker("m1", "color", text("#123456"))]).project;
    let a = golden_apply(&coloured, vec![set_marker("m1", "color", None)]);
    assert_eq!(a.project.timelines[0].markers[0].color, None);
    for path in ["label", "at"] {
        assert_eq!(
            code(try_apply(&p, vec![set_marker("m1", path, None)])),
            "INVALID_OP"
        );
    }
    for (op, want) in [
        (set_marker("m1", "at", text("-1")), "INVALID_RANGE"),
        (
            set_marker("m1", "at", Some(ParamValue::Number(1.0))),
            "PARAM_TYPE",
        ),
        (set_marker("m1", "name", text("x")), "PARAM_UNKNOWN"),
        (set_marker("m9", "label", text("x")), "NOT_FOUND"),
    ] {
        assert_eq!(code(replay(&p, vec![op])), want);
    }
}

#[test]
fn marker_remove_and_undo_put_it_back_in_place() {
    let p = apply(
        &project(),
        vec![
            add_marker(secs(5), "a").into(),
            MarkerAdd {
                id: "$n".into(),
                color: Some("#abcdef".into()),
                duration: Some(secs(2)),
                ..add_marker(secs(5), "b")
            }
            .into(),
            MarkerAdd {
                id: "$o".into(),
                ..add_marker(secs(5), "c")
            }
            .into(),
        ],
    )
    .project;
    let a = golden_apply(&p, vec![remove_marker("m2")]);
    assert_eq!(markers(&a.project), [at("m1", 5), at("m3", 5)]);
    assert_eq!(
        a.diff.changes,
        [change(EntityKind::Marker, "m2", ChangeKind::Removed)]
    );
    assert_eq!(
        a.inverse,
        [Op::from(MarkerAdd {
            id: "m2".into(),
            color: Some("#abcdef".into()),
            duration: Some(secs(2)),
            ..add_marker(secs(5), "b")
        })]
    );
    let f = fp(&remove_marker("m2"), &a);
    assert_eq!(f.removes, ["m2"]);
    assert_eq!(code(try_apply(&p, vec![remove_marker("m9")])), "NOT_FOUND");
}

#[test]
fn marker_positions_never_map() {
    // c1 [0,4), c2 [4,8); a marker at 6, inside c2.
    let p = apply(
        &with_clips(&[4, 4]),
        vec![add_marker(secs(6), "Cue").into()],
    )
    .project;
    let ripple = insert("$n", "t1", secs(0), "a1", secs(0), secs(2));
    // A ripple before it leaves it.
    let a = apply(&p, vec![ripple.clone()]);
    assert_eq!(markers(&a.project), [at("m1", 6)]);
    let a = apply(&p, vec![delete("c1")]);
    assert_eq!(markers(&a.project), [at("m1", 6)]);
    // A stale marker edit over a ripple keeps its time.
    let base = with_clips(&[4, 4]);
    let r = rebase_over(
        &base,
        vec![ripple.clone()],
        vec![add_marker(secs(6), "Cue").into()],
    )
    .unwrap();
    assert_eq!(markers(&r.applied.project), [at("m1", 6)]);
    let r = rebase_over(&p, vec![ripple], vec![set_marker("m1", "at", text("7"))]).unwrap();
    assert_eq!(markers(&r.applied.project), [at("m1", 7)]);
}

#[test]
fn stale_marker_edits_conflict() {
    let p = apply(&project(), vec![add_marker(secs(5), "Intro").into()]).project;
    let label = |s: &str| set_marker("m1", "label", text(s));
    let e = rebase_over(&p, vec![label("A")], vec![label("B")]).unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
    rebase_over(
        &p,
        vec![label("A")],
        vec![set_marker("m1", "at", text("6"))],
    )
    .unwrap();
    let e = rebase_over(&p, vec![remove_marker("m1")], vec![label("B")]).unwrap_err();
    assert_eq!(conflict_reason(&e), "REMOVED");
}

#[test]
fn bins_nest_move_and_remove() {
    let p = project();
    let a = golden_apply(&p, vec![add_bin("$b", "Footage", None)]);
    assert_eq!(a.bindings["$b"], "b1");
    assert_eq!(
        a.diff.changes,
        [change(EntityKind::Bin, "b1", ChangeKind::Added)]
    );
    assert_eq!(a.inverse, [remove_bin("b1")]);
    let p = apply(
        &p,
        vec![
            add_bin("$b", "Footage", None),
            add_bin("$c", "Day 1", Some("$b")),
        ],
    )
    .project;
    assert_eq!(p.bins["b2"].parent.as_deref(), Some("b1"));
    // Move to the top and back.
    let a = golden_apply(&p, vec![move_bin("b2", None)]);
    assert_eq!(a.project.bins["b2"].parent, None);
    assert_eq!(a.diff.changes, [param(EntityKind::Bin, "b2", "parent")]);
    assert_eq!(a.inverse, [move_bin("b2", Some("b1"))]);
    let f = fp(&move_bin("b2", None), &a);
    assert_eq!(
        f.touches,
        [("b2".to_string(), Touch::Param("parent".into()))]
    );
    let f = fp(
        &move_bin("b2", Some("b1")),
        &golden_apply(&a.project, vec![move_bin("b2", Some("b1"))]),
    );
    assert_eq!(f.uses, ["b1"]);
    // A bin holding bins or assets stays.
    let e = try_apply(&p, vec![remove_bin("b1")]).unwrap_err();
    assert_eq!(e.code, "BIN_NOT_EMPTY");
    assert_eq!(e.details["bins"], json!(["b2"]));
    let mut add = asset_of(AssetKind::Image, "$i");
    add.bin = Some("b2".into());
    let filled = apply(&p, vec![add.into()]).project;
    let e = replay(&filled, vec![remove_bin("b2")]).unwrap_err();
    assert_eq!(e.code, "BIN_NOT_EMPTY");
    assert_eq!(e.details["assets"], json!(["a1"]));
    // An empty one goes, and comes back with its literal id.
    let a = golden_apply(&p, vec![remove_bin("b2")]);
    assert_eq!(
        a.diff.changes,
        [change(EntityKind::Bin, "b2", ChangeKind::Removed)]
    );
    assert_eq!(a.inverse, [add_bin("b2", "Day 1", Some("b1"))]);
    assert_eq!(fp(&remove_bin("b2"), &a).removes, ["b2"]);
    // Literal ids are internal; parents must exist.
    assert_eq!(
        code(try_apply(&p, vec![add_bin("b7", "x", None)])),
        "INVALID_OP"
    );
    assert_eq!(
        code(try_apply(&p, vec![add_bin("$x", "x", Some("b9"))])),
        "NOT_FOUND"
    );
    internal(&a.project, a.inverse.clone()).unwrap();
}

#[test]
fn bin_cycle_refused() {
    let p = apply(
        &project(),
        vec![
            add_bin("$a", "A", None),
            add_bin("$b", "B", Some("$a")),
            add_bin("$c", "C", Some("$b")),
        ],
    )
    .project;
    for (bin, parent) in [("b1", "b1"), ("b1", "b2"), ("b1", "b3"), ("b2", "b3")] {
        for r in [
            try_apply(&p, vec![move_bin(bin, Some(parent))]),
            replay(&p, vec![move_bin(bin, Some(parent))]),
        ] {
            let e = r.unwrap_err();
            assert_eq!(e.code, "BIN_CYCLE", "{bin} into {parent}");
            assert_eq!(e.details["bin"], bin);
        }
    }
    // Sideways and up are fine.
    golden_apply(&p, vec![move_bin("b3", Some("b1"))]);
    golden_apply(&p, vec![move_bin("b2", None)]);
}

#[test]
fn asset_move_files_an_asset() {
    let p = apply(&with_clips(&[4]), vec![add_bin("$b", "Footage", None)]).project;
    let a = golden_apply(&p, vec![move_asset("a1", Some("b1"))]);
    assert_eq!(a.project.assets["a1"].bin.as_deref(), Some("b1"));
    assert_eq!(a.diff.changes, [param(EntityKind::Asset, "a1", "bin")]);
    assert_eq!(a.inverse, [move_asset("a1", None)]);
    let f = fp(&move_asset("a1", Some("b1")), &a);
    assert_eq!(f.touches, [("a1".to_string(), Touch::Param("bin".into()))]);
    assert_eq!(f.uses, ["b1"]);
    assert_eq!(
        code(try_apply(&p, vec![move_asset("a1", Some("b9"))])),
        "NOT_FOUND"
    );
    assert_eq!(
        code(try_apply(&p, vec![move_asset("a9", None)])),
        "NOT_FOUND"
    );
    // A stale move into a bin removed meanwhile.
    let e = rebase_over(
        &p,
        vec![remove_bin("b1")],
        vec![move_asset("a1", Some("b1"))],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "REMOVED");
}

fn pin(asset: &str) -> FontPin {
    FontPin {
        family: "Test Sans".into(),
        weight: 400,
        italic: false,
        face: 0,
        sha256: "ab".repeat(32),
        size: 1234,
        format: FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: FontSource::Asset {
            asset: asset.into(),
        },
    }
}

#[test]
fn asset_remove_in_use_lists_items() {
    // a1 media (clip c1), a2 image (image item img1), a3 lottie (graphic g1), a4 LUT (a lut
    // effect of c1), a5 font (a pin), a6 unused.
    let mut p = with_clips(&[4]);
    let mut ops: Vec<Op> = [
        AssetKind::Image,
        AssetKind::LottieTemplate,
        AssetKind::Lut,
        AssetKind::Font,
        AssetKind::Image,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, k)| asset_of(k, &format!("$x{i}")).into())
    .collect();
    for (id, body) in [
        (
            "$img",
            ItemBody::Image {
                asset: "$x0".into(),
            },
        ),
        (
            "$g",
            ItemBody::Graphic {
                asset: "$x1".into(),
                slots: BTreeMap::new(),
            },
        ),
    ] {
        ops.push(
            ItemInsert {
                id: id.into(),
                track: "t1".into(),
                at: secs(4),
                duration: secs(1),
                mode: InsertMode::Ripple,
                body,
            }
            .into(),
        );
    }
    p = apply(&p, ops).project;
    let c1 = &mut p.timelines[0].tracks[0].items[0];
    assert_eq!(c1.id, "c1");
    c1.common.effects.push(Effect {
        id: "fx1".into(),
        kind: "lut".into(),
        enabled: true,
        params: [("asset".to_string(), ParamValue::Text("a4".into()))].into(),
    });
    p.fonts
        .pins
        .insert("Test Sans|400|normal".into(), pin("a5"));
    for (asset, items, pins) in [
        ("a1", json!(["c1"]), json!([])),
        ("a2", json!(["img1"]), json!([])),
        ("a3", json!(["g1"]), json!([])),
        ("a4", json!(["c1"]), json!([])),
        ("a5", json!([]), json!(["Test Sans|400|normal"])),
    ] {
        for r in [
            try_apply(&p, vec![remove_asset(asset)]),
            replay(&p, vec![remove_asset(asset)]),
        ] {
            let e = r.unwrap_err();
            assert_eq!(e.code, "ASSET_IN_USE", "{asset}");
            assert_eq!(e.details["items"], items, "{asset}");
            assert_eq!(e.details["pins"], pins, "{asset}");
        }
    }
    // Owner: a font asset a pin uses cannot go; the fix says to unpin it.
    let e = try_apply(&p, vec![remove_asset("a5")]).unwrap_err();
    assert!(e.fix.unwrap().contains("unpin"));
    // The unused one goes, from a client.
    let a = golden_apply(&p, vec![remove_asset("a6")]);
    assert!(!a.project.assets.contains_key("a6"));
    assert_eq!(
        a.diff.changes,
        [change(EntityKind::Asset, "a6", ChangeKind::Removed)]
    );
}

#[test]
fn asset_remove_then_undo_restores_kind_fields() {
    let p = apply(&project(), vec![add_bin("$b", "Library", None)]).project;
    for kind in [
        AssetKind::Media,
        AssetKind::Image,
        AssetKind::Svg,
        AssetKind::LottieTemplate,
        AssetKind::Font,
        AssetKind::Lut,
    ] {
        let mut add = asset_of(kind, "$a");
        add.bin = Some("b1".into());
        let with = apply(&p, vec![add.into()]).project;
        let a = golden_apply(&with, vec![remove_asset("a1")]);
        let mut back = asset_of(kind, "a1");
        back.bin = Some("b1".into());
        assert_eq!(a.inverse, [Op::from(back)], "{kind:?}");
        // The undo brings it back with every field; then its bin cannot go.
        let undone = internal(&a.project, a.inverse.clone()).unwrap().project;
        assert_eq!(undone, with, "{kind:?}");
        assert_eq!(
            code(try_apply(&undone, vec![remove_bin("b1")])),
            "BIN_NOT_EMPTY"
        );
    }
}

#[test]
fn stale_asset_edits_conflict() {
    let p = with_clips(&[4]);
    let spare = apply(&p, vec![asset_of(AssetKind::Image, "$i").into()]).project;
    let use_it: Op = ItemInsert {
        id: "$n".into(),
        track: "t1".into(),
        at: secs(4),
        duration: secs(1),
        mode: InsertMode::Ripple,
        body: ItemBody::Image { asset: "a2".into() },
    }
    .into();
    // A stale use of an asset removed meanwhile.
    let e = rebase_over(&spare, vec![remove_asset("a2")], vec![use_it.clone()]).unwrap_err();
    assert_eq!(conflict_reason(&e), "REMOVED");
    // A stale removal of an asset used meanwhile fails at the head.
    let e = rebase_over(&spare, vec![use_it], vec![remove_asset("a2")]).unwrap_err();
    assert_eq!(conflict_reason(&e), "ASSET_IN_USE");
    // A move and a removal of one asset.
    let filed = apply(&spare, vec![add_bin("$b", "x", None)]).project;
    let e = rebase_over(
        &filed,
        vec![remove_asset("a2")],
        vec![move_asset("a2", Some("b1"))],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "REMOVED");
}

#[test]
fn ops_carry_their_ids() {
    let ops: Vec<Op> = vec![
        MarkerAdd {
            color: Some("#ff0000".into()),
            ..add_marker(secs(1), "Cue")
        }
        .into(),
        set_marker("m1", "label", text("Cue")),
        remove_marker("m1"),
        add_bin("$b", "Footage", Some("b1")),
        move_bin("b2", Some("b1")),
        remove_bin("b1"),
        move_asset("a1", Some("b1")),
        remove_asset("a1"),
    ];
    for op in &ops {
        ids_cover_every_id_in_json(op);
        assert!(!op.is_internal(), "{}", op.name());
    }
    assert_eq!(ops[0].ids(), ["$m", "tl1"]);
    assert_eq!(ops[0].allocates(), Some("$m"));
    assert_eq!(ops[3].ids(), ["$b", "b1"]);
    assert_eq!(ops[3].allocates(), Some("$b"));
    assert_eq!(ops[6].ids(), ["a1", "b1"]);
    let _ = Item::new("x", secs(0), secs(1), ItemBody::Adjustment {});
}
