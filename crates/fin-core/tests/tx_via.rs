//! `Tx.via` (plan 8B Task 3, spec §3.8): optional and absent by default, with no upcaster. A
//! record without it keeps its bytes and hash; apply and replay ignore it. Rebase, pick,
//! export and import carrying it are `fin-store`'s `tests/tx_via.rs`.

mod common;

use std::collections::BTreeMap;

use common::{av_asset, insert, project, secs, tx};
use fin_core::tx::Via;
use fin_core::{MemIds, Op};
use fin_core::{Record, Tx, apply_committed, apply_tx, encode_record};

/// The bytes of [`record`] before `via` existed (and its hash): a record without `via` must
/// keep them exactly, or every stored chain would re-hash.
const PINNED: &str = concat!(
    r#"{"txId":"tx-test","seq":1,"kind":"edit","schemaVersion":3,"parentHash":"blake3:00","#,
    r#""author":"test","committedAt":1700000000000,"intent":{"txId":"tx-test","author":"test","#,
    r#""branch":"main","baseRev":0,"schemaVersion":3,"ops":[{"op":"asset.add","id":"$a","#,
    r#""kind":"media","path":"/media/a.mov","fingerprint":"fp","duration":"60","#,
    r#""video":{"width":1920,"height":1080,"rate":"30"},"audio":{"sampleRate":48000,"channels":2}}]},"#,
    r#""resolved":[{"op":"asset.add","id":"$a","kind":"media","path":"/media/a.mov","#,
    r#""fingerprint":"fp","duration":"60","video":{"width":1920,"height":1080,"rate":"30"},"#,
    r#""audio":{"sampleRate":48000,"channels":2}}],"inverse":[],"bindings":{"$a":"a1"}}"#,
);
const PINNED_HASH: &str = "0085835cdfd0ede183a00ec3de3ae10dae0a0157658b721d44022b7c0a2160e6";

fn via() -> Via {
    Via {
        plugin: "@acme/tools@1.2.0".into(),
        command: "trim-silence".into(),
    }
}

fn record(intent: Tx) -> Record {
    Record::new_edit(
        "tx-test".into(),
        1,
        intent.schema_version,
        Some("blake3:00".into()),
        "test".into(),
        1_700_000_000_000,
        intent.clone(),
        intent.ops.clone(),
        vec![],
        BTreeMap::from([("$a".to_string(), "a1".to_string())]),
    )
}

#[test]
fn via_absent_is_unchanged_bytes() {
    let t = tx(vec![av_asset("$a", secs(60))]);
    assert_eq!(t.via, None);
    let bytes = encode_record(&record(t.clone()));
    assert_eq!(String::from_utf8(bytes.clone()).unwrap(), PINNED);
    assert_eq!(blake3::hash(&bytes).to_hex().as_str(), PINNED_HASH);
    // Absent decodes as none, and a tx without it never writes the key.
    let back: Record = serde_json::from_slice(PINNED.as_bytes()).unwrap();
    assert_eq!(back.intent, t);
    assert!(!serde_json::to_string(&t).unwrap().contains("via"));

    // Present: stored with the record, and read back.
    let tagged = Tx {
        via: Some(Box::new(via())),
        ..t
    };
    let bytes = encode_record(&record(tagged.clone()));
    let s = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        s.contains(r#""via":{"plugin":"@acme/tools@1.2.0","command":"trim-silence"}"#),
        "{s}"
    );
    let back: Record = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(back.intent, tagged);
}

#[test]
fn apply_and_replay_ignore_via() {
    let p = project();
    let ops: Vec<Op> = vec![
        av_asset("$a", secs(60)),
        insert("$c", "t1", secs(0), "$a", secs(0), secs(10)),
    ];
    let plain = tx(ops.clone());
    let tagged = Tx {
        via: Some(Box::new(via())),
        ..tx(ops)
    };
    let a = apply_tx(&p, &plain, &mut MemIds::new()).unwrap();
    let b = apply_tx(&p, &tagged, &mut MemIds::new()).unwrap();
    assert_eq!(a, b, "apply ignores via");

    let ra = apply_committed(&p, &plain, &a.bindings).unwrap();
    let rb = apply_committed(&p, &tagged, &a.bindings).unwrap();
    assert_eq!(ra, rb, "replay ignores via");
    assert_eq!(rb.project, a.project);
}
