//! Commit records: byte-stable encoding, placeholder resolution, upcasting.

mod common;

use std::collections::BTreeMap;

use common::*;
use fin_core::{
    CommittedTx, Diff, Edge, Link, MIN_READER, Op, RECORD_FORMAT, Record, RecordKind,
    SCHEMA_VERSION, encode_record, resolve_ops, upcast_record, upcast_snapshot,
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
    Record::new_edit(
        intent.tx_id.clone(),
        7,
        SCHEMA_VERSION,
        None,
        intent.author.clone(),
        1_700_000_000_000,
        intent,
        resolved,
        vec![
            fin_core::ItemRemove {
                id: "c1".into(),
                mode: fin_core::DeleteMode::Ripple,
            }
            .into(),
            Op::AssetRemove { id: "a1".into() },
        ],
        b,
    )
}

/// 4B writes format 2; a file holding only edits stays readable by version 1 (the segment
/// version follows content, `fin_store::segment::version_for`).
#[test]
fn format_versions_are_two() {
    assert_eq!((RECORD_FORMAT, MIN_READER), (2, 2));
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
    v["kind"] = json!("squash");
    let e = upcast_record(v).unwrap_err();
    assert_eq!(e.code, "NEWER_FORMAT");
    assert_eq!(e.details["recordKind"], "squash");

    let mut v = record_value();
    v["kind"] = json!(3);
    assert_eq!(upcast_record(v).unwrap_err().code, "INVALID_FORMAT");

    let mut v = record_value();
    v.as_object_mut().unwrap().remove("kind");
    assert_eq!(upcast_record(v).unwrap_err().code, "INVALID_FORMAT");
}

const H: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";

/// One link of every kind, with every optional field both set and unset.
fn every_link() -> Vec<Link> {
    vec![
        Link::Edit,
        Link::Undo {
            undoes: vec!["u2".into(), "u1".into()],
        },
        Link::Redo {
            redoes: "u3".into(),
        },
        Link::Merge {
            merge_id: "m1".into(),
            from: "br1".into(),
            origin: "o1".into(),
        },
        Link::CherryPick {
            origin: "o2".into(),
            from: "br2".into(),
            origin_hash: Some(H.into()),
            recovers: None,
        },
        Link::CherryPick {
            origin: "o2".into(),
            from: "br2".into(),
            origin_hash: None,
            recovers: None,
        },
        Link::Revert {
            reverts: "m1".into(),
        },
        Link::Checkpoint {
            name: "before-cut".into(),
            recovers: None,
        },
        Link::Branch {
            branch_id: "br3".into(),
            name: "alt".into(),
            fork_seq: 4,
            fork_hash: Some(H.into()),
            epoch: 1,
            recovers: None,
        },
        Link::Branch {
            branch_id: "br4".into(),
            name: "empty".into(),
            fork_seq: 0,
            fork_hash: None,
            epoch: 1,
            recovers: None,
        },
        Link::BranchSet {
            branch_id: "br3".into(),
            name: Some("alt2".into()),
            archived: None,
            recovers: None,
        },
        Link::BranchSet {
            branch_id: "br3".into(),
            name: None,
            archived: Some(true),
            recovers: None,
        },
        // Recovered stand-ins name the quarantined record they stand for (Decision 27).
        Link::CherryPick {
            origin: "o3".into(),
            from: "br2".into(),
            origin_hash: Some(H.into()),
            recovers: Some(H.into()),
        },
        Link::Checkpoint {
            name: "kept".into(),
            recovers: Some(H.into()),
        },
        Link::Branch {
            branch_id: "br5".into(),
            name: "again".into(),
            fork_seq: 0,
            fork_hash: None,
            epoch: 3,
            recovers: Some(H.into()),
        },
        Link::BranchSet {
            branch_id: "br3".into(),
            name: Some("alt3".into()),
            archived: None,
            recovers: Some(H.into()),
        },
    ]
}

/// `sample()` carrying `link`; kinds without ops get none.
fn linked(link: &Link) -> Record {
    let mut r = sample();
    if !link.kind().has_ops() {
        r.intent.ops.clear();
        r.resolved.clear();
        r.inverse.clear();
        r.bindings.clear();
    }
    r.set_link(link.clone());
    r
}

#[test]
fn every_kind_roundtrips_byte_stable() {
    for link in every_link() {
        let record = linked(&link);
        assert_eq!(record.kind, link.kind());
        assert_eq!(record.link().unwrap(), link);
        let bytes = encode_record(&record);
        let parsed: Record = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, record, "{link:?}");
        assert_eq!(encode_record(&parsed), bytes, "{link:?}");
        let up = upcast_record(serde_json::from_slice(&bytes).unwrap()).unwrap();
        assert_eq!(up, record, "{link:?}");

        // Link fields sit right after `kind`, in Decision 1 order, and only those present.
        let text = std::str::from_utf8(&bytes).unwrap();
        let order = [
            "kind",
            "undoes",
            "redoes",
            "mergeId",
            "from",
            "origin",
            "originHash",
            "reverts",
            "name",
            "branchId",
            "forkSeq",
            "forkHash",
            "archived",
            "recovers",
            "schemaVersion",
        ];
        let positions: Vec<usize> = order
            .iter()
            .filter_map(|k| text.find(&format!("\"{k}\":")))
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{text}");
        assert!(
            !text.contains("null,\"schemaVersion\""),
            "absent fields are omitted: {text}"
        );
    }
    // Setting another link clears the fields of the old one.
    let mut r = linked(&every_link()[8]);
    r.set_link(Link::Edit);
    assert_eq!(r.link().unwrap(), Link::Edit);
    let text = String::from_utf8(encode_record(&r)).unwrap();
    assert!(
        text.contains("\"kind\":\"edit\",\"schemaVersion\":"),
        "{text}"
    );
}

#[test]
fn edit_record_has_no_link_fields() {
    let text = String::from_utf8(encode_record(&sample())).unwrap();
    assert!(
        text.contains("\"kind\":\"edit\",\"schemaVersion\":"),
        "{text}"
    );
    assert_eq!(sample().link().unwrap(), Link::Edit);
}

#[test]
fn kind_names_are_camel_case() {
    let names = [
        (RecordKind::Edit, "edit", true, true, true),
        (RecordKind::Undo, "undo", true, true, false),
        (RecordKind::Redo, "redo", true, true, false),
        (RecordKind::Merge, "merge", false, true, false),
        (RecordKind::CherryPick, "cherryPick", true, true, false),
        (RecordKind::Revert, "revert", true, true, false),
        (RecordKind::Checkpoint, "checkpoint", false, false, false),
        (RecordKind::Branch, "branch", false, false, false),
        (RecordKind::BranchSet, "branchSet", false, false, false),
    ];
    for (kind, name, content, ops, is_4a) in names {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<RecordKind>(json!(name)).unwrap(),
            kind
        );
        assert_eq!(
            (kind.is_content(), kind.has_ops(), kind.is_4a()),
            (content, ops, is_4a),
            "{name}"
        );
    }
    assert_eq!(
        serde_json::to_value(Link::Merge {
            merge_id: "m".into(),
            from: "br1".into(),
            origin: "o".into()
        })
        .unwrap(),
        json!({"kind": "merge", "mergeId": "m", "from": "br1", "origin": "o"})
    );
    assert_eq!(
        serde_json::to_value(Link::CherryPick {
            origin: "o".into(),
            from: "br1".into(),
            origin_hash: None,
            recovers: None,
        })
        .unwrap(),
        json!({"kind": "cherryPick", "origin": "o", "from": "br1"})
    );
}

#[test]
fn link_fields_must_match_kind() {
    let value =
        |link: &Link| -> Value { serde_json::from_slice(&encode_record(&linked(link))).unwrap() };
    let links = every_link();
    let edit = value(&links[0]);
    let undo = value(&links[1]);
    let merge = value(&links[3]);
    let pick = value(&links[4]);
    let checkpoint = value(&links[7]);
    let branch = value(&links[8]);
    let root = value(&links[9]);
    let set = value(&links[10]);
    let with = |v: &Value, f: &dyn Fn(&mut serde_json::Map<String, Value>)| {
        let mut v = v.clone();
        f(v.as_object_mut().unwrap());
        v
    };
    let bad: Vec<(&str, Value)> = vec![
        (
            "edit with undoes",
            with(&edit, &|m| {
                m.insert("undoes".into(), json!(["u1"]));
            }),
        ),
        (
            "edit with archived",
            with(&edit, &|m| {
                m.insert("archived".into(), json!(true));
            }),
        ),
        (
            "undo without undoes",
            with(&undo, &|m| {
                m.remove("undoes");
            }),
        ),
        (
            "undo with no targets",
            with(&undo, &|m| {
                m.insert("undoes".into(), json!([]));
            }),
        ),
        (
            "undo with redoes",
            with(&undo, &|m| {
                m.insert("redoes".into(), json!("u1"));
            }),
        ),
        (
            "merge without from",
            with(&merge, &|m| {
                m.remove("from");
            }),
        ),
        (
            "merge without mergeId",
            with(&merge, &|m| {
                m.remove("mergeId");
            }),
        ),
        (
            "merge with originHash",
            with(&merge, &|m| {
                m.insert("originHash".into(), json!(H));
            }),
        ),
        (
            "merge with recovers",
            with(&merge, &|m| {
                m.insert("recovers".into(), json!(H));
            }),
        ),
        (
            "cherryPick with mergeId",
            with(&pick, &|m| {
                m.insert("mergeId".into(), json!("m1"));
            }),
        ),
        (
            "cherryPick without origin",
            with(&pick, &|m| {
                m.remove("origin");
            }),
        ),
        (
            "revert as edit",
            with(&value(&links[6]), &|m| {
                m.insert("kind".into(), json!("edit"));
            }),
        ),
        (
            "checkpoint without name",
            with(&checkpoint, &|m| {
                m.remove("name");
            }),
        ),
        (
            "checkpoint with ops",
            with(&checkpoint, &|m| {
                m["intent"]["ops"] = json!([{ "op": "clip.delete", "clip": "c1" }]);
                m.insert(
                    "resolved".into(),
                    json!([{ "op": "clip.delete", "clip": "c1" }]),
                );
            }),
        ),
        (
            "checkpoint with an inverse",
            with(&checkpoint, &|m| {
                m.insert(
                    "inverse".into(),
                    json!([{ "op": "clip.delete", "clip": "c1" }]),
                );
            }),
        ),
        (
            "branch without forkSeq",
            with(&branch, &|m| {
                m.remove("forkSeq");
            }),
        ),
        (
            "branch without forkHash above 0",
            with(&branch, &|m| {
                m.remove("forkHash");
            }),
        ),
        (
            "branch with forkHash at 0",
            with(&root, &|m| {
                m.insert("forkHash".into(), json!(H));
            }),
        ),
        (
            "branch without name",
            with(&branch, &|m| {
                m.remove("name");
            }),
        ),
        (
            "branchSet without branchId",
            with(&set, &|m| {
                m.remove("branchId");
            }),
        ),
        (
            "branchSet with forkSeq",
            with(&set, &|m| {
                m.insert("forkSeq".into(), json!(1));
            }),
        ),
        (
            "undoes not a list",
            with(&undo, &|m| {
                m.insert("undoes".into(), json!("u1"));
            }),
        ),
    ];
    for (what, v) in bad {
        let e = upcast_record(v).unwrap_err();
        assert_eq!(e.code, "INVALID_FORMAT", "{what}: {e}");
        assert_eq!(e.details["kind"], "record", "{what}");
    }
}

#[test]
fn committed_tx_link_omitted_for_edits() {
    let mut c = CommittedTx {
        rev: 3,
        tx: sample().intent,
        diff: Diff::default(),
        bindings: BTreeMap::new(),
        link: None,
    };
    let v = serde_json::to_value(&c).unwrap();
    assert!(v.get("link").is_none(), "{v}");
    let back: CommittedTx = serde_json::from_value(v).unwrap();
    assert_eq!(back, c);

    c.link = Some(Box::new(Link::Undo {
        undoes: vec!["u1".into()],
    }));
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["link"], json!({"kind": "undo", "undoes": ["u1"]}));
    assert_eq!(serde_json::from_value::<CommittedTx>(v).unwrap(), c);
}

/// Finite `f64`s from every corner: random bit patterns (subnormals included), `±0.0`, the
/// extremes and values with long decimal expansions.
fn any_finite() -> impl proptest::strategy::Strategy<Value = f64> {
    use proptest::prelude::*;
    prop_oneof![
        3 => any::<u64>().prop_map(f64::from_bits).prop_filter("finite", |x| x.is_finite()),
        1 => (0u64..1 << 52).prop_map(f64::from_bits),
        1 => (0u64..1 << 52).prop_map(|m| -f64::from_bits(m)),
        1 => prop::sample::select(vec![
            0.0, -0.0, f64::MIN_POSITIVE, -f64::MIN_POSITIVE, f64::MAX, f64::MIN, f64::EPSILON,
            5e-324, -5e-324, 0.1, 1.0 / 3.0, 2.0f64.sqrt(), 1e300, 123_456_789.012_345_68,
        ]),
        1 => any::<f64>().prop_filter("finite", |x| x.is_finite()),
    ]
}

/// The floats a clip's common fields carry, in a fixed order.
fn floats_of(c: &fin_core::Common) -> Vec<u64> {
    use fin_core::ParamValue;
    let mut out = vec![c.opacity, c.transform.rotation, c.transform.position[0]];
    for kfs in c.keyframes.values() {
        for k in kfs {
            match &k.value {
                ParamValue::Number(x) => out.push(*x),
                ParamValue::Vec2([x, y]) => out.extend([*x, *y]),
                _ => {}
            }
        }
    }
    for e in &c.effects {
        for v in e.params.values() {
            if let ParamValue::Number(x) = v {
                out.push(*x);
            }
        }
    }
    out.into_iter().map(f64::to_bits).collect()
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256),
        ..proptest::prelude::ProptestConfig::default()
    })]

    /// B2: parameter values inside ops come back bit for bit from record bytes (serde_json's
    /// `float_roundtrip`), through the reader's path (bytes → `Value` → `upcast_record`), and
    /// the record re-encodes to the same bytes.
    #[test]
    fn param_values_round_trip_through_record_bytes(
        xs in proptest::collection::vec(any_finite(), 7)
    ) {
        use fin_core::{DeleteMode, Effect, InsertMode, Interp, Item, ItemBody, ItemInsert, ItemRestore, Keyframe, ParamValue};
        let mut clip = Item::clip("c1", "a1", secs(0), secs(0), secs(1));
        clip.common.opacity = xs[0];
        clip.common.transform.rotation = xs[1];
        clip.common.transform.position[0] = xs[2];
        clip.common.keyframes.insert("opacity".into(), vec![
            Keyframe { id: "kf1".into(), at: secs(0), value: ParamValue::Number(xs[3]), interp: Interp::Linear },
            Keyframe { id: "kf2".into(), at: secs(1), value: ParamValue::Vec2([xs[4], xs[5]]), interp: Interp::Hold },
        ]);
        clip.common.effects.push(Effect {
            id: "fx1".into(),
            kind: "blur".into(),
            enabled: true,
            params: BTreeMap::from([("radius".to_string(), ParamValue::Number(xs[6]))]),
        });
        let want = floats_of(&clip.common);
        let restore: Op = ItemRestore { item: clip, track: "t1".into(), mode: DeleteMode::Ripple }.into();
        // An insert carries values in its body: a graphic's slots.
        let slots = BTreeMap::from([
            ("a".to_string(), ParamValue::Number(xs[0])),
            ("b".to_string(), ParamValue::Vec2([xs[1], xs[2]])),
        ]);
        let graphic: Op = ItemInsert {
            id: "$g".into(),
            track: "t1".into(),
            at: secs(0),
            duration: secs(1),
            mode: InsertMode::Ripple,
            body: ItemBody::Graphic { asset: "a1".into(), slots: slots.clone() },
        }
        .into();
        let mut record = sample();
        record.inverse = vec![restore.clone()];
        record.resolved = vec![restore, graphic];
        let bytes = encode_record(&record);
        let read = upcast_record(serde_json::from_slice(&bytes).unwrap()).unwrap();
        for op in read.inverse.iter().chain(&read.resolved) {
            match op {
                Op::ItemRestore(r) => proptest::prop_assert_eq!(floats_of(&r.item.common), want.clone()),
                Op::ItemInsert(i) => {
                    let ItemBody::Graphic { slots: got, .. } = &i.body else { panic!("{op:?}") };
                    let bits = |m: &BTreeMap<String, ParamValue>| -> Vec<u64> {
                        m.values().flat_map(|v| match v {
                            ParamValue::Number(x) => vec![x.to_bits()],
                            ParamValue::Vec2([x, y]) => vec![x.to_bits(), y.to_bits()],
                            _ => vec![],
                        }).collect()
                    };
                    proptest::prop_assert_eq!(bits(got), bits(&slots));
                }
                other => panic!("{other:?}"),
            }
        }
        proptest::prop_assert_eq!(encode_record(&read), bytes);
    }

    /// 6B-1 Task 11: an `asset.reprobe` (schema 4) comes back from record bytes as it was
    /// written, display, rotation and pixel aspect included, and re-encodes to the same bytes.
    #[test]
    fn reprobe_values_round_trip_through_record_bytes(
        w in 1u32..=16_384, h in 1u32..=16_384, rot in 0usize..4,
        pa in (1i64..=1000, 1i64..=1000), known in proptest::bool::ANY,
    ) {
        use fin_core::{AssetReprobe, AudioInfo, DisplaySize, Rational, Rotation, VideoInfo};
        let rotation = [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270][rot];
        let video = VideoInfo {
            display: known.then_some(DisplaySize { width: h, height: w }),
            rotation: known.then_some(rotation),
            pixel_aspect: known.then(|| Rational::new(pa.0, pa.1).unwrap()),
            ..VideoInfo::coded(w, h, Rational::new(30_000, 1001).unwrap())
        };
        let op: Op = AssetReprobe {
            asset: "a1".into(),
            video: Some(video),
            audio: Some(AudioInfo { sample_rate: 48_000, channels: 2 }),
        }
        .into();
        let mut record = sample();
        record.resolved = vec![op.clone()];
        record.inverse = vec![op.clone()];
        let bytes = encode_record(&record);
        let read = upcast_record(serde_json::from_slice(&bytes).unwrap()).unwrap();
        proptest::prop_assert_eq!(&read.resolved, &vec![op]);
        proptest::prop_assert_eq!(encode_record(&read), bytes);
    }
}
