//! `review.verdict` v1 (5E Task 4, Decision 20): the kind, its golden, the 64-id cap, the
//! training-only `reason`, and the shape 7A's outcome tracker sends.

#[path = "events_samples/mod.rs"]
mod samples;

use std::path::Path;

use fin_core::events::payloads::*;
use fin_core::events::*;
use serde_json::{Value, json};

use samples::{SESSION, TX, key, review_verdict};

fn spec() -> &'static KindSpec {
    kind("review.verdict", 1).expect("review.verdict v1")
}

#[test]
fn review_verdict_v1_golden() {
    let k = spec();
    assert!(!k.reserved && !k.training_only);
    assert_eq!(k.sources, [Source::Finisherd, Source::Agent]);
    assert!(kind("review.verdict", 0).is_none());
    let actual = serde_json::to_string_pretty(&review_verdict()).unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/events_golden/review_verdict.json");
    if std::env::var("FIN_EVENTS_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, format!("{actual}\n")).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap();
    assert_eq!(actual, want.trim_end(), "review_verdict drifted");
    let v: Value = serde_json::from_str(&actual).unwrap();
    assert_eq!(decode_payload(k, &v).unwrap(), review_verdict().into());
    // The schema bounds the list too.
    assert_eq!((k.schema)()["properties"]["txIds"]["maxItems"], json!(64));
}

fn wire(tx_ids: Vec<String>) -> Value {
    json!({
        "itemKind": "none", "verdict": "merged", "via": "merge", "by": "agent",
        "txIds": tx_ids, "accepted": 1, "rejected": 0, "sessionId": SESSION, "openS": 5,
    })
}

#[test]
fn tx_ids_capped_at_64() {
    let ctx = ScrubCtx::default();
    let ulid = |n: usize| format!("01JC{n:022}");
    for n in [0, 1, 64] {
        let v = wire((0..n).map(ulid).collect());
        assert!(validate_payload(spec(), &v, &key, &ctx).is_ok(), "{n}");
    }
    let over = wire((0..65).map(ulid).collect());
    assert_eq!(
        validate_payload(spec(), &over, &key, &ctx)
            .unwrap_err()
            .code,
        "EVENT_INVALID"
    );
    assert!(decode_payload(spec(), &over).is_err());
    // A raw idempotency key is keyed, not refused.
    let raw = wire(vec!["my-key".into()]);
    let AnyPayload::ReviewVerdict(p) = validate_payload(spec(), &raw, &key, &ctx).unwrap() else {
        panic!("review.verdict")
    };
    assert!(p.tx_ids[0].as_str().starts_with("k1:"));
}

#[test]
fn reason_is_training_only() {
    let p: AnyPayload = review_verdict().into();
    let full = serde_json::to_value(&p).unwrap();
    assert_eq!(full["reason"], json!("too loud"));
    for purposes in [Purposes::DIAGNOSTICS, Purposes::NONE] {
        let mut v = full.clone();
        redact(spec(), &mut v, purposes);
        assert!(v.get("reason").is_none(), "{purposes:?}");
        assert_eq!(v["txIds"], full["txIds"]);
        assert_eq!(v["verdict"], json!("rejected"));
        decode_payload(spec(), &v).unwrap();
    }
    let mut v = full.clone();
    redact(spec(), &mut v, Purposes::ALL);
    assert_eq!(v, full);
    // The typed twin agrees.
    let mut typed = p.clone();
    typed.strip_content();
    let mut redacted = full.clone();
    redact(spec(), &mut redacted, Purposes::DIAGNOSTICS);
    assert_eq!(serde_json::to_value(&typed).unwrap(), redacted);
}

#[test]
fn outcome_verdict_shape_decodes() {
    // 7A's `OutcomeVerdict`, as `apps/finisherd/src/agent/outcomes.ts` builds it.
    let v = json!({
        "verdict": "undone", "txIds": [TX], "by": "user", "via": "undo",
        "itemKind": "none", "sessionId": SESSION, "openS": 42,
    });
    let AnyPayload::ReviewVerdict(p) =
        validate_payload(spec(), &v, &key, &ScrubCtx::default()).unwrap()
    else {
        panic!("review.verdict")
    };
    assert_eq!(
        (p.item_kind, p.verdict),
        (ReviewItemKind::None, Verdict::Undone)
    );
    assert_eq!((p.by, p.via), (VerdictBy::User, Some(VerdictVia::Undo)));
    assert_eq!((p.accepted, p.rejected), (0, 0));
}
