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
    ItemInsert {
        id: id.into(),
        track: "t3".into(),
        at: Rational::zero(),
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
