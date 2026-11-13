//! Golden tests of the frozen upcaster steps (plan 4C Decision 5): frozen old shapes in, the
//! expected new shapes out. The snapshot goldens are `upgrade_golden/snapshot_v{1,2}.json`.

mod common;

use common::*;
use fin_core::{
    Diff, Edge, MemIds, Op, Project, Rational, apply_internal, new_project, upcast_diff,
    upcast_snapshot,
};
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

/// The v2 shape: kind `item` with its op's track, each entry's op, a start trim's `edge` and
/// `by`, and no shift for the `clip.remove` without ripple (its `item.remove` lifts and
/// records none; its change keeps that shift's track).
fn v2_diff() -> Value {
    json!({
        "changes": [
            { "kind": "asset", "id": "a1", "change": "added", "op": 0 },
            { "kind": "item", "id": "c1", "change": "added", "op": 1, "track": "t1" },
            { "kind": "item", "id": "c1", "change": "updated", "op": 2, "track": "t1", "edge": "start",
              "by": "-1/2" },
            { "kind": "item", "id": "c1", "change": "updated", "op": 3, "track": "t1", "edge": "end" },
            { "kind": "item", "id": "c1", "change": "updated", "op": 4, "track": "t1", "edge": "start",
              "by": "3" },
            { "kind": "item", "id": "c1", "change": "removed", "op": 5, "track": "t1" },
            { "kind": "item", "id": "c1", "change": "added", "op": 6, "track": "t1" },
            { "kind": "item", "id": "c1", "change": "removed", "op": 7, "track": "t1" },
            { "kind": "asset", "id": "a1", "change": "removed", "op": 8 }
        ],
        "shifts": [
            { "track": "t1", "from": "0", "by": "8", "op": 1 },
            { "track": "t1", "from": "8", "by": "-1/2", "op": 2 },
            { "track": "t1", "from": "15/2", "by": "-1", "op": 3 },
            { "track": "t1", "from": "13/2", "by": "3", "op": 4 },
            { "track": "t1", "from": "19/2", "by": "-19/2", "op": 5 },
            { "track": "t1", "from": "0", "by": "8", "op": 6 }
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

/// A change takes its track from its own op's shift, a lift's dropped zero shift included;
/// a shift without a track is damage.
#[test]
fn v1_change_track_comes_from_its_shift() {
    let ops = [
        json!({ "op": "clip.delete", "clip": "c1" }),
        json!({ "op": "clip.remove", "id": "c2", "ripple": false }),
    ];
    let diff = json!({
        "changes": [{ "kind": "clip", "id": "c1", "change": "removed" },
                    { "kind": "clip", "id": "c2", "change": "removed" }],
        "shifts": [{ "track": "t2", "from": "5", "by": "-5" },
                   { "track": "t3", "from": "4", "by": "0" }] });
    let got = serde_json::to_value(upcast_diff(diff.clone(), &ops).unwrap()).unwrap();
    assert_eq!(
        (&got["changes"][0]["track"], &got["changes"][1]["track"]),
        (&json!("t2"), &json!("t3"))
    );
    assert_eq!(got["shifts"].as_array().unwrap().len(), 1);
    let mut no_track = diff;
    no_track["shifts"][1]
        .as_object_mut()
        .unwrap()
        .remove("track");
    assert_eq!(
        upcast_diff(no_track, &ops).unwrap_err().code,
        "INVALID_FORMAT"
    );
}

/// The content offset `−delta` is written in canonical form whatever form the delta has; a
/// delta that is not a time is `INVALID_FORMAT`.
#[test]
fn start_trim_offset_is_canonical() {
    let by = |delta: &str| {
        let ops = [json!({ "op": "clip.trim", "clip": "c1", "edge": "start", "delta": delta })];
        let diff = json!({ "changes": [{ "kind": "clip", "id": "c1", "change": "updated" }],
                           "shifts": [{ "track": "t1", "from": "5", "by": "0" }] });
        upcast_diff(diff, &ops)
            .map(|d| serde_json::to_value(d).unwrap()["changes"][0]["by"].clone())
    };
    for (delta, want) in [
        ("1", "-1"),
        ("-1", "1"),
        ("1/2", "-1/2"),
        ("-7/3", "7/3"),
        ("+1", "-1"),
        ("+3/2", "-3/2"),
        ("01", "-1"),
        ("2/4", "-1/2"),
        ("-6/3", "2"),
        ("-0", "0"),
        ("0/5", "0"),
        ("9223372036854775807", "-9223372036854775807"),
    ] {
        assert_eq!(by(delta).unwrap(), json!(want), "{delta}");
    }
    for bad in [
        "",
        "-",
        "+",
        "x",
        "1/0",
        "1/",
        "/2",
        "1.5",
        "--1",
        "-+1",
        "1/-2",
        "18446744073709551616",
    ] {
        assert_eq!(by(bad).unwrap_err().code, "INVALID_FORMAT", "{bad:?}");
    }
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

/// The v1 shape of a v2 op as 4B wrote it (the record step run backwards; test only).
fn downcast(op: &Op) -> Value {
    let mut v = serde_json::to_value(op).unwrap();
    let m = v.as_object_mut().unwrap();
    let mode = m.remove("mode");
    let ripple = mode.as_ref().map(|m| m == "ripple");
    match m["op"].as_str().unwrap() {
        "asset.add" => {
            m.remove("kind");
        }
        "item.insert" => {
            m.insert("op".into(), json!("clip.insert"));
            m.remove("kind");
        }
        "item.trim" | "item.delete" => {
            let name = m["op"].as_str().unwrap().replace("item.", "clip.");
            m.insert("op".into(), json!(name));
            let item = m.remove("item").unwrap();
            m.insert("clip".into(), item);
        }
        "item.remove" => {
            m.insert("op".into(), json!("clip.remove"));
            m.insert("ripple".into(), json!(ripple));
        }
        "item.restore" => {
            m.insert("op".into(), json!("clip.restore"));
            let mut item = m.remove("item").unwrap();
            item.as_object_mut().unwrap().remove("kind");
            m.insert("clip".into(), item);
            m.insert("ripple".into(), json!(ripple));
        }
        _ => {}
    }
    v
}

/// The v1 shape of a fresh v2 diff of ripple ops: no `op`, `edge` or `by`; kind `clip`.
fn v1_shape(fresh: &Diff) -> Value {
    let mut v1 = serde_json::to_value(fresh).unwrap();
    for (list, keys) in [("changes", &["op", "edge", "by"][..]), ("shifts", &["op"])] {
        for e in v1[list].as_array_mut().unwrap() {
            let e = e.as_object_mut().unwrap();
            for k in keys {
                e.remove(*k);
            }
            if e.get("kind") == Some(&json!("item")) {
                e.insert("kind".into(), json!("clip"));
            }
        }
    }
    v1
}

/// A v1 record (as 4B stored it) of `intent_ops` with the given resolved and inverse ops.
fn v1_record(
    intent_ops: Vec<Value>,
    resolved: Vec<Value>,
    inverse: Vec<Value>,
    bindings: Value,
) -> Value {
    json!({
        "txId": "01JC0000000000000000000001", "seq": 3, "kind": "edit", "schemaVersion": 1,
        "parentHash": null, "author": "fixture", "committedAt": 1_700_000_000_000_i64,
        "intent": { "txId": "01JC0000000000000000000001", "author": "fixture",
                    "branch": "main", "baseRev": 2, "schemaVersion": 1, "ops": intent_ops },
        "resolved": resolved, "inverse": inverse, "bindings": bindings
    })
}

/// The stored-diff step equals 4C's replay of the upcast record: a ripple tx and its inverse,
/// written in the v1 shapes, upcast, replayed, and their v1-shaped fresh diffs upcast back.
#[test]
fn v1_shape_of_a_fresh_diff_upcasts_back() {
    let p = project();
    let ops = vec![
        av_asset("$a", secs(100)),
        insert("$c1", "t1", secs(0), "$a", secs(0), secs(5)),
        insert("$c2", "t1", secs(5), "$a", secs(0), secs(4)),
        trim("$c1", Edge::Start, r("1/3")),
        trim("$c2", Edge::End, secs(-1)),
        delete("$c2"),
    ];
    let client = apply(&p, ops.clone());
    let undo = apply_internal(
        &client.project,
        &tx(client.inverse.clone()),
        &mut MemIds::after(&client.project),
    )
    .unwrap();
    assert!(
        undo.diff.changes.len() == 6 && client.diff.shifts.len() == 5,
        "every kind is covered: {:?}",
        undo.diff
    );
    let v1 = |ops: &[Op]| -> Vec<Value> { ops.iter().map(downcast).collect() };
    let bindings = serde_json::to_value(&client.bindings).unwrap();
    let resolved = fin_core::resolve_ops(&ops, &client.bindings).unwrap();
    let record = v1_record(v1(&ops), v1(&resolved), v1(&client.inverse), bindings);
    // The record step gives back the v2 ops exactly.
    let up = fin_core::upcast_record(record).unwrap();
    assert_eq!(
        (
            up.intent.ops.clone(),
            up.resolved.clone(),
            up.inverse.clone()
        ),
        (ops.clone(), resolved, client.inverse.clone())
    );
    // Replaying it records what the diff step gives its v1 diff.
    let replay = fin_core::apply_committed(&p, &up.intent, &up.bindings).unwrap();
    assert_eq!(replay.diff, client.diff);
    let cases = [
        (Vec::new(), Diff::default()),
        (v1(&ops), client.diff.clone()),
        (v1(&client.inverse), undo.diff.clone()),
    ];
    for (ops, fresh) in cases {
        assert_eq!(upcast_diff(v1_shape(&fresh), &ops).unwrap(), fresh);
    }
}

/// A lift remove and restore: the v1 zero shifts are dropped, as the v2 ops record none.
#[test]
fn lift_ops_lose_their_zero_shift() {
    let ops = [
        json!({ "op": "clip.restore", "clip": { "id": "c1", "asset": "a1", "start": "2",
                "sourceIn": "0", "duration": "1" }, "track": "t1", "ripple": false }),
        json!({ "op": "clip.remove", "id": "c1", "ripple": false }),
        json!({ "op": "clip.remove", "id": "c2", "ripple": true }),
    ];
    let diff = json!({
        "changes": [{ "kind": "clip", "id": "c1", "change": "added" },
                    { "kind": "clip", "id": "c1", "change": "removed" },
                    { "kind": "clip", "id": "c2", "change": "removed" }],
        "shifts": [{ "track": "t1", "from": "2", "by": "0" },
                   { "track": "t1", "from": "3", "by": "0" },
                   { "track": "t1", "from": "9", "by": "-4" }]
    });
    let got = serde_json::to_value(upcast_diff(diff, &ops).unwrap()).unwrap();
    assert_eq!(
        got["shifts"],
        json!([{ "track": "t1", "from": "9", "by": "-4", "op": 2 }])
    );
}

/// The frozen record step, op by op (plan 4C Task 7): each v1 op shape in `intent`, `resolved`
/// and `inverse`, to its v2 shape.
#[test]
fn upcast_v1_record_golden_per_op() {
    let cases = [
        (
            json!({ "op": "asset.add", "id": "$a", "path": "m.mov", "duration": "600",
                    "video": { "width": 1920, "height": 1080, "rate": "30" }, "fingerprint": "fp" }),
            json!({ "op": "asset.add", "id": "$a", "kind": "media", "path": "m.mov",
                    "fingerprint": "fp", "duration": "600",
                    "video": { "width": 1920, "height": 1080, "rate": "30" } }),
        ),
        (
            json!({ "op": "clip.insert", "id": "$c", "track": "t1", "at": "1/2", "asset": "a1",
                    "sourceIn": "2", "duration": "8" }),
            json!({ "op": "item.insert", "id": "$c", "track": "t1", "at": "1/2", "duration": "8",
                    "mode": "ripple", "kind": "clip", "asset": "a1", "sourceIn": "2" }),
        ),
        (
            json!({ "op": "clip.trim", "clip": "c1", "edge": "start", "delta": "-1" }),
            json!({ "op": "item.trim", "item": "c1", "edge": "start", "delta": "-1", "mode": "ripple" }),
        ),
        (
            json!({ "op": "clip.delete", "clip": "c1" }),
            json!({ "op": "item.delete", "item": "c1", "mode": "ripple" }),
        ),
        (
            json!({ "op": "clip.remove", "id": "c1", "ripple": true }),
            json!({ "op": "item.remove", "id": "c1", "mode": "ripple" }),
        ),
        (
            json!({ "op": "clip.remove", "id": "c1", "ripple": false }),
            json!({ "op": "item.remove", "id": "c1", "mode": "lift" }),
        ),
        (
            json!({ "op": "clip.restore", "clip": { "id": "c1", "asset": "a1", "start": "3",
                    "sourceIn": "1", "duration": "2" }, "track": "t1", "ripple": false }),
            json!({ "op": "item.restore", "item": { "id": "c1", "start": "3", "duration": "2",
                    "kind": "clip", "asset": "a1", "sourceIn": "1" }, "track": "t1", "mode": "lift" }),
        ),
        // A missing `ripple` on a restore is 4B's default: true.
        (
            json!({ "op": "clip.restore", "clip": { "id": "c1", "asset": "a1", "start": "3",
                    "sourceIn": "1", "duration": "2" }, "track": "t1" }),
            json!({ "op": "item.restore", "item": { "id": "c1", "start": "3", "duration": "2",
                    "kind": "clip", "asset": "a1", "sourceIn": "1" }, "track": "t1", "mode": "ripple" }),
        ),
        (
            json!({ "op": "asset.remove", "id": "a1" }),
            json!({ "op": "asset.remove", "id": "a1" }),
        ),
    ];
    for (v1, v2) in cases {
        let record = v1_record(
            vec![v1.clone()],
            vec![v1.clone()],
            vec![v1.clone()],
            json!({}),
        );
        let up = fin_core::upcast_record(record).unwrap();
        for ops in [&up.intent.ops, &up.resolved] {
            assert_eq!(
                serde_json::to_value(ops).unwrap(),
                json!([v2.clone()]),
                "{v1}"
            );
        }
        // An inverse trim acts on its item alone (a 4B clip had no link group).
        let mut inverse = v2.clone();
        if inverse["op"] == "item.trim" {
            inverse["alone"] = json!(true);
        }
        assert_eq!(
            serde_json::to_value(&up.inverse).unwrap(),
            json!([inverse]),
            "{v1}"
        );
        assert_eq!((up.schema_version, up.intent.schema_version), (5, 5));
    }
}

/// The whole record: ids, timestamps and bindings untouched; every list upgraded.
#[test]
fn upcast_v1_record_golden_whole() {
    let record = v1_record(
        vec![v1_ops()[0].clone(), v1_ops()[1].clone()],
        vec![
            json!({ "op": "asset.add", "id": "a1", "path": "media/clip.mov", "duration": "600",
                    "fingerprint": "fp" }),
            json!({ "op": "clip.insert", "id": "c1", "track": "t1", "at": "0", "asset": "a1",
                    "sourceIn": "0", "duration": "8" }),
        ],
        vec![
            json!({ "op": "clip.remove", "id": "c1", "ripple": true }),
            json!({ "op": "asset.remove", "id": "a1" }),
        ],
        json!({ "$a": "a1", "$c": "c1" }),
    );
    let up = fin_core::upcast_record(record.clone()).unwrap();
    assert_eq!(up.tx_id, record["txId"]);
    assert_eq!(
        (up.seq, up.committed_at, up.author.as_str()),
        (3, 1_700_000_000_000, "fixture")
    );
    assert_eq!(
        serde_json::to_value(&up.bindings).unwrap(),
        record["bindings"]
    );
    let names = |ops: &[Op]| ops.iter().map(Op::name).collect::<Vec<_>>();
    assert_eq!(names(&up.intent.ops), ["asset.add", "item.insert"]);
    assert_eq!(names(&up.resolved), ["asset.add", "item.insert"]);
    assert_eq!(names(&up.inverse), ["item.remove", "asset.remove"]);
    // Every op the v1 lists held is a v2 op now: none is an alias.
    assert!(
        up.intent
            .ops
            .iter()
            .chain(&up.resolved)
            .chain(&up.inverse)
            .all(|o| !o.is_alias())
    );
    // A malformed v1 op is `INVALID_FORMAT`.
    let bad = v1_record(
        vec![json!({ "op": "clip.restore", "clip": "c1", "track": "t1" })],
        vec![],
        vec![],
        json!({}),
    );
    assert_eq!(
        fin_core::upcast_record(bad).unwrap_err().code,
        "INVALID_FORMAT"
    );
}

fn golden(name: &str) -> Value {
    let path = format!("{}/tests/upgrade_golden/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

/// The frozen snapshot steps: `snapshot_v1.json` upcasts through v2 (`snapshot_v2.json`) and
/// v3 and v4 to v5, compared as a decoded `Project` and as re-encoded JSON (each step's output
/// is canonical JSON). The v2 golden holds no keyframe, so v2 → v3 only stamps the version, and
/// v3 → v4 and v4 → v5 always do.
#[test]
fn upcast_v1_snapshot_golden() {
    let got = upcast_snapshot(golden("snapshot_v1.json")).unwrap();
    let mut current = golden("snapshot_v2.json");
    current["schemaVersion"] = json!(5);
    let want: Project = serde_json::from_value(current.clone()).unwrap();
    assert_eq!(got, want);
    assert_eq!(serde_json::to_value(&got).unwrap(), current);
    assert_eq!(upcast_snapshot(golden("snapshot_v2.json")).unwrap(), want);
}

/// The v2 → v3 snapshot step (spec §5.12.2): every key gets `kf<n>` in document order (items,
/// then paths in order, effect paths included), and nothing else changes. `corpus_05_v2.json`
/// is the v2 corpus case 05 (item and effect keys); the corpus holds it at v5 (v3 → v4 and v4 → v5 only
/// stamp the version).
#[test]
fn upcast_v2_snapshot_labels_keys_in_document_order() {
    let case = |dir: &str, name: &str| -> Value {
        let path = format!("{}/tests/{dir}/{name}", env!("CARGO_MANIFEST_DIR"));
        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v["project"].clone()
    };
    let v2 = case("upgrade_golden", "corpus_05_v2.json");
    let v3 = case("eval_corpus/projects", "05-bezier-lut-blur.json");
    assert_eq!(v2["schemaVersion"], 2);
    let got = upcast_snapshot(v2.clone()).unwrap();
    assert_eq!(serde_json::to_value(&got).unwrap(), v3);
    // Only ids and the version differ.
    let mut stripped = v3.clone();
    stripped["schemaVersion"] = json!(2);
    for tl in stripped["timelines"].as_array_mut().unwrap() {
        for tr in tl["tracks"].as_array_mut().unwrap() {
            for it in tr["items"].as_array_mut().unwrap() {
                for keys in it["keyframes"]
                    .as_object_mut()
                    .into_iter()
                    .flat_map(|m| m.values_mut())
                {
                    for k in keys.as_array_mut().unwrap() {
                        k.as_object_mut().unwrap().remove("id");
                    }
                }
            }
        }
    }
    assert_eq!(stripped, v2);
    let ids: Vec<String> = got
        .timelines
        .iter()
        .flat_map(|t| &t.tracks)
        .flat_map(|t| &t.items)
        .flat_map(|i| i.common.keyframes.values().flatten())
        .map(|k| k.id.clone())
        .collect();
    assert!(!ids.is_empty());
    assert_eq!(
        ids,
        (1..=ids.len())
            .map(|n| format!("kf{n}"))
            .collect::<Vec<_>>()
    );
    let effect_keys = got
        .timelines
        .iter()
        .flat_map(|t| &t.tracks)
        .flat_map(|t| &t.items)
        .any(|i| i.common.keyframes.keys().any(|p| p.starts_with("effects.")));
    assert!(effect_keys, "the golden holds effect keys too");
}

/// A v2 record whose ops and lists hold no keyframe upcasts unchanged but for its version; one
/// with a keyframe op, or a key list nested in a put-back, is `SCHEMA_VERSION` (a pre-release
/// format, spec §5.12.2).
#[test]
fn upcast_v2_record_refuses_keyframes() {
    let v2 = |ops: Value| -> Value {
        let mut r = v1_record(vec![], vec![], vec![], json!({}));
        r["schemaVersion"] = json!(2);
        r["intent"]["schemaVersion"] = json!(2);
        r["intent"]["ops"] = ops.clone();
        r["resolved"] = ops;
        r
    };
    let plain = json!([{ "op": "marker.add", "id": "$m", "timeline": "tl1", "at": "1",
                         "label": "x" }]);
    let up = fin_core::upcast_record(v2(plain.clone())).unwrap();
    assert_eq!((up.schema_version, up.intent.schema_version), (5, 5));
    assert_eq!(serde_json::to_value(&up.intent.ops).unwrap(), plain);
    let keyed = json!([{ "op": "keyframe.set", "item": "c1", "path": "opacity", "at": "1",
                         "value": 0.5, "interp": { "type": "linear" } }]);
    let e = fin_core::upcast_record(v2(keyed)).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["schemaVersion"]),
        ("SCHEMA_VERSION", &json!(2))
    );
    let nested = json!([{ "op": "item.restore", "track": "t1", "mode": "lift",
        "item": { "id": "c1", "start": "0", "duration": "1", "kind": "adjustment",
                  "keyframes": { "opacity": [{ "at": "0", "value": 1.0,
                                               "interp": { "type": "hold" } }] } } }]);
    assert_eq!(
        fin_core::upcast_record(v2(nested)).unwrap_err().code,
        "SCHEMA_VERSION"
    );
}

#[test]
fn malformed_v1_snapshot_is_invalid_format() {
    let base = golden("snapshot_v1.json");
    let breaks: [fn(&mut Value); 5] = [
        |v| {
            v.as_object_mut().unwrap().remove("timeline");
        },
        |v| v["timeline"]["tracks"] = json!({}),
        |v| v["timeline"]["tracks"][0]["kind"] = json!("caption"),
        |v| v["timeline"]["tracks"][0]["items"][0] = json!("c1"),
        |v| v["assets"] = json!([]),
    ];
    for (i, f) in breaks.iter().enumerate() {
        let mut v = base.clone();
        f(&mut v);
        let e = upcast_snapshot(v).unwrap_err();
        assert_eq!(e.code, "INVALID_FORMAT", "case {i}");
    }
}

/// `new_project` is the upcast of 4B's rev-0 project (plan 4C Decision 5), of 4C's (spec
/// §5.12.2), of schema 3's (spec §5.13) and of schema 4's (6B-1 Task 11): same timeline,
/// names, rate and 1080×1920, for any rate.
#[test]
fn new_project_equals_upcast_genesis() {
    for rate in ["30", "30000/1001", "25", "60"] {
        let v1 = json!({
            "id": "01m6r3sq1s34zp9esh9dtgwwjd", "name": "Genesis", "schemaVersion": 1,
            "rate": rate,
            "timeline": { "id": "tl1", "tracks": [
                { "id": "t1", "kind": "video", "items": [] },
                { "id": "t2", "kind": "audio", "items": [] }
            ] },
            "assets": {}
        });
        let rate: Rational = rate.parse().unwrap();
        let fresh =
            new_project("01m6r3sq1s34zp9esh9dtgwwjd".into(), "Genesis".into(), rate).unwrap();
        let up = upcast_snapshot(v1).unwrap();
        assert_eq!(up, fresh, "{rate}");
        // And from v2 (a rev-0 project holds no keyframe), v3 (nor any item) and v4 (nor any
        // asset).
        for version in [2, 3, 4] {
            let mut old = serde_json::to_value(&fresh).unwrap();
            old["schemaVersion"] = json!(version);
            let up = upcast_snapshot(old).unwrap();
            assert_eq!(up, fresh, "{rate} v{version}");
            assert_eq!(
                serde_json::to_string(&up).unwrap(),
                serde_json::to_string(&fresh).unwrap()
            );
        }
        assert_eq!(
            serde_json::to_string(&up).unwrap(),
            serde_json::to_string(&fresh).unwrap()
        );
        let tl = &up.timelines[0];
        assert_eq!((tl.resolution.width, tl.resolution.height), (1080, 1920));
        assert_eq!(tl.rate, rate);
    }
}

/// 4C Tasks 9–10 review: the stored inverse of a 4B trim, undone after its clip joined a link
/// group, trims that clip alone (not its whole group).
#[test]
fn undo_of_a_4b_trim_after_a_link_trims_the_item_alone() {
    // c1 [0,5) on t1 and c2 [0,5) on t2, both from a1; c1 was trimmed by 4B from 6 s to 5 s.
    let p = apply(
        &project(),
        vec![
            av_asset("$a", secs(100)),
            insert("$c1", "t1", secs(0), "$a", secs(0), secs(5)),
            insert("$c2", "t2", secs(0), "$a", secs(0), secs(5)),
            common::moves::link(&["$c1", "$c2"], "$lk"),
        ],
    )
    .project;
    let record = v1_record(
        vec![json!({ "op": "clip.trim", "clip": "c1", "edge": "end", "delta": "-1" })],
        vec![json!({ "op": "clip.trim", "clip": "c1", "edge": "end", "delta": "-1" })],
        vec![json!({ "op": "clip.trim", "clip": "c1", "edge": "end", "delta": "1" })],
        json!({}),
    );
    let up = fin_core::upcast_record(record).unwrap();
    let undone = apply_internal(&p, &tx(up.inverse.clone()), &mut MemIds::after(&p)).unwrap();
    let durations: Vec<Rational> = undone.project.timelines[0]
        .tracks
        .iter()
        .map(|t| t.items[0].duration)
        .collect();
    assert_eq!(durations, [secs(6), secs(5)], "only c1 grows back");
}

/// The v3 → v4 snapshot step is the identity (spec §5.13): `corpus_20_v3.json` (corpus case
/// 20 as schema 3 wrote it: a graphic, linked A/V, keyed items) upcasts to the current (v5)
/// corpus case exactly, with no graphic `sourceIn` and no fade offset guessed.
#[test]
fn upcast_v3_snapshot_is_identity() {
    let case = |dir: &str, name: &str| -> Value {
        let path = format!("{}/tests/{dir}/{name}", env!("CARGO_MANIFEST_DIR"));
        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v["project"].clone()
    };
    let v3 = case("upgrade_golden", "corpus_20_v3.json");
    let current = case("eval_corpus/projects", "20-mixed-reel.json");
    assert_eq!(
        (
            v3["schemaVersion"].clone(),
            current["schemaVersion"].clone()
        ),
        (json!(3), json!(5))
    );
    let got = upcast_snapshot(v3.clone()).unwrap();
    assert_eq!(serde_json::to_value(&got).unwrap(), current);
    let mut relabelled = v3;
    relabelled["schemaVersion"] = json!(5);
    assert_eq!(relabelled, current, "only the version differs");
    let items: Vec<_> = got
        .timelines
        .iter()
        .flat_map(|t| &t.tracks)
        .flat_map(|t| &t.items)
        .collect();
    assert!(
        items
            .iter()
            .any(|i| matches!(i.body, fin_core::ItemBody::Graphic { .. }))
    );
    for i in items {
        if let fin_core::ItemBody::Graphic { source_in, .. } = &i.body {
            assert!(source_in.is_zero(), "{}", i.id);
        }
        if let Some(a) = &i.common.audio {
            assert!(
                a.fade_in_offset.is_zero() && a.fade_out_offset.is_zero(),
                "{}",
                i.id
            );
        }
    }
}

/// A v3 record upcasts to v4 unchanged but for its versions (spec §5.13), keyframe ops and a
/// split included.
#[test]
fn upcast_v3_record_is_identity() {
    let ops = json!([
        { "op": "item.split", "item": "c1", "offset": "2", "id": "$r" },
        { "op": "keyframe.set", "item": "c1", "path": "opacity", "key": "kf1", "at": "1",
          "value": 0.5, "interp": { "type": "linear" } }
    ]);
    let mut r = v1_record(vec![], vec![], vec![], json!({ "$r": "c2" }));
    r["schemaVersion"] = json!(3);
    r["intent"]["schemaVersion"] = json!(3);
    r["intent"]["ops"] = ops.clone();
    r["resolved"] = ops;
    let up = fin_core::upcast_record(r.clone()).unwrap();
    assert_eq!((up.schema_version, up.intent.schema_version), (5, 5));
    let mut want = r;
    want["schemaVersion"] = json!(5);
    want["intent"]["schemaVersion"] = json!(5);
    let got = serde_json::to_value(&up).unwrap();
    for key in [
        "intent",
        "resolved",
        "inverse",
        "bindings",
        "txId",
        "seq",
        "committedAt",
    ] {
        assert_eq!(got[key], want[key], "{key}");
    }
}

/// The v4 → v5 snapshot step is the identity (6B-1 Task 11): `snapshot_v4.json` (corpus case
/// 05 as schema 4 wrote it: media assets with `video`, keyed items, plus schema 4's own
/// fields: a graphic's `sourceIn` and an audio clip's fade offsets) comes back as it was but
/// for `schemaVersion: 5`, with no `display`, `rotation` or `pixelAspect` guessed.
#[test]
fn upcast_v4_snapshot_golden() {
    let v4 = golden("snapshot_v4.json");
    assert_eq!(v4["schemaVersion"], 4);
    let got = upcast_snapshot(v4.clone()).unwrap();
    let mut v5 = v4;
    v5["schemaVersion"] = json!(5);
    assert_eq!(serde_json::to_value(&got).unwrap(), v5);
    let videos: Vec<_> = got
        .assets
        .values()
        .filter_map(|a| a.video.as_ref())
        .collect();
    assert!(!videos.is_empty(), "the golden has video assets");
    assert!(
        videos
            .iter()
            .all(|v| v.display.is_none() && v.rotation.is_none() && v.pixel_aspect.is_none())
    );
    let typed = serde_json::to_value(&got).unwrap();
    let tracks = &typed["timelines"][0]["tracks"];
    assert_eq!(
        tracks[1]["items"][0]["sourceIn"], "1/2",
        "the graphic's sourceIn"
    );
    let audio = &tracks[2]["items"][0]["audio"];
    assert_eq!(
        (&audio["fadeInOffset"], &audio["fadeOutOffset"]),
        (&json!("1"), &json!("1/2")),
        "the fade offsets"
    );
}

/// A v4 record upcasts to v5 unchanged but for its versions (6B-1 Task 11), a fade offset,
/// an `asset.add` with `video` and keyframe ops included.
#[test]
fn upcast_v4_record_is_identity() {
    let ops = json!([
        { "op": "asset.add", "id": "$a", "kind": "media", "path": "media/a.mov",
          "fingerprint": "fp", "duration": "60",
          "video": { "width": 1920, "height": 1080, "rate": "30" } },
        { "op": "param.set", "item": "c1", "path": "audio.fadeInOffset", "value": "2" },
        { "op": "keyframe.set", "item": "c1", "path": "opacity", "key": "kf1", "at": "1",
          "value": 0.5, "interp": { "type": "linear" } }
    ]);
    let mut r = v1_record(vec![], vec![], vec![], json!({ "$a": "a1" }));
    r["schemaVersion"] = json!(4);
    r["intent"]["schemaVersion"] = json!(4);
    r["intent"]["ops"] = ops.clone();
    r["resolved"] = ops;
    let up = fin_core::upcast_record(r.clone()).unwrap();
    assert_eq!((up.schema_version, up.intent.schema_version), (5, 5));
    let mut want = r;
    want["schemaVersion"] = json!(5);
    want["intent"]["schemaVersion"] = json!(5);
    let got = serde_json::to_value(&up).unwrap();
    for key in ["intent", "resolved", "inverse", "bindings", "txId", "seq"] {
        assert_eq!(got[key], want[key], "{key}");
    }
}

/// A schema-4 record cannot hold what only schema 5 has: an `asset.reprobe`, or a `video` with
/// `display`, `rotation` or `pixelAspect` is `INVALID_FORMAT` (damage or a mislabelled record,
/// never a v4 writer's), while an item's `transform.rotation` is v4 as ever.
#[test]
fn upcast_v4_record_refuses_v5_shapes() {
    // Each of the three op lists on its own (the others empty), so a scan of any one list
    // dropped fails here.
    for at in ["/intent/ops", "/resolved", "/inverse"] {
        let v4 = |ops: Value| -> Value {
            let mut r = v1_record(vec![], vec![], vec![], json!({}));
            r["schemaVersion"] = json!(4);
            r["intent"]["schemaVersion"] = json!(4);
            *r.pointer_mut(at).unwrap() = ops;
            r
        };
        let reprobe = json!([{ "op": "asset.reprobe", "asset": "a1" }]);
        let e = fin_core::upcast_record(v4(reprobe)).unwrap_err();
        assert_eq!(
            (e.code.as_str(), &e.details["schemaVersion"]),
            ("INVALID_FORMAT", &json!(4)),
            "{at}"
        );
        for field in ["display", "rotation", "pixelAspect"] {
            let mut video = json!({ "width": 1920, "height": 1080, "rate": "30" });
            video[field] = match field {
                "display" => json!({ "width": 1080, "height": 1920 }),
                "rotation" => json!(90),
                _ => json!("1"),
            };
            let add = json!([{ "op": "asset.add", "id": "$a", "kind": "media", "path": "a.mov",
                               "fingerprint": "fp", "duration": "1", "video": video }]);
            let e = fin_core::upcast_record(v4(add)).unwrap_err();
            assert_eq!(e.code, "INVALID_FORMAT", "{at} {field}");
        }
        let turned = json!([{ "op": "param.set", "item": "c1", "path": "transform.rotation",
                              "value": 90.0 }]);
        assert!(fin_core::upcast_record(v4(turned)).is_ok(), "{at}");
        // A graphic's text slots are user data: a slot named `op` holding `asset.reprobe` is
        // v4 text, not an op, and comes through unchanged.
        let slots = json!([{ "op": "item.insert", "id": "$g", "track": "t1", "at": "0",
                             "duration": "1", "mode": "ripple", "kind": "graphic",
                             "asset": "a1", "slots": { "op": "asset.reprobe" } }]);
        let up = fin_core::upcast_record(v4(slots.clone())).unwrap();
        let got = serde_json::to_value(&up).unwrap();
        assert_eq!(
            got.pointer(at).unwrap()[0]["slots"],
            slots[0]["slots"],
            "{at}"
        );
    }
}
