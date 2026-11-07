mod common;

use common::ids_json::{ids_cover_every_id_in_json, ids_cover_json};
use fin_core::{
    AssetAdd, BlendMode, CaptionBlock, DeleteMode, Edge, Effect, InsertMode, Interp, Item,
    ItemBody, ItemDelete, ItemInsert, ItemLink, ItemMove, ItemRemove, ItemRestore, ItemSlide,
    ItemSlip, ItemSplit, ItemTrim, ItemUnlink, Keyframe, Op, ParamValue, Rational, SlideSides,
    TrimMode, Word,
};
use serde_json::json;

/// A clip item with everything that carries an id: effects, a link group, keyframes.
pub fn busy_clip() -> Item {
    let r = Rational::from_int;
    let mut item = Item::clip("c1", "a1", r(0), r(0), r(1));
    item.common.effects = vec![
        Effect {
            id: "fx1".into(),
            kind: "blur".into(),
            enabled: true,
            params: [("radius".to_string(), ParamValue::Number(2.5))].into(),
        },
        Effect {
            id: "fx2".into(),
            kind: "colorAdjust".into(),
            enabled: false,
            params: Default::default(),
        },
    ];
    item.common.link = Some("lk1".into());
    item.common.blend = BlendMode::Screen;
    item.common.keyframes.insert(
        "opacity".into(),
        vec![Keyframe {
            at: r(0),
            value: ParamValue::Number(0.5),
            interp: Interp::Linear,
        }],
    );
    item
}

/// A caption item in project style `st2`.
pub fn caption() -> Item {
    let r = Rational::from_int;
    let body = ItemBody::Caption {
        block: CaptionBlock {
            words: vec![Word {
                text: "hi".into(),
                start: r(0),
                end: r(1),
            }],
            style: "st2".into(),
            overrides: Default::default(),
            source: None,
        },
    };
    let mut item = Item::new("cap1", r(0), r(1), body);
    item.common.effects.push(Effect {
        id: "fx3".into(),
        kind: "blur".into(),
        enabled: true,
        params: Default::default(),
    });
    item
}

fn every_variant() -> Vec<Op> {
    let r = Rational::from_int;
    let mut add = AssetAdd::media("$a", "a1.mov", r(2), None, None, "c9");
    add.bin = Some("b1".into());
    let ops: Vec<Op> = vec![
        add.into(),
        ItemInsert {
            id: "$c".into(),
            track: "t1".into(),
            at: r(0),
            duration: r(1),
            mode: InsertMode::Ripple,
            body: ItemBody::Image { asset: "a1".into() },
        }
        .into(),
        ItemTrim {
            item: "c1".into(),
            edge: Edge::Start,
            delta: r(1),
            mode: TrimMode::Ripple,
            alone: true,
        }
        .into(),
        ItemDelete {
            item: "c1".into(),
            mode: DeleteMode::Lift,
        }
        .into(),
        ItemSlip {
            item: "c1".into(),
            delta: r(1),
        }
        .into(),
        ItemSplit {
            item: "c1".into(),
            offset: r(1),
            id: "$r".into(),
        }
        .into(),
        ItemMove {
            item: "c1".into(),
            track: "t2".into(),
            at: r(3),
            mode: InsertMode::Overwrite,
        }
        .into(),
        ItemSlide {
            item: "c1".into(),
            delta: r(1),
            sides: Some(SlideSides {
                before: Some("c2".into()),
                after: Some("c3".into()),
            }),
        }
        .into(),
        ItemLink {
            items: vec!["c1".into(), "$c".into()],
            id: "$lk".into(),
        }
        .into(),
        ItemUnlink {
            group: "lk1".into(),
        }
        .into(),
        Op::AssetRemove { id: "a1".into() },
        ItemRemove {
            id: "c1".into(),
            mode: DeleteMode::Ripple,
        }
        .into(),
        ItemRestore {
            item: busy_clip(),
            track: "t1".into(),
            mode: DeleteMode::Ripple,
        }
        .into(),
        Op::ClipInsert {
            id: "$c".into(),
            track: "t1".into(),
            at: r(0),
            asset: "a1".into(),
            source_in: r(0),
            duration: r(1),
        },
        Op::ClipTrim {
            clip: "c1".into(),
            edge: Edge::Start,
            delta: r(1),
        },
        Op::ClipDelete { clip: "c1".into() },
    ];
    // Adding a variant breaks this match, so the list above cannot silently fall behind.
    for op in &ops {
        match op {
            Op::AssetAdd(_)
            | Op::ItemInsert(_)
            | Op::ItemTrim(_)
            | Op::ItemDelete(_)
            | Op::ItemSlip(_)
            | Op::ItemSplit(_)
            | Op::ItemMove(_)
            | Op::ItemSlide(_)
            | Op::ItemLink(_)
            | Op::ItemUnlink(_)
            | Op::AssetRemove { .. }
            | Op::ItemRemove(_)
            | Op::ItemRestore(_)
            | Op::ClipInsert { .. }
            | Op::ClipTrim { .. }
            | Op::ClipDelete { .. } => {}
        }
    }
    ops
}

#[test]
fn ops_table_lists_every_variant() {
    let ops = every_variant();
    let mut names: Vec<&str> = ops.iter().map(Op::name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), ops.len(), "names are unique");
    for op in &ops {
        let json = serde_json::to_value(op).unwrap();
        assert_eq!(json["op"], op.name());
        let back: Op = serde_json::from_value(json).unwrap();
        assert_eq!(&back, op);
        ids_cover_every_id_in_json(op);
        let mut m = op.clone();
        let refs: Vec<String> = m.refs_mut().into_iter().map(|s| s.clone()).collect();
        assert_eq!(
            op.ids(),
            refs,
            "{}: ids lists what refs_mut does",
            op.name()
        );
        assert_eq!(op.is_alias(), op.name().starts_with("clip."));
    }
}

/// Decision 24: a restore lists every id nested in its item (B6).
#[test]
fn restore_lists_nested_ids() {
    let ops: Vec<Op> = [busy_clip(), caption()]
        .into_iter()
        .map(|item| {
            ItemRestore {
                item,
                track: "t3".into(),
                mode: DeleteMode::Lift,
            }
            .into()
        })
        .collect();
    assert_eq!(
        ops[0].ids(),
        ["c1", "a1", "fx1", "fx2", "lk1", "t3"],
        "{:?}",
        ops[0]
    );
    assert_eq!(ops[1].ids(), ["cap1", "st2", "fx3", "t3"]);
    for op in &ops {
        ids_cover_every_id_in_json(op);
    }
}

#[test]
fn ids_json_helper_flags_an_unlisted_id() {
    let op = Op::ClipDelete { clip: "c1".into() };
    let mut json = serde_json::to_value(&op).unwrap();
    assert_eq!(ids_cover_json(&op, &json), Ok(()));
    json["other"] = json!("c9");
    assert_eq!(ids_cover_json(&op, &json), Err(vec!["c9".to_string()]));
    // Free-text fields are not ids.
    let mut json = serde_json::to_value(&op).unwrap();
    json["label"] = json!("c9");
    assert_eq!(ids_cover_json(&op, &json), Ok(()));
    // An id nested in a restored item is found.
    let mut item = busy_clip();
    item.common.link = None;
    let op: Op = ItemRestore {
        item,
        track: "t1".into(),
        mode: DeleteMode::Ripple,
    }
    .into();
    let mut json = serde_json::to_value(&op).unwrap();
    json["item"]["link"] = json!("lk7");
    assert_eq!(ids_cover_json(&op, &json), Err(vec!["lk7".to_string()]));
}
