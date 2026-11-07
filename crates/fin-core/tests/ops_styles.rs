//! Caption styles (plan 4C Task 18, Decision 28): `style.add|set|remove`. Presets are read
//! only; a project style is `st<n>`; a style a caption uses cannot go (`STYLE_IN_USE`).

mod common;

use std::collections::BTreeMap;

use common::ids_json::ids_cover_every_id_in_json;
use common::stale::{conflict_reason, rebase_over};
use common::*;
use fin_core::captions::preset;
use fin_core::rebase::{Touch, footprint, split_diff};
use fin_core::{
    Anchor, ApplyMode, CaptionBlock, CaptionStyle, ChangeKind, CoreError, EntityKind, InsertMode,
    ItemBody, ItemInsert, MemIds, Op, ParamValue, Project, StyleAdd, StyleRemove, StyleSet, Track,
    TrackKind, Word, apply_tx_mode,
};

fn num(x: f64) -> ParamValue {
    ParamValue::Number(x)
}

fn text(s: &str) -> ParamValue {
    ParamValue::Text(s.into())
}

fn add(id: &str, name: &str, from: Option<&str>, set: &[(&str, ParamValue)]) -> Op {
    StyleAdd {
        id: id.into(),
        name: name.into(),
        from: from.map(str::to_string),
        set: set
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
    }
    .into()
}

fn set(style: &str, path: &str, value: Option<ParamValue>) -> Op {
    StyleSet {
        style: style.into(),
        path: path.into(),
        value,
    }
    .into()
}

fn remove(style: &str) -> Op {
    StyleRemove {
        style: style.into(),
    }
    .into()
}

fn code<T: std::fmt::Debug>(r: Result<T, CoreError>) -> String {
    r.unwrap_err().code
}

fn replay(p: &Project, ops: Vec<Op>) -> Result<fin_core::Applied, CoreError> {
    apply_tx_mode(p, &tx(ops), &mut MemIds::after(p), ApplyMode::Replay)
}

fn fp(op: &Op, a: &fin_core::Applied) -> fin_core::rebase::Footprint {
    footprint(
        op,
        &split_diff(std::slice::from_ref(op), &a.diff).unwrap()[0],
    )
}

/// A project with caption track `t3` and project style `st1` ("Big", 80 px).
fn styled() -> Project {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    apply(
        &p,
        vec![add("$st", "Big", None, &[("text.size", num(80.0))])],
    )
    .project
}

fn caption_insert(style: &str) -> Op {
    ItemInsert {
        id: "$cap".into(),
        track: "t3".into(),
        at: secs(0),
        duration: secs(2),
        mode: InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: CaptionBlock {
                words: vec![Word {
                    text: "hi".into(),
                    start: secs(0),
                    end: secs(1),
                }],
                style: style.into(),
                overrides: BTreeMap::new(),
                source: None,
            },
        },
    }
    .into()
}

#[test]
fn style_add_golden() {
    let p = project();
    let op = add(
        "$st",
        "Loud",
        None,
        &[
            ("text.size", num(80.0)),
            ("color", text("#ff0000")),
            ("stroke.width", num(4.0)),
        ],
    );
    let a = golden_apply(&p, vec![op.clone()]);
    assert_eq!(a.bindings["$st"], "st1");
    let s = &a.project.styles["st1"];
    assert_eq!((s.id.as_str(), s.name.as_str()), ("st1", "Loud"));
    assert_eq!((s.text.size, s.color.as_str()), (80.0, "#ff0000"));
    assert_eq!(s.stroke.as_ref().unwrap().width, 4.0);
    assert_eq!(s.text.family, CaptionStyle::default().text.family);
    let c = &a.diff.changes[0];
    assert_eq!(
        (c.kind, c.id.as_str(), c.change),
        (EntityKind::Style, "st1", ChangeKind::Added)
    );
    assert_eq!(a.inverse, vec![remove("st1")]);
    assert!(fp(&op, &a).creates.contains(&"$st".to_string()));
    ids_cover_every_id_in_json(&op);
    // From a preset: its every field, under the new id and name.
    let a = apply(&p, vec![add("$st", "Mine", Some("preset:bold"), &[])]);
    let want = CaptionStyle {
        id: "st1".into(),
        name: "Mine".into(),
        ..preset("preset:bold").unwrap().clone()
    };
    assert_eq!(a.project.styles["st1"], want);
    // From a project style (used, so a stale record removing it conflicts).
    let p = styled();
    let op = add(
        "$st",
        "Copy",
        Some("st1"),
        &[("position.anchor", text("top"))],
    );
    let a = apply(&p, vec![op.clone()]);
    let s = &a.project.styles["st2"];
    assert_eq!((s.text.size, s.position.anchor), (80.0, Anchor::Top));
    assert!(fp(&op, &a).uses.contains(&"st1".to_string()));
    ids_cover_every_id_in_json(&op);
    // `from` must exist; a literal id is internal; a set path must be a style path.
    assert_eq!(
        code(try_apply(&p, vec![add("$s", "X", Some("st9"), &[])])),
        "NOT_FOUND"
    );
    assert_eq!(
        code(try_apply(
            &p,
            vec![add("$s", "X", Some("preset:nope"), &[])]
        )),
        "NOT_FOUND"
    );
    assert_eq!(
        code(try_apply(&p, vec![add("st5", "X", None, &[])])),
        "INVALID_OP"
    );
    assert_eq!(
        code(try_apply(
            &p,
            vec![add("$s", "X", None, &[("opacity", num(1.0))])]
        )),
        "PARAM_UNKNOWN"
    );
    assert_eq!(
        code(try_apply(
            &p,
            vec![add("$s", "X", None, &[("text.size", text("big"))])]
        )),
        "PARAM_TYPE"
    );
    assert_eq!(
        code(try_apply(&p, vec![add("$s", "", None, &[])])),
        "INVALID_RANGE"
    );
}

#[test]
fn style_set_golden() {
    let p = styled();
    let op = set("st1", "text.family", Some(text("Noto Sans")));
    let a = golden_apply(&p, vec![op.clone()]);
    assert_eq!(a.project.styles["st1"].text.family, "Noto Sans");
    assert_eq!(
        a.inverse,
        vec![set("st1", "text.family", Some(text("Inter")))]
    );
    let c = &a.diff.changes[0];
    assert_eq!(
        (c.kind, c.id.as_str(), c.path.as_deref()),
        (EntityKind::Style, "st1", Some("text.family"))
    );
    assert_eq!(
        fp(&op, &a).touches,
        [("st1".to_string(), Touch::Param("text.family".into()))]
    );
    ids_cover_every_id_in_json(&op);
    // An optional part appears with its first field and goes when back at its default.
    let a = golden_apply(&p, vec![set("st1", "stroke.width", Some(num(4.0)))]);
    assert!(a.project.styles["st1"].stroke.is_some());
    assert_eq!(a.inverse, vec![set("st1", "stroke.width", None)]);
    let a = golden_apply(&p, vec![set("st1", "highlight", Some(text("#ffd400")))]);
    assert_eq!(
        a.project.styles["st1"].highlight.as_deref(),
        Some("#ffd400")
    );
    // A reset takes the default; its inverse sets the old value.
    let a = golden_apply(&p, vec![set("st1", "text.size", None)]);
    assert_eq!(a.project.styles["st1"].text.size, 64.0);
    assert_eq!(a.inverse, vec![set("st1", "text.size", Some(num(80.0)))]);
    // Weights are whole; times canonical; enums named.
    let a = golden_apply(&p, vec![set("st1", "gap", Some(text("3/4")))]);
    assert_eq!(a.project.styles["st1"].gap, r("3/4"));
    assert_eq!(
        code(try_apply(
            &p,
            vec![set("st1", "text.weight", Some(num(450.5)))]
        )),
        "PARAM_TYPE"
    );
    assert_eq!(
        code(try_apply(
            &p,
            vec![set("st1", "safeZone", Some(text("myspace")))]
        )),
        "PARAM_TYPE"
    );
    // Presets are read only; unknown styles NOT_FOUND; range is policy, shape structural.
    assert_eq!(
        code(try_apply(
            &p,
            vec![set("preset:bold", "color", Some(text("#000000")))]
        )),
        "INVALID_OP"
    );
    assert_eq!(
        code(try_apply(&p, vec![set("st9", "color", None)])),
        "NOT_FOUND"
    );
    assert_eq!(
        code(try_apply(&p, vec![set("st1", "text.size", Some(num(0.0)))])),
        "INVALID_RANGE"
    );
    assert!(replay(&p, vec![set("st1", "text.size", Some(num(0.0)))]).is_ok());
    assert_eq!(
        code(replay(&p, vec![set("st1", "text.size", Some(text("x")))])),
        "PARAM_TYPE"
    );
    // -0.0 is stored as 0.0.
    let a = apply(&p, vec![set("st1", "text.letterSpacing", Some(num(-0.0)))]);
    assert_eq!(a.project.styles["st1"].text.letter_spacing.to_bits(), 0);
}

#[test]
fn style_remove_golden() {
    // A style with every optional part set comes back exactly.
    let p = styled();
    let p = apply(
        &p,
        vec![
            set("st1", "stroke.color", Some(text("#ff0000"))),
            set("st1", "background.padding", Some(num(12.0))),
            set("st1", "highlight", Some(text("#00ff00"))),
            set("st1", "maxDuration", Some(text("3"))),
            set("st1", "text.italic", Some(ParamValue::Bool(true))),
        ],
    )
    .project;
    let op = remove("st1");
    let a = golden_apply(&p, vec![op.clone()]);
    assert!(a.project.styles.is_empty());
    let Op::StyleAdd(back) = &a.inverse[0] else {
        panic!("{:?}", a.inverse);
    };
    assert_eq!(
        (back.id.as_str(), back.name.as_str(), back.from.as_deref()),
        ("st1", "Big", None)
    );
    ids_cover_every_id_in_json(&a.inverse[0]);
    let f = fp(&op, &a);
    assert!(f.removes.contains(&"st1".to_string()));
    assert_eq!(code(try_apply(&p, vec![remove("st9")])), "NOT_FOUND");
    assert_eq!(
        code(try_apply(&p, vec![remove("preset:bold")])),
        "INVALID_OP"
    );
}

#[test]
fn style_remove_in_use_refused() {
    let p = styled();
    let p = apply(&p, vec![caption_insert("st1")]).project;
    let e = try_apply(&p, vec![remove("st1")]).unwrap_err();
    assert_eq!(e.code, "STYLE_IN_USE");
    assert_eq!(e.details["items"], serde_json::json!(["cap1"]));
    assert!(e.fix.is_some());
    // Structural: replay refuses it too (nothing may name a missing style).
    assert_eq!(code(replay(&p, vec![remove("st1")])), "STYLE_IN_USE");
    // A caption naming a missing style is NOT_FOUND, in every mode.
    let q = styled();
    assert_eq!(
        code(try_apply(&q, vec![caption_insert("st9")])),
        "NOT_FOUND"
    );
    assert_eq!(
        code(replay(&q, vec![caption_insert("preset:nope")])),
        "NOT_FOUND"
    );
    // A style made earlier in the transaction can be used by its placeholder.
    let a = apply(&q, vec![add("$s", "New", None, &[]), caption_insert("$s")]);
    let item = &a.project.timelines[0].tracks[2].items[0];
    let ItemBody::Caption { block } = &item.body else {
        panic!()
    };
    assert_eq!(block.style, "st2");
}

#[test]
fn style_edits_conflict_and_merge() {
    let p = styled();
    // Two sets of one path: SAME_PARAM.
    let e = rebase_over(
        &p,
        vec![set("st1", "color", Some(text("#ff0000")))],
        vec![set("st1", "color", Some(text("#00ff00")))],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
    // Different paths merge.
    let ok = rebase_over(
        &p,
        vec![set("st1", "color", Some(text("#ff0000")))],
        vec![set("st1", "text.size", Some(num(90.0)))],
    );
    assert!(ok.is_ok(), "{ok:?}");
    // A set of a style removed meanwhile: REMOVED.
    let e = rebase_over(&p, vec![remove("st1")], vec![set("st1", "color", None)]).unwrap_err();
    assert_eq!(conflict_reason(&e), "REMOVED");
    // A caption inserted with a style removed meanwhile: REMOVED.
    let e = rebase_over(&p, vec![remove("st1")], vec![caption_insert("st1")]).unwrap_err();
    assert_eq!(conflict_reason(&e), "REMOVED");
}
