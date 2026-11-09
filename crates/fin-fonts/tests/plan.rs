//! Pin planning (4C Decision 35): `plan_pins` over the built-in catalog.

use std::collections::BTreeMap;

use fin_core::{
    CaptionBlock, InsertMode, ItemBody, ItemInsert, MemIds, Op, ParamValue, Project, Rational,
    StyleAdd, Track, TrackKind, Word, apply_tx, new_project,
};
use fin_fonts::FontCatalog;
use fin_fonts::plan::plan_pins;
use serde_json::Value;

fn project() -> Project {
    let mut p = new_project("p1".into(), "Demo".into(), Rational::from_int(30)).unwrap();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    p
}

fn caption(id: &str, text: &str, style: &str) -> Op {
    caption_at(id, 0, text, style)
}

/// A 2 s caption at `at` s.
fn caption_at(id: &str, at: i64, text: &str, style: &str) -> Op {
    ItemInsert {
        id: id.into(),
        track: "t3".into(),
        at: Rational::from_int(at),
        duration: Rational::from_int(2),
        mode: InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: CaptionBlock {
                words: text
                    .split_whitespace()
                    .map(|t| Word {
                        text: t.into(),
                        start: Rational::zero(),
                        end: Rational::zero(),
                    })
                    .collect(),
                style: style.into(),
                overrides: BTreeMap::new(),
                source: None,
            },
        },
    }
    .into()
}

fn style(family: &str, weight: f64) -> Op {
    StyleAdd {
        id: "$st".into(),
        name: "Mine".into(),
        from: None,
        set: [
            ("text.family".to_string(), ParamValue::Text(family.into())),
            ("text.weight".to_string(), ParamValue::Number(weight)),
        ]
        .into(),
    }
    .into()
}

/// The planned pins as `(key, fallback)`.
fn pins(ops: &[Op]) -> Vec<(String, Option<u64>)> {
    ops.iter()
        .map(|op| {
            let v: Value = serde_json::to_value(op).unwrap();
            assert_eq!(v["op"], "font.pin", "{v}");
            (
                v["key"].as_str().unwrap().to_string(),
                v["fallback"].as_u64(),
            )
        })
        .collect()
}

fn commit(p: &Project, ops: Vec<Op>) -> Project {
    let tx = fin_core::Tx {
        tx_id: "t".into(),
        author: "a".into(),
        branch: "main".into(),
        base_rev: 0,
        schema_version: fin_core::SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    };
    apply_tx(p, &tx, &mut MemIds::after(p)).unwrap().project
}

#[test]
fn plan_pins_for_new_style_face() {
    let c = FontCatalog::builtin();
    let p = project();
    let ops = vec![style("Noto Sans", 700.0), caption("$c", "Hello", "$st")];
    let plan = plan_pins(&p, &ops, c).unwrap();
    assert_eq!(
        pins(&plan.ops),
        [("Noto Sans|700|normal".to_string(), None)]
    );
    assert!(plan.notes.is_empty());
    let v = serde_json::to_value(&plan.ops[0]).unwrap();
    assert_eq!(v["pin"]["source"]["catalog"], c.catalog_version.as_str());
    assert_eq!(v["pin"]["source"]["style"], "bold");
    // Prepended, the client's tx applies and the caption's faces resolve.
    let mut all = plan.ops.clone();
    all.extend(ops);
    let after = commit(&p, all);
    assert!(fin_core::layout::font_needs(&after).faces.len() == 1);
    // A style no caption uses needs nothing.
    let plan = plan_pins(&p, &[style("Noto Sans", 700.0)], c).unwrap();
    assert!(plan.ops.is_empty());
}

#[test]
fn plan_pins_fallback_for_new_script() {
    let c = FontCatalog::builtin();
    let p = project();
    // preset:bold is Inter 700 (Latin, Cyrillic, Greek): Hebrew and emoji need fallbacks.
    let ops = vec![caption("$c", "Hi שלום 😀", "preset:bold")];
    let plan = plan_pins(&p, &ops, c).unwrap();
    assert_eq!(
        pins(&plan.ops),
        [
            ("Inter|700|normal".to_string(), None),
            ("Noto Sans Hebrew|400|normal".to_string(), Some(0)),
            ("Noto Color Emoji|400|normal".to_string(), Some(1)),
        ]
    );
    let mut all = plan.ops.clone();
    all.extend(ops);
    let after = commit(&p, all);
    assert_eq!(
        after.fonts.fallback,
        ["Noto Sans Hebrew|400|normal", "Noto Color Emoji|400|normal"]
    );
    // Planning again on the result pins nothing more.
    let again = plan_pins(&after, &[], c).unwrap();
    assert!(again.ops.is_empty() && again.notes.is_empty());
}

#[test]
fn plan_pins_nothing_when_pinned() {
    let c = FontCatalog::builtin();
    let p = project();
    let first = vec![caption("$c", "Hello мир", "preset:bold")];
    let plan = plan_pins(&p, &first, c).unwrap();
    let mut all = plan.ops.clone();
    all.extend(first);
    let p = commit(&p, all);
    let plan = plan_pins(&p, &[caption("$d", "again", "preset:bold")], c);
    // Overwriting cap1 at 0 still needs only Inter, which is pinned.
    let plan = plan.unwrap();
    assert!(plan.ops.is_empty() && plan.notes.is_empty(), "{plan:?}");
}

#[test]
fn plan_never_pins_unknown_face() {
    let c = FontCatalog::builtin();
    let p = project();
    let ops = vec![style("Comic Sans", 400.0), caption("$c", "Hello", "$st")];
    let plan = plan_pins(&p, &ops, c).unwrap();
    assert_eq!(plan.notes.len(), 1);
    let n = &plan.notes[0];
    assert_eq!(n.code, "FONT_NOT_IN_CATALOG");
    assert_eq!(n.details["key"], "Comic Sans|400|normal");
    assert!(n.fix.is_some());
    // Its text asks for no fallback: the caption stays unmeasured without its face.
    assert!(plan.ops.is_empty(), "{:?}", plan.ops);
    // An unknown weight of a known family is not pinned either.
    let plan = plan_pins(&p, &[style("Inter", 300.0), caption("$c", "Hi", "$st")], c).unwrap();
    assert_eq!(plan.notes[0].details["key"], "Inter|300|normal");
    // A tx that cannot apply fails as it would at commit.
    let e = plan_pins(&p, &[caption("$c", "Hi", "st9")], c).unwrap_err();
    assert_eq!(e.code, "NOT_FOUND");
}

/// The plan's ops prepended to `ops`, committed on `p`.
fn planned(p: &Project, ops: Vec<Op>, c: &FontCatalog) -> (Vec<(String, Option<u64>)>, Project) {
    let plan = plan_pins(p, &ops, c).unwrap();
    assert!(plan.notes.is_empty(), "{:?}", plan.notes);
    let got = pins(&plan.ops);
    let mut all = plan.ops;
    all.extend(ops);
    (got, commit(p, all))
}

#[test]
fn plan_places_fallbacks_in_index_order() {
    let c = FontCatalog::builtin();
    let p = project();
    // cap1's style face is Noto Sans Hebrew 400; cap2 (Inter 700) needs Arabic, then Hebrew:
    // the Hebrew face pinned for cap1's style is placed after Noto Sans Arabic.
    let ops = vec![
        style("Noto Sans Hebrew", 400.0),
        caption_at("$a", 0, "שלום", "$st"),
        caption_at("$b", 3, "مرحبا שלום", "preset:bold"),
    ];
    let (got, after) = planned(&p, ops, c);
    assert_eq!(
        got,
        [
            ("Inter|700|normal".to_string(), None),
            ("Noto Sans Arabic|400|normal".to_string(), Some(0)),
            ("Noto Sans Hebrew|400|normal".to_string(), Some(1)),
        ]
    );
    assert_eq!(
        after.fonts.fallback,
        ["Noto Sans Arabic|400|normal", "Noto Sans Hebrew|400|normal"]
    );
    assert!(plan_pins(&after, &[], c).unwrap().ops.is_empty());
}

#[test]
fn plan_skips_a_placed_fallback_that_does_not_cover() {
    // Noto Sans placed for Latin (its pin covers Latin, Cyrillic, Greek).
    let builtin = FontCatalog::builtin();
    let (_, head) = planned(
        &project(),
        vec![
            style("Noto Sans Hebrew", 400.0),
            caption("$a", "Hello", "$st"),
        ],
        builtin,
    );
    assert_eq!(head.fonts.fallback, ["Noto Sans|400|normal"]);
    // A catalog that offers Noto Sans for Arabic too: its placed pin does not cover it, so
    // the next entry is placed.
    let mut c = builtin.clone();
    let ns = c.entries.iter_mut().find(|e| e.id == "noto-sans").unwrap();
    ns.scripts.push("Arab".into());
    let ops = vec![caption_at("$b", 3, "مرحبا", "preset:bold")];
    let (got, _) = planned(&head, ops.clone(), &c);
    assert_eq!(
        got,
        [
            ("Inter|700|normal".to_string(), None),
            ("Noto Sans Arabic|400|normal".to_string(), Some(1)),
        ]
    );
    // With no other entry for it: a note, never nothing.
    c.fallback.retain(|id| id != "noto-sans-arabic");
    let plan = plan_pins(&head, &ops, &c).unwrap();
    assert_eq!(pins(&plan.ops), [("Inter|700|normal".to_string(), None)]);
    assert_eq!(plan.notes.len(), 1);
    assert_eq!(
        (
            plan.notes[0].code.as_str(),
            &plan.notes[0].details["script"]
        ),
        ("FONT_NOT_IN_CATALOG", &"Arab".into())
    );
}

/// Plan 4C Task 26 (e): a caption import whose pins `fonts.plan` planned, for a style added in
/// an earlier transaction (so the style uses the face before the pin exists), commits and
/// undoes exactly through its stored inverse — the inverse `font.unpin`s skip the use check —
/// and the redo (the inverse's inverse) and a replay of the record give the import again.
#[test]
fn undo_of_an_import_with_planned_pins_is_exact() {
    let c = FontCatalog::builtin();
    let before = commit(&project(), vec![style("Noto Sans", 700.0)]);
    assert!(plan_pins(&before, &[], c).unwrap().ops.is_empty());
    let import = vec![caption("$c", "Hello שלום", "st1")];
    let plan = plan_pins(&before, &import, c).unwrap();
    assert_eq!(
        pins(&plan.ops),
        [
            ("Noto Sans|700|normal".to_string(), None),
            ("Noto Sans Hebrew|400|normal".to_string(), Some(0)),
        ]
    );
    let mut ops = plan.ops.clone();
    ops.extend(import);
    let tx = fin_core::Tx {
        tx_id: "import".into(),
        author: "a".into(),
        branch: "main".into(),
        base_rev: 1,
        schema_version: fin_core::SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    };
    let a = apply_tx(&before, &tx, &mut MemIds::after(&before)).unwrap();
    let internal = |ops: Vec<Op>| fin_core::Tx { ops, ..tx.clone() };
    let after = &a.project;
    let back = fin_core::apply_internal(
        after,
        &internal(a.inverse.clone()),
        &mut MemIds::after(after),
    )
    .unwrap();
    assert_eq!(back.project, before);
    let again = fin_core::apply_internal(
        &before,
        &internal(back.inverse),
        &mut MemIds::after(&before),
    )
    .unwrap();
    assert_eq!(&again.project, after);
    let rp = fin_core::apply_committed(&before, &tx, &a.bindings).unwrap();
    assert_eq!(&rp.project, after);
    assert_eq!(rp.inverse, a.inverse);
}
