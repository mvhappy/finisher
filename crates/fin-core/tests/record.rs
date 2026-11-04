//! Commit records: byte-stable encoding, placeholder resolution, upcasting.

mod common;

use std::collections::BTreeMap;

use common::*;
use fin_core::{
    Edge, MIN_READER, Op, RECORD_FORMAT, Record, RecordKind, SCHEMA_VERSION, encode_record,
    resolve_ops, upcast_record, upcast_snapshot,
};
use serde_json::{Value, json};

fn bindings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn sample() -> Record {
    let intent = tx(vec![
        av_asset("$a", secs(100)),
        insert("$c", "t1", secs(0), "$a", secs(0), secs(5)),
    ]);
    let b = bindings(&[("$a", "a1"), ("$c", "c1")]);
    let resolved = resolve_ops(&intent.ops, &b).unwrap();
    Record {
        tx_id: intent.tx_id.clone(),
        seq: 7,
        kind: RecordKind::Edit,
        schema_version: SCHEMA_VERSION,
        parent_hash: None,
        author: intent.author.clone(),
        committed_at: 1_700_000_000_000,
        intent,
        resolved,
        inverse: vec![
            Op::ClipRemove {
                id: "c1".into(),
                ripple: true,
            },
            Op::AssetRemove { id: "a1".into() },
        ],
        bindings: b,
    }
}

#[test]
fn format_versions_start_at_one() {
    assert_eq!((RECORD_FORMAT, MIN_READER), (1, 1));
}

#[test]
fn record_roundtrip_is_byte_stable() {
    let bytes = encode_record(&sample());
    assert_ne!(bytes.last(), Some(&b'\n'), "no trailing newline");
    let parsed: Record = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed, sample());
    assert_eq!(encode_record(&parsed), bytes);

    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(!text.contains(": ") && !text.contains('\n'), "compact JSON");
    let order = [
        "txId",
        "seq",
        "kind",
        "schemaVersion",
        "parentHash",
        "author",
        "committedAt",
        "intent",
        "resolved",
        "inverse",
        "bindings",
    ];
    let positions: Vec<usize> = order
        .iter()
        .map(|k| text.find(&format!("\"{k}\":")).unwrap())
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{text}");
    assert!(text.starts_with("{\"txId\":"));
    assert!(text.contains("\"kind\":\"edit\""));
    assert!(text.contains("\"parentHash\":null"));
}

#[test]
fn resolve_ops_substitutes_every_placeholder() {
    let ops = vec![
        av_asset("$a", secs(100)),
        insert("$c", "t1", secs(0), "$a", secs(0), secs(5)),
        Op::ClipTrim {
            clip: "$c".into(),
            edge: Edge::End,
            delta: secs(1),
        },
        Op::ClipDelete { clip: "$c".into() },
        Op::ClipDelete { clip: "c9".into() },
    ];
    let resolved = resolve_ops(&ops, &bindings(&[("$a", "a1"), ("$c", "c4")])).unwrap();
    let text = serde_json::to_string(&resolved).unwrap();
    assert!(!text.contains('$'), "{text}");
    assert_eq!(
        resolved[1],
        insert("c4", "t1", secs(0), "a1", secs(0), secs(5))
    );
    assert_eq!(resolved[4], Op::ClipDelete { clip: "c9".into() });
}

#[test]
fn resolve_ops_unbound_placeholder_errors() {
    let ops = vec![insert("$c", "t1", secs(0), "$a", secs(0), secs(5))];
    let e = resolve_ops(&ops, &bindings(&[("$c", "c1")])).unwrap_err();
    assert_eq!(e.code, "UNBOUND_PLACEHOLDER");
    assert_eq!(e.details["placeholder"], "$a");
    assert_eq!(e.details["opIndex"], 0);
}

fn record_value() -> Value {
    serde_json::from_slice(&encode_record(&sample())).unwrap()
}

#[test]
fn upcast_current_record_is_identity() {
    assert_eq!(upcast_record(record_value()).unwrap(), sample());
}

#[test]
fn upcast_current_snapshot_is_identity() {
    let p = with_clips(&[3, 4]);
    assert_eq!(
        upcast_snapshot(serde_json::to_value(&p).unwrap()).unwrap(),
        p
    );
}

#[test]
fn upcast_rejects_newer() {
    let mut v = record_value();
    v["schemaVersion"] = json!(SCHEMA_VERSION + 1);
    let e = upcast_record(v).unwrap_err();
    assert_eq!(e.code, "NEWER_FORMAT");
    assert_eq!(e.details["schemaVersion"], SCHEMA_VERSION + 1);
    assert_eq!(e.details["supported"], SCHEMA_VERSION);
    assert!(e.fix.is_some());

    let mut p = serde_json::to_value(project()).unwrap();
    p["schemaVersion"] = json!(SCHEMA_VERSION + 1);
    assert_eq!(upcast_snapshot(p).unwrap_err().code, "NEWER_FORMAT");
}

#[test]
fn upcast_rejects_unknown_older_and_malformed() {
    let mut v = record_value();
    v["schemaVersion"] = json!(0);
    assert_eq!(upcast_record(v).unwrap_err().code, "SCHEMA_VERSION");

    let mut v = record_value();
    v.as_object_mut().unwrap().remove("schemaVersion");
    assert_eq!(upcast_record(v).unwrap_err().code, "INVALID_FORMAT");

    let mut v = record_value();
    v["seq"] = json!("seven");
    assert_eq!(upcast_record(v).unwrap_err().code, "INVALID_FORMAT");
}

#[test]
fn upcast_unknown_kind_is_newer_format() {
    let mut v = record_value();
    v["kind"] = json!("undo");
    let e = upcast_record(v).unwrap_err();
    assert_eq!(e.code, "NEWER_FORMAT");
    assert_eq!(e.details["recordKind"], "undo");

    let mut v = record_value();
    v["kind"] = json!(3);
    assert_eq!(upcast_record(v).unwrap_err().code, "INVALID_FORMAT");

    let mut v = record_value();
    v.as_object_mut().unwrap().remove("kind");
    assert_eq!(upcast_record(v).unwrap_err().code, "INVALID_FORMAT");
}
