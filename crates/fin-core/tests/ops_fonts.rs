//! Font pins (plan 4C Task 18, Decision 29): `font.pin|unpin|repin`. Pins are plain data:
//! applying one never fetches and never reads font bytes (there is no provider to give it).

mod common;

use std::collections::BTreeMap;

use common::ids_json::ids_cover_every_id_in_json;
use common::stale::{committed, conflict_reason, rebase_over};
use common::*;
use fin_core::rebase::{Touch, footprint, split_diff};
use fin_core::{
    ApplyMode, AssetAdd, AssetKind, CaptionBlock, ChangeKind, CoreError, EntityKind, FontFace,
    FontFile, FontFormat, FontPin, FontPinOp, FontRepin, FontSource, FontUnpin, InsertMode,
    ItemBody, ItemInsert, MemIds, Op, ParamValue, Project, StyleAdd, Track, TrackKind, Word,
    apply_tx_mode, font_key, rebase_tx,
};

const INTER: &str = "Inter|700|normal";

fn sha(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn catalog_pin(family: &str, weight: u16, sha256: &str, scripts: &[&str]) -> FontPin {
    FontPin {
        family: family.into(),
        weight,
        italic: false,
        face: 0,
        sha256: sha256.into(),
        size: 1000,
        format: FontFormat::Ttf,
        scripts: scripts.iter().map(|s| (*s).to_string()).collect(),
        source: FontSource::Catalog {
            entry: "inter".into(),
            style: "bold".into(),
            catalog: "2026.11.1".into(),
        },
    }
}

fn inter(sha256: &str) -> FontPin {
    catalog_pin("Inter", 700, sha256, &["Latn", "Cyrl", "Grek"])
}

fn pin(key: &str, p: FontPin, fallback: Option<u32>) -> Op {
    FontPinOp {
        key: key.into(),
        pin: p,
        fallback,
    }
    .into()
}

fn unpin(key: &str) -> Op {
    FontUnpin { key: key.into() }.into()
}

fn repin(key: &str, p: FontPin) -> Op {
    FontRepin {
        key: key.into(),
        pin: p,
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

fn font_asset() -> Op {
    let mut a = AssetAdd::media("$f", "/fonts/test.ttf", secs(1), None, None, "fp");
    a.kind = AssetKind::Font;
    a.duration = None;
    a.font = Some(FontFile {
        sha256: sha('c'),
        size: 1234,
        format: FontFormat::Ttf,
        faces: vec![FontFace {
            index: 0,
            family: "Test Sans".into(),
            weight: 400,
            italic: false,
            scripts: vec!["Latn".into()],
        }],
    });
    a.into()
}

fn asset_pin(asset: &str) -> FontPin {
    FontPin {
        family: "Test Sans".into(),
        weight: 400,
        italic: false,
        face: 0,
        sha256: sha('c'),
        size: 1234,
        format: FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: FontSource::Asset {
            asset: asset.into(),
        },
    }
}

fn caption(style: &str, text: &str) -> Op {
    ItemInsert {
        id: "$cap".into(),
        track: "t3".into(),
        at: secs(0),
        duration: secs(2),
        mode: InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: CaptionBlock {
                words: vec![Word {
                    text: text.into(),
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

fn with_caption_track() -> Project {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    p
}

#[test]
fn pin_golden() {
    let p = project();
    let op = pin(INTER, inter(&sha('a')), None);
    let a = golden_apply(&p, vec![op.clone()]);
    assert_eq!(a.project.fonts.pins[INTER], inter(&sha('a')));
    assert!(a.project.fonts.fallback.is_empty());
    let c = &a.diff.changes[0];
    assert_eq!(
        (c.kind, c.id.as_str(), c.change),
        (EntityKind::Font, "font/Inter|700|normal", ChangeKind::Added)
    );
    assert_eq!(a.inverse, vec![unpin(INTER)]);
    assert_eq!(fp(&op, &a).creates, ["font/Inter|700|normal"]);
    ids_cover_every_id_in_json(&op);
    // Into the fallback order at an index; unpinning puts it back there.
    let noto = catalog_pin("Noto Sans", 400, &sha('b'), &["Latn"]);
    let nkey = font_key("Noto Sans", 400, false);
    let p = apply(&p, vec![op, pin(&nkey, noto.clone(), Some(0))]).project;
    let arabic = catalog_pin("Noto Sans Arabic", 400, &sha('d'), &["Arab"]);
    let akey = font_key("Noto Sans Arabic", 400, false);
    let a = golden_apply(&p, vec![pin(&akey, arabic.clone(), Some(0))]);
    assert_eq!(a.project.fonts.fallback, [akey.clone(), nkey.clone()]);
    let q = a.project;
    let a = golden_apply(&q, vec![unpin(&nkey)]);
    assert_eq!(a.inverse, vec![pin(&nkey, noto, Some(1))]);
    assert_eq!(a.project.fonts.fallback, std::slice::from_ref(&akey));
    let f = fp(&unpin(&nkey), &a);
    assert!(f.removes.contains(&format!("font/{nkey}")));
    // A client index past the end is INVALID_RANGE.
    assert_eq!(
        code(try_apply(
            &q,
            vec![pin(
                "X|400|normal",
                catalog_pin("X", 400, &sha('e'), &[]),
                Some(5)
            )]
        )),
        "INVALID_RANGE"
    );
}

#[test]
fn pin_is_checked() {
    let p = project();
    // The key names the pin's face.
    assert_eq!(
        code(try_apply(
            &p,
            vec![pin("Inter|400|normal", inter(&sha('a')), None)]
        )),
        "INVALID_OP"
    );
    // A SHA-256 is 64 lowercase hex digits; a size is at least a byte.
    assert_eq!(
        code(try_apply(&p, vec![pin(INTER, inter("ABC"), None)])),
        "INVALID_OP"
    );
    let mut bad = inter(&sha('a'));
    bad.size = 0;
    assert_eq!(code(replay(&p, vec![pin(INTER, bad, None)])), "INVALID_OP");
    // An asset pin names a font asset holding that file and face.
    let p = apply(&p, vec![font_asset()]).project;
    assert_eq!(
        code(try_apply(
            &p,
            vec![pin("Test Sans|400|normal", asset_pin("a9"), None)]
        )),
        "NOT_FOUND"
    );
    let mut wrong = asset_pin("a1");
    wrong.sha256 = sha('d');
    assert_eq!(
        code(try_apply(
            &p,
            vec![pin("Test Sans|400|normal", wrong, None)]
        )),
        "INVALID_OP"
    );
    let mut face = asset_pin("a1");
    face.face = 3;
    assert_eq!(
        code(try_apply(&p, vec![pin("Test Sans|400|normal", face, None)])),
        "INVALID_OP"
    );
}

#[test]
fn asset_pin_golden_and_ids() {
    let p = apply(&project(), vec![font_asset()]).project;
    let key = font_key("Test Sans", 400, false);
    let op = pin(&key, asset_pin("a1"), None);
    ids_cover_every_id_in_json(&op);
    assert!(op.ids().contains(&"a1"));
    let a = golden_apply(&p, vec![op.clone()]);
    assert!(fp(&op, &a).uses.contains(&"a1".to_string()));
    // The pinned asset is in use (Task 13's check).
    let e = try_apply(&a.project, vec![Op::AssetRemove { id: "a1".into() }]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["pins"]),
        ("ASSET_IN_USE", &serde_json::json!([key]))
    );
    // A placeholder asset made earlier in the transaction.
    let a = apply(
        &project(),
        vec![font_asset(), pin(&key, asset_pin("$f"), None)],
    );
    assert_eq!(
        a.project.fonts.pins[&key].source,
        FontSource::Asset { asset: "a1".into() }
    );
}

#[test]
fn same_hash_pin_is_a_noop_and_merges() {
    let p = apply(&project(), vec![pin(INTER, inter(&sha('a')), None)]).project;
    let a = apply(&p, vec![pin(INTER, inter(&sha('a')), Some(0))]);
    assert_eq!(a.project, p);
    assert!(a.diff.changes.is_empty() && a.diff.shifts.is_empty() && a.inverse.is_empty());
    // Two edits that pin the same face merge cleanly.
    let base = project();
    let r = rebase_over(
        &base,
        vec![pin(INTER, inter(&sha('a')), None)],
        vec![pin(INTER, inter(&sha('a')), None)],
    )
    .unwrap();
    assert_eq!(r.applied.project.fonts.pins.len(), 1);
    assert!(r.applied.diff.changes.is_empty());
}

#[test]
fn other_hash_pin_is_font_pinned() {
    let p = apply(&project(), vec![pin(INTER, inter(&sha('a')), None)]).project;
    let e = try_apply(&p, vec![pin(INTER, inter(&sha('b')), None)]).unwrap_err();
    assert_eq!(e.code, "FONT_PINNED");
    assert_eq!(e.details["key"], INTER);
    assert!(e.fix.as_deref().unwrap().contains("repin"));
    // Structural on replay: valid history cannot produce it.
    assert_eq!(
        code(replay(&p, vec![pin(INTER, inter(&sha('b')), None)])),
        "FONT_PINNED"
    );
    // Stale: theirs pinned another file; ours re-runs and fails with that code.
    let e = rebase_over(
        &project(),
        vec![pin(INTER, inter(&sha('a')), None)],
        vec![pin(INTER, inter(&sha('b')), None)],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "FONT_PINNED");
}

#[test]
fn unpin_in_use_refused() {
    let p = with_caption_track();
    let noto = font_key("Noto Sans", 400, false);
    let arabic = font_key("Noto Sans Arabic", 400, false);
    let p = apply(
        &p,
        vec![
            pin(INTER, inter(&sha('a')), None),
            pin(
                &noto,
                catalog_pin("Noto Sans", 400, &sha('b'), &["Latn"]),
                Some(0),
            ),
            pin(
                &arabic,
                catalog_pin("Noto Sans Arabic", 400, &sha('d'), &["Arab"]),
                Some(1),
            ),
        ],
    )
    .project;
    // Nothing uses them yet.
    for k in [INTER, &noto, &arabic] {
        assert!(try_apply(&p, vec![unpin(k)]).is_ok(), "{k}");
    }
    // A project style resolving to the key uses it.
    let add = StyleAdd {
        id: "$st".into(),
        name: "S".into(),
        from: None,
        set: BTreeMap::new(),
    };
    let q = apply(&p, vec![add.into()]).project;
    let e = try_apply(&q, vec![unpin(INTER)]).unwrap_err();
    assert_eq!(e.code, "FONT_IN_USE");
    assert_eq!(e.details["styles"], serde_json::json!(["st1"]));
    assert!(e.fix.is_some());
    // A caption: its style's face, and a fallback covering its text's scripts.
    let q = apply(&p, vec![caption("preset:bold", "مرحبا")]).project;
    let e = try_apply(&q, vec![unpin(&arabic)]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["items"]),
        ("FONT_IN_USE", &serde_json::json!(["cap1"]))
    );
    assert!(
        try_apply(&q, vec![unpin(&noto)]).is_ok(),
        "no Latin in the caption"
    );
    assert_eq!(
        code(try_apply(&q, vec![unpin("Nope|400|normal")])),
        "NOT_FOUND"
    );
    // An override of the face counts.
    let mut o = BTreeMap::new();
    o.insert(
        "text.family".to_string(),
        ParamValue::Text("Noto Sans".into()),
    );
    o.insert("text.weight".to_string(), ParamValue::Number(400.0));
    let mut ins = caption("preset:bold", "hi");
    if let Op::ItemInsert(i) = &mut ins
        && let ItemBody::Caption { block } = &mut i.body
    {
        block.overrides = o;
    }
    let q = apply(&p, vec![ins]).project;
    assert_eq!(code(try_apply(&q, vec![unpin(&noto)])), "FONT_IN_USE");
}

#[test]
fn repin_inverse_exact() {
    let p = apply(&project(), vec![pin(INTER, inter(&sha('a')), Some(0))]).project;
    let mut newer = inter(&sha('b'));
    newer.source = FontSource::Catalog {
        entry: "inter".into(),
        style: "bold".into(),
        catalog: "2027.1.1".into(),
    };
    let op = repin(INTER, newer.clone());
    let a = golden_apply(&p, vec![op.clone()]);
    assert_eq!(a.project.fonts.pins[INTER], newer);
    assert_eq!(a.project.fonts.fallback, [INTER]);
    assert_eq!(a.inverse, vec![repin(INTER, inter(&sha('a')))]);
    let c = &a.diff.changes[0];
    assert_eq!(
        (c.kind, c.id.as_str(), c.change, c.path.as_deref()),
        (
            EntityKind::Font,
            "font/Inter|700|normal",
            ChangeKind::Updated,
            Some("sha256")
        )
    );
    assert_eq!(
        fp(&op, &a).touches,
        [(
            "font/Inter|700|normal".to_string(),
            Touch::Param("sha256".into())
        )]
    );
    ids_cover_every_id_in_json(&op);
    // Two repins of one key: SAME_PARAM. A repin of a key unpinned meanwhile: REMOVED.
    let e = rebase_over(
        &p,
        vec![repin(INTER, newer.clone())],
        vec![repin(INTER, inter(&sha('c')))],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
    let e = rebase_over(&p, vec![unpin(INTER)], vec![repin(INTER, newer.clone())]).unwrap_err();
    assert_eq!(conflict_reason(&e), "REMOVED");
    // The face must stay the key's; the key must be pinned.
    assert_eq!(
        code(try_apply(
            &p,
            vec![repin(INTER, catalog_pin("Inter", 400, &sha('b'), &[]))]
        )),
        "INVALID_OP"
    );
    assert_eq!(
        code(try_apply(&project(), vec![repin(INTER, newer)])),
        "NOT_FOUND"
    );
}

#[test]
fn font_pin_never_reads_bytes() {
    // No measurer, catalog or byte provider exists in fin-core: pins apply, replay and rebase
    // as data, for files no machine holds.
    let p = project();
    let ops = vec![pin(INTER, inter(&sha('f')), Some(0))];
    let a = apply(&p, ops.clone());
    let r = replay(&p, ops.clone()).unwrap();
    assert_eq!(r.project, a.project);
    let (head, c) = committed(
        &p,
        vec![pin(
            "X|400|normal",
            catalog_pin("X", 400, &sha('e'), &[]),
            None,
        )],
        "tx-a",
    );
    let mut t = tx(ops);
    t.tx_id = "tx-b".into();
    let r = rebase_tx(&p, &head, &t, &[c], &mut MemIds::after(&head)).unwrap();
    assert_eq!(r.applied.project.fonts.pins.len(), 2);
}
