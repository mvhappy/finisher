//! Golden tests of the frozen upcaster steps (plan 4C Decision 5): frozen old shapes in, the
//! expected new shapes out.

mod common;

use common::*;
use fin_core::{Diff, Edge, MemIds, Op, apply_internal, upcast_diff};
use serde_json::{Value, json};

/// One v1 op of each kind, as 4B stored them (`intent.ops`), and the v1 diff their tx stored:
/// one change per op, one shift per clip op.
fn v1_ops() -> Vec<Value> {
    serde_json::from_value(json!([
        { "op": "asset.add", "id": "$a", "path": "media/clip.mov", "duration": "600",
          "video": { "width": 1920, "height": 1080, "rate": "30" },
          "audio": { "sampleRate": 48000, "channels": 2 }, "fingerprint": "fp" },
        { "op": "clip.insert", "id": "$c", "track": "t1", "at": "0", "asset": "$a",
          "sourceIn": "0", "duration": "8" },
        { "op": "clip.trim", "clip": "$c", "edge": "start", "delta": "1/2" },
        { "op": "clip.trim", "clip": "$c", "edge": "end", "delta": "-1" },
        { "op": "clip.trim", "clip": "$c", "edge": "start", "delta": "-3" },
        { "op": "clip.delete", "clip": "$c" },
        { "op": "clip.restore", "clip": { "id": "c1", "asset": "a1", "start": "0",
          "sourceIn": "0", "duration": "8" }, "track": "t1", "ripple": true },
        { "op": "clip.remove", "id": "c1", "ripple": false },
        { "op": "asset.remove", "id": "a1" }
    ]))
    .unwrap()
}

fn v1_diff() -> Value {
    json!({
        "changes": [
            { "kind": "asset", "id": "a1", "change": "added" },
            { "kind": "clip", "id": "c1", "change": "added" },
            { "kind": "clip", "id": "c1", "change": "updated" },
            { "kind": "clip", "id": "c1", "change": "updated" },
            { "kind": "clip", "id": "c1", "change": "updated" },
            { "kind": "clip", "id": "c1", "change": "removed" },
            { "kind": "clip", "id": "c1", "change": "added" },
            { "kind": "clip", "id": "c1", "change": "removed" },
            { "kind": "asset", "id": "a1", "change": "removed" }
        ],
        "shifts": [
            { "track": "t1", "from": "0", "by": "8" },
            { "track": "t1", "from": "8", "by": "-1/2" },
            { "track": "t1", "from": "15/2", "by": "-1" },
            { "track": "t1", "from": "13/2", "by": "3" },
            { "track": "t1", "from": "19/2", "by": "-19/2" },
            { "track": "t1", "from": "0", "by": "8" },
            { "track": "t1", "from": "8", "by": "0" }
        ]
    })
}

/// The v2 shape (kind stays `clip` until model v2 renames it, plan 4C Task 6).
fn v2_diff() -> Value {
    json!({
        "changes": [
            { "kind": "asset", "id": "a1", "change": "added", "op": 0 },
            { "kind": "clip", "id": "c1", "change": "added", "op": 1 },
            { "kind": "clip", "id": "c1", "change": "updated", "op": 2, "edge": "start",
              "by": "-1/2" },
            { "kind": "clip", "id": "c1", "change": "updated", "op": 3, "edge": "end" },
            { "kind": "clip", "id": "c1", "change": "updated", "op": 4, "edge": "start",
              "by": "3" },
            { "kind": "clip", "id": "c1", "change": "removed", "op": 5 },
            { "kind": "clip", "id": "c1", "change": "added", "op": 6 },
            { "kind": "clip", "id": "c1", "change": "removed", "op": 7 },
            { "kind": "asset", "id": "a1", "change": "removed", "op": 8 }
        ],
        "shifts": [
            { "track": "t1", "from": "0", "by": "8", "op": 1 },
            { "track": "t1", "from": "8", "by": "-1/2", "op": 2 },
            { "track": "t1", "from": "15/2", "by": "-1", "op": 3 },
            { "track": "t1", "from": "13/2", "by": "3", "op": 4 },
            { "track": "t1", "from": "19/2", "by": "-19/2", "op": 5 },
            { "track": "t1", "from": "0", "by": "8", "op": 6 },
            { "track": "t1", "from": "8", "by": "0", "op": 7 }
        ]
    })
}

#[test]
fn upcast_v1_diff_golden() {
    let got = upcast_diff(v1_diff(), &v1_ops()).unwrap();
    assert_eq!(serde_json::to_value(&got).unwrap(), v2_diff());
    // A trim by 0 at the start moves no content either way: `by` is "0", never "-0".
    let ops = [json!({ "op": "clip.trim", "clip": "c1", "edge": "start", "delta": "0" })];
    let diff = json!({ "changes": [{ "kind": "clip", "id": "c1", "change": "updated" }],
                       "shifts": [{ "track": "t1", "from": "5", "by": "0" }] });
    let got = serde_json::to_value(upcast_diff(diff, &ops).unwrap()).unwrap();
    assert_eq!(got["changes"][0]["by"], "0");
}

/// A v2 diff comes back as it is, whatever ops are passed.
#[test]
fn v2_diff_is_returned_unchanged() {
    let want: Diff = serde_json::from_value(v2_diff()).unwrap();
    assert_eq!(upcast_diff(v2_diff(), &[]).unwrap(), want);
    assert_eq!(upcast_diff(v2_diff(), &v1_ops()).unwrap(), want);
}

#[test]
fn mixed_diff_is_invalid_format() {
    let mut mixed = v1_diff();
    mixed["shifts"][0]["op"] = json!(1);
    let e = upcast_diff(mixed, &v1_ops()).unwrap_err();
    assert_eq!(e.code, "INVALID_FORMAT");
    assert_eq!(e.details["kind"], "diff");
    let mut mixed = v2_diff();
    mixed["changes"][4].as_object_mut().unwrap().remove("op");
    assert_eq!(
        upcast_diff(mixed, &v1_ops()).unwrap_err().code,
        "INVALID_FORMAT"
    );
}

/// A v1 diff that does not match its ops (counts, unknown kinds) is damage.
#[test]
fn mismatched_v1_diff_is_invalid_format() {
    let ops = v1_ops();
    let mut short = v1_diff();
    short["changes"].as_array_mut().unwrap().pop();
    assert_eq!(upcast_diff(short, &ops).unwrap_err().code, "INVALID_FORMAT");
    let mut short = v1_diff();
    short["shifts"].as_array_mut().unwrap().pop();
    assert_eq!(upcast_diff(short, &ops).unwrap_err().code, "INVALID_FORMAT");
    assert_eq!(
        upcast_diff(v1_diff(), &ops[..8]).unwrap_err().code,
        "INVALID_FORMAT"
    );
    let mut unknown = ops.clone();
    unknown[0]["op"] = json!("item.insert");
    assert_eq!(
        upcast_diff(v1_diff(), &unknown).unwrap_err().code,
        "INVALID_FORMAT"
    );
    assert_eq!(
        upcast_diff(json!({ "changes": [] }), &[]).unwrap_err().code,
        "INVALID_FORMAT"
    );
}

#[test]
fn empty_diff_reads_the_same() {
    let empty = json!({ "changes": [], "shifts": [] });
    assert_eq!(upcast_diff(empty.clone(), &[]).unwrap(), Diff::default());
    // A record with no ops (a branch or checkpoint) stored the empty diff.
    let empty_v1: Vec<Value> = Vec::new();
    assert_eq!(upcast_diff(empty, &empty_v1).unwrap(), Diff::default());
}

/// Today's ops record what the step gives a v1 diff: the v1 shape of a fresh diff (no `op`,
/// `edge` or `by`) upcasts back to it, for every op kind.
#[test]
fn v1_shape_of_a_fresh_diff_upcasts_back() {
    let p = project();
    let client = apply(
        &p,
        vec![
            av_asset("$a", secs(100)),
            insert("$c1", "t1", secs(0), "$a", secs(0), secs(5)),
            insert("$c2", "t1", secs(5), "$a", secs(0), secs(4)),
            Op::ClipTrim {
                clip: "$c1".into(),
                edge: Edge::Start,
                delta: r("1/3"),
            },
            Op::ClipTrim {
                clip: "$c2".into(),
                edge: Edge::End,
                delta: secs(-1),
            },
            Op::ClipDelete { clip: "$c2".into() },
        ],
    );
    let undo = apply_internal(
        &client.project,
        &tx(client.inverse.clone()),
        &mut MemIds::after(&client.project),
    )
    .unwrap();
    let ops_of = |ops: Vec<Op>| -> Vec<Value> {
        ops.iter()
            .map(|o| serde_json::to_value(o).unwrap())
            .collect()
    };
    let cases = [
        (Vec::new(), Diff::default()),
        (
            ops_of(vec![
                av_asset("$a", secs(100)),
                insert("$c1", "t1", secs(0), "$a", secs(0), secs(5)),
                insert("$c2", "t1", secs(5), "$a", secs(0), secs(4)),
                Op::ClipTrim {
                    clip: "$c1".into(),
                    edge: Edge::Start,
                    delta: r("1/3"),
                },
                Op::ClipTrim {
                    clip: "$c2".into(),
                    edge: Edge::End,
                    delta: secs(-1),
                },
                Op::ClipDelete { clip: "$c2".into() },
            ]),
            client.diff.clone(),
        ),
        (ops_of(client.inverse.clone()), undo.diff.clone()),
    ];
    assert!(
        undo.diff.changes.len() == 6 && client.diff.shifts.len() == 5,
        "every kind is covered: {:?}",
        undo.diff
    );
    for (ops, fresh) in cases {
        let mut v1 = serde_json::to_value(&fresh).unwrap();
        for (list, keys) in [("changes", &["op", "edge", "by"][..]), ("shifts", &["op"])] {
            for e in v1[list].as_array_mut().unwrap() {
                let e = e.as_object_mut().unwrap();
                for k in keys {
                    e.remove(*k);
                }
            }
        }
        assert_eq!(upcast_diff(v1, &ops).unwrap(), fresh);
    }
}
