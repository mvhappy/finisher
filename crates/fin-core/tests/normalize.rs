//! `fin_core::normalize` (plan 4C Decision 6): the `clip.*` client aliases become `item.*` ops
//! at the commit entry, and a schema-1 client tx of schema-1 op kinds is upgraded.

mod common;

use common::*;
use fin_core::{
    ApplyMode, AssetKind, Edge, MemIds, Op, SCHEMA_VERSION, Tx, apply_tx_mode, normalize,
};
use serde_json::json;

fn aliases() -> Vec<Op> {
    vec![
        Op::ClipInsert {
            id: "$c".into(),
            track: "t1".into(),
            at: secs(5),
            asset: "a1".into(),
            source_in: secs(1),
            duration: secs(2),
        },
        Op::ClipTrim {
            clip: "c1".into(),
            edge: Edge::Start,
            delta: secs(1),
        },
        Op::ClipDelete { clip: "c1".into() },
    ]
}

#[test]
fn normalize_maps_aliases() {
    let got = normalize(tx(aliases())).unwrap();
    assert_eq!(
        got.ops,
        [
            insert("$c", "t1", secs(5), "a1", secs(1), secs(2)),
            trim("c1", Edge::Start, secs(1)),
            delete("c1"),
        ]
    );
    // Everything else is untouched; normalising twice changes nothing.
    let mut want = tx(Vec::new());
    want.ops = got.ops.clone();
    assert_eq!(got, want);
    assert_eq!(normalize(got.clone()).unwrap(), got);
    // An alias applies as the op it stands for.
    let p = with_clips(&[5]);
    let via_alias = apply(&p, aliases());
    let via_items = apply(&p, got.ops);
    assert_eq!(via_alias, via_items);
}

fn v1_tx(ops: serde_json::Value) -> Tx {
    serde_json::from_value(json!({
        "txId": "tx-1", "author": "old-fin", "branch": "main", "baseRev": 3,
        "schemaVersion": 1, "argsHash": format!("blake3:{}", "0".repeat(64)), "ops": ops
    }))
    .unwrap()
}

#[test]
fn normalize_adds_media_kind_to_v1_asset_add() {
    let t = v1_tx(
        json!([{ "op": "asset.add", "id": "$a", "path": "/m.mov", "duration": "10",
                           "audio": { "sampleRate": 48000, "channels": 2 }, "fingerprint": "fp" }]),
    );
    let got = normalize(t).unwrap();
    let Op::AssetAdd(a) = &got.ops[0] else {
        panic!("{:?}", got.ops)
    };
    assert_eq!(a.kind, AssetKind::Media);
    assert_eq!(serde_json::to_value(&got.ops[0]).unwrap()["kind"], "media");
}

#[test]
fn v1_client_tx_is_accepted_and_upgraded() {
    let t = v1_tx(json!([
        { "op": "asset.add", "id": "$a", "path": "/m.mov", "duration": "10",
          "video": { "width": 2, "height": 2, "rate": "30" }, "fingerprint": "fp" },
        { "op": "clip.insert", "id": "$c", "track": "t1", "at": "0", "asset": "$a",
          "sourceIn": "0", "duration": "4" },
        { "op": "clip.trim", "clip": "$c", "edge": "end", "delta": "-1" }
    ]));
    let got = normalize(t.clone()).unwrap();
    assert_eq!(got.schema_version, SCHEMA_VERSION);
    assert_eq!(
        (&got.tx_id, &got.author, got.base_rev, &got.args_hash),
        (&t.tx_id, &t.author, t.base_rev, &t.args_hash),
        "ids, base and argsHash are untouched"
    );
    let names: Vec<&str> = got.ops.iter().map(Op::name).collect();
    assert_eq!(names, ["asset.add", "item.insert", "item.trim"]);
    let p = project();
    let a = apply_tx_mode(&p, &got, &mut MemIds::after(&p), ApplyMode::Client).unwrap();
    assert_eq!(clip(&a.project, "c1").duration, secs(3));
    // Unnormalised, a schema-1 tx is refused by the apply.
    let e = apply_tx_mode(&p, &t, &mut MemIds::after(&p), ApplyMode::Client).unwrap_err();
    assert_eq!(e.code, "SCHEMA_VERSION");
}

#[test]
fn v1_tx_with_a_v2_op_is_refused() {
    let mut t = tx(vec![delete("c1")]);
    t.schema_version = 1;
    let e = normalize(t).unwrap_err();
    assert_eq!(e.code, "SCHEMA_VERSION");
    assert_eq!(
        (&e.details["op"], &e.details["opIndex"]),
        (&json!("item.delete"), &json!(0))
    );
    // Other versions are the apply's to refuse.
    let mut t = tx(aliases());
    t.schema_version = 9;
    assert_eq!(normalize(t.clone()).unwrap(), t);
}

#[test]
fn alias_outside_client_mode_is_invalid_op() {
    let p = with_clips(&[5]);
    for mode in [ApplyMode::Internal, ApplyMode::Replay] {
        for op in aliases() {
            let e =
                apply_tx_mode(&p, &tx(vec![op.clone()]), &mut MemIds::after(&p), mode).unwrap_err();
            assert_eq!(e.code, "INVALID_OP", "{op:?} in {mode:?}");
            assert_eq!(e.details["op"], op.name());
        }
    }
}

/// Review of Task 7 (S1): the stored intent's values are canonical, so its bytes agree with
/// the state the insert makes: `-0.0` becomes `0.0`; a non-finite number is refused with its
/// op's index.
#[test]
fn normalize_makes_insert_values_canonical() {
    let graphic = |x: f64| -> Op {
        fin_core::ItemInsert {
            id: "$g".into(),
            track: "t1".into(),
            at: secs(0),
            duration: secs(1),
            mode: fin_core::InsertMode::Overwrite,
            body: fin_core::ItemBody::Graphic {
                asset: "a1".into(),
                slots: [("x".to_string(), fin_core::ParamValue::Vec2([x, 1.0]))].into(),
            },
        }
        .into()
    };
    let got = normalize(tx(vec![delete("c1"), graphic(-0.0)])).unwrap();
    let Op::ItemInsert(i) = &got.ops[1] else {
        panic!("{:?}", got.ops)
    };
    let fin_core::ItemBody::Graphic { slots, .. } = &i.body else {
        panic!("{i:?}")
    };
    let fin_core::ParamValue::Vec2([x, _]) = slots["x"] else {
        panic!("{slots:?}")
    };
    assert_eq!(x.to_bits(), 0.0f64.to_bits());
    assert!(
        serde_json::to_string(&got.ops[1])
            .unwrap()
            .contains("[0.0,1.0]")
    );
    let e = normalize(tx(vec![delete("c1"), graphic(f64::NAN)])).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(
        (&e.details["path"], &e.details["opIndex"]),
        (&json!("slots.x"), &json!(1))
    );
}
