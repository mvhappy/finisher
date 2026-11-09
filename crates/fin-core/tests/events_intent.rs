//! Edit intents and wire forms (plan D, Decisions 6, 7, 22, 24 and 26): nothing free-text,
//! path-like or unkeyed survives into an intent or a received entry; intent sizes are exact.

use fin_core::Tx;
use fin_core::events::intent::MAX_INTENT_TXS;
use fin_core::events::payloads::*;
use fin_core::events::*;
use serde_json::{Value, json};

const ULID_TX: &str = "01JC0000000000000000000002";

fn key(data: &[u8]) -> [u8; 16] {
    let full = blake3::keyed_hash(&[9; 32], data);
    full.as_bytes()[..16].try_into().unwrap()
}

fn args_hash() -> String {
    format!("blake3:{}", "ab".repeat(32))
}

fn tx_from(v: Value) -> Tx {
    serde_json::from_value(v).unwrap()
}

/// A request tx as `fin` sends one: an idempotency key as its txId, a free-text author, a
/// branch name, an `argsHash`, and an `asset.add` of a file under the user's home.
fn raw_tx() -> Tx {
    tx_from(json!({
        "txId": "idem-key-alice-laptop-7f3a", "author": "Alice Smith (alice@alice-laptop)",
        "branch": "feature-secret", "baseRev": 3, "schemaVersion": 2,
        "argsHash": args_hash(),
        "ops": [
            { "op": "asset.add", "id": "$a", "path": "/home/alice/Secret Project/Clip.MP4",
              "fingerprint": "f1", "duration": "10" },
            { "op": "item.trim", "item": "c1", "edge": "start", "delta": "1", "mode": "ripple" },
        ],
    }))
}

fn trims(n: usize) -> Tx {
    let ops: Vec<Value> = (0..n)
        .map(|i| json!({ "op": "item.trim", "item": format!("c{i}"), "edge": "end", "delta": "1/2", "mode": "ripple" }))
        .collect();
    tx_from(json!({
        "txId": ULID_TX, "author": "a", "branch": "br12", "baseRev": 1, "schemaVersion": 2,
        "ops": ops,
    }))
}

#[test]
fn intent_never_keeps_author_tx_id_args_hash_or_path() {
    let raw = raw_tx();
    let path = "/home/alice/Secret Project/Clip.MP4";
    let intent = IntentTx::from_tx(raw, &key);
    let text = serde_json::to_string(&intent).unwrap();
    for leak in [
        "idem", "alice", "Alice", "Smith", "Secret", "home", "Clip", "blake3", "feature",
    ] {
        assert!(!text.contains(leak), "{leak} survived: {text}");
    }
    assert!(intent.tx_id().is_hashed());
    assert_eq!(
        intent.tx_id(),
        &IdRef::from_hashed(Hashed::keyed(b"idem-key-alice-laptop-7f3a", &key))
    );
    assert_eq!(
        intent.author(),
        &Hashed::keyed(b"Alice Smith (alice@alice-laptop)", &key)
    );
    assert_eq!(
        intent.args_hash(),
        Some(&Hashed::keyed(args_hash().as_bytes(), &key))
    );
    assert_eq!(intent.branch(), None, "a branch name is not kept");
    assert_eq!((intent.base_rev(), intent.schema_version()), (3, 2));
    let fin_core::Op::AssetAdd(add) = &intent.ops()[0] else {
        panic!()
    };
    let token = PathRef::from_token(&add.path).expect("a path token");
    assert_eq!(token.root, PathRoot::Other);
    assert_eq!(token.ext.map(|e| e.as_str()), Some("mp4"));
    assert_eq!(token.hash, Hashed::keyed(path.as_bytes(), &key));
    assert_eq!(token.token(), add.path);
    assert_eq!(intent.ops()[1].name(), "item.trim");
    // A canonical ULID txId and a branch id are kept as they are.
    let kept = IntentTx::from_tx(trims(1), &key);
    assert_eq!(kept.tx_id().as_str(), ULID_TX);
    assert_eq!(kept.branch().map(BranchId::as_str), Some("br12"));
    assert_eq!(kept.args_hash(), None);
    // The intent's argsHash joins with `cli.command`'s.
    let cli = CliCommandIn {
        name: names::CLI_COMMANDS.get_or_other("asset.add"),
        flags: vec![],
        args_hash: Blake3Hex::parse(&args_hash()),
        micros: 1,
        exit: 0,
        code: None,
        online: true,
        dry_run: false,
        tx_ids: vec![],
    }
    .into_payload(&key, &ScrubCtx::default());
    assert_eq!(cli.args_hash.as_ref(), intent.args_hash());
}

#[test]
fn path_tokens() {
    let p = PathRef::of_path("media/a.WAV", &key);
    assert_eq!(
        (p.root, p.ext.map(|e| e.as_str())),
        (PathRoot::Project, Some("wav"))
    );
    let q = PathRef::of_path("C:\\Users\\bob\\a.exe", &key);
    assert_eq!((q.root, q.ext), (PathRoot::Other, None));
    assert_eq!(PathRef::from_token(&q.token()), Some(q.clone()));
    assert!(q.token().starts_with("<other>/k1:") && !q.token().contains('.'));
    for raw in [
        "/home/alice/a.mp4",
        "<other>/k1:zz.mp4",
        "<nope>/k1:00000000000000000000000000000000",
        "<other>/k1:00000000000000000000000000000000.exe",
        "",
    ] {
        assert_eq!(PathRef::from_token(raw), None, "{raw}");
    }
}

#[test]
fn intent_decode_refuses_a_raw_path() {
    let intent = IntentTx::from_tx(raw_tx(), &key);
    let mut v = serde_json::to_value(&intent).unwrap();
    let back: IntentTx = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(back, intent);
    assert_eq!(back.bytes(), intent.bytes());
    v["ops"][0]["path"] = json!("/home/alice/a.mp4");
    assert!(serde_json::from_value::<IntentTx>(v.clone()).is_err());
    let spec = kind("log.commit", 2).unwrap();
    let payload = json!({ "record": "edit", "outcome": "accepted", "micros": 1, "intent": [v] });
    assert_eq!(
        decode_payload(spec, &payload).unwrap_err().code,
        "EVENT_INVALID"
    );
}

#[test]
fn intent_bytes_are_exact() {
    for txs in [vec![], vec![raw_tx()], vec![raw_tx(), trims(3), trims(40)]] {
        let intent = TxIntent::from_txs(txs, &key);
        let n = serde_json::to_string(&intent).unwrap().len();
        assert_eq!(intent.bytes(), n);
        for tx in &intent.0 {
            assert_eq!(tx.bytes(), serde_json::to_string(tx).unwrap().len());
        }
        let read: TxIntent =
            serde_json::from_str(&serde_json::to_string(&intent).unwrap()).unwrap();
        assert_eq!(read.bytes(), n);
    }
    let many = TxIntent::from_txs((0..MAX_INTENT_TXS + 5).map(|_| trims(1)), &key);
    assert_eq!(many.0.len(), MAX_INTENT_TXS);
}

fn commit() -> LogCommit {
    LogCommit {
        record: CommitRecord::Edit,
        outcome: CommitOutcome::Accepted,
        code: None,
        ops: Counts::default(),
        base_rev: Some(1),
        rebased_over: None,
        conflicts: Counts::default(),
        links: vec![],
        micros: 9,
        intent: None,
        intent_omitted: None,
    }
}

#[test]
fn attach_intent_drops_an_oversized_intent() {
    let mut small = commit();
    assert!(small.attach_intent(TxIntent::from_txs([trims(2)], &key), 300));
    assert!(small.intent.is_some() && small.intent_omitted.is_none());
    let entry = serde_json::to_string(&AnyPayload::from(small.clone())).unwrap();
    assert!(entry.len() + 300 <= MAX_INTENT_ENTRY_BYTES);

    let big = TxIntent::from_txs([trims(600)], &key);
    assert!(big.bytes() > MAX_INTENT_ENTRY_BYTES);
    let mut large = commit();
    assert!(!large.attach_intent(big, 300));
    assert_eq!(large.intent, None);
    assert_eq!(large.intent_omitted, Some(IntentOmitted::TooLarge));

    // Under the cap alone; over it with the rest of the entry.
    let edge = TxIntent::from_txs([trims(200)], &key);
    let mut fits = commit();
    assert!(fits.attach_intent(edge.clone(), 0));
    let mut over = commit();
    assert!(!over.attach_intent(edge.clone(), MAX_INTENT_ENTRY_BYTES - edge.bytes()));
    // The payload's own size counts the intent exactly.
    let p = AnyPayload::from(fits);
    let n = serde_json::to_string(&p).unwrap().len();
    assert!(p.approx_bytes() + 64 >= n && p.approx_bytes() <= n + 400);
}

#[test]
fn wire_cli_command_is_keyed() {
    let spec = kind("cli.command", 1).unwrap();
    let sent = json!({
        "name": "item.trim", "flags": ["idem-key", "dry-run", "dry-run", "--secret"],
        "argsHash": args_hash(), "micros": 5, "exit": 0, "online": true, "dryRun": true,
        "txIds": [ULID_TX, "idem-key-alice"],
    });
    let AnyPayload::CliCommand(c) =
        validate_payload(spec, &sent, &key, &ScrubCtx::default()).unwrap()
    else {
        panic!()
    };
    assert_eq!(c.name.as_str(), "item.trim");
    let flags: Vec<&str> = c.flags.iter().map(Name::as_str).collect();
    assert_eq!(flags, ["dry-run", "idem-key", "other"]);
    assert_eq!(
        c.args_hash,
        Some(Hashed::keyed(args_hash().as_bytes(), &key))
    );
    assert_eq!(c.tx_ids[0].as_str(), ULID_TX);
    assert_eq!(
        c.tx_ids[1],
        IdRef::from_hashed(Hashed::keyed(b"idem-key-alice", &key))
    );
    let stored = serde_json::to_string(&*c).unwrap();
    assert!(
        !stored.contains("blake3") && !stored.contains("alice"),
        "{stored}"
    );
    // The stored form round-trips through the stored decode, not through the wire.
    let v = serde_json::to_value(&*c).unwrap();
    assert_eq!(decode_payload(spec, &v).unwrap(), AnyPayload::CliCommand(c));
    assert_eq!(
        validate_payload(spec, &v, &key, &ScrubCtx::default())
            .unwrap_err()
            .code,
        "EVENT_INVALID",
        "a keyed argsHash is not a wire argsHash"
    );
    let mut long = sent.clone();
    long["txIds"] = json!(["x".repeat(MAX_RAW_ID + 1)]);
    assert!(validate_payload(spec, &long, &key, &ScrubCtx::default()).is_err());
    assert_eq!(RawId::new(&"x".repeat(MAX_RAW_ID)).map(|_| ()), Some(()));
}

#[test]
fn wire_error_panic_rescrubs_and_hashes() {
    let spec = kind("error.panic", 1).unwrap();
    let server = ScrubCtx {
        user: Some("alice".into()),
        host: Some("alice-laptop".into()),
        ..ScrubCtx::default()
    };
    let sent = json!({
        "where": "fin", "location": "crates/fin/src/main.rs:9", "thirdParty": false,
        "thread": "main", "message": "boom at /home/alice/x on alice-laptop",
        "messageHash": format!("k1:{}", "0".repeat(32)),
    });
    let AnyPayload::ErrorPanic(p) = validate_payload(spec, &sent, &key, &server).unwrap() else {
        panic!()
    };
    let message = "boom at <path> on <host>";
    assert_eq!(p.message.as_ref().map(|m| m.0.as_str()), Some(message));
    assert_eq!(p.message_hash, Hashed::keyed(message.as_bytes(), &key));
    assert_eq!(p.at, PanicWhere::Fin);
    assert_eq!(p.thread, PanicThread::Main);
    // The stored form decodes as stored and is scrubbed again on demand (M3).
    let mut stored = decode_payload(spec, &serde_json::to_value(&*p).unwrap()).unwrap();
    stored.rescrub(&ScrubCtx {
        user: Some("boom".into()),
        ..ScrubCtx::default()
    });
    let AnyPayload::ErrorPanic(again) = stored else {
        panic!()
    };
    assert_eq!(
        again.message.unwrap().0.as_str(),
        "<user> at <path> on <host>"
    );
}

#[test]
fn every_kind_fin_sends_has_a_wire_form() {
    let fin: Vec<&str> = kinds()
        .iter()
        .filter(|k| k.sources.contains(&Source::Fin))
        .map(|k| k.name)
        .collect();
    assert!(fin.iter().all(|k| WIRE_KINDS.contains(k)), "{fin:?}");
    // `finisherd` sends the agent kinds through wire forms too (raw model ids and upstreams,
    // unkeyed hashes, and the approvals' provenance; sub-project 7A review H1, H2, M3; Task 10 B1).
    assert_eq!(
        WIRE_KINDS,
        [
            "cli.command",
            "error.panic",
            "agent.session",
            "agent.turn",
            "agent.toolCall",
            "agent.toolResult",
            "agent.approval"
        ]
    );
}
