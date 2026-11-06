mod common;

use common::ids_json::{ids_cover_every_id_in_json, ids_cover_json};
use fin_core::{Clip, Edge, Op, Rational};
use serde_json::json;

fn every_variant() -> Vec<Op> {
    let r = Rational::from_int;
    let ops = vec![
        Op::AssetAdd {
            id: "$a".into(),
            path: "a1.mov".into(),
            duration: r(2),
            video: None,
            audio: None,
            fingerprint: "c9".into(),
        },
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
        Op::AssetRemove { id: "a1".into() },
        Op::ClipRemove {
            id: "c1".into(),
            ripple: true,
        },
        Op::ClipRestore {
            clip: Box::new(Clip::new("c1", "a1", r(0), r(0), r(1))),
            track: "t1".into(),
            ripple: true,
        },
    ];
    // Adding a variant breaks this match, so the list above cannot silently fall behind.
    for op in &ops {
        match op {
            Op::AssetAdd { .. }
            | Op::ClipInsert { .. }
            | Op::ClipTrim { .. }
            | Op::ClipDelete { .. }
            | Op::AssetRemove { .. }
            | Op::ClipRemove { .. }
            | Op::ClipRestore { .. } => {}
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
}
