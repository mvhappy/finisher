//! The builder's duplicate rule and replay (plan 7A Task 10 review M1): redelivered synthetic
//! results and repeated session phases read once; deltas rebuild the exact bodies, checked
//! against `bodyBlake3`, chained only through the turn replayed just before.

use fin_core::events::names::{AGENT_MODELS, AGENT_PROVIDERS, AGENT_ROLES};
use fin_core::events::payloads::*;
use fin_core::events::{AnyPayload, Basis, Code, Content, IdRef, Purposes, Source};
use fin_dataset_inputs::{Disk, ReplayTurn, Store, dedup, entries, hash_of, replay};
use fin_journal::{Consent, Journal, JournalOptions};
use serde_json::{Value, json};

const SESSION: &str = "01JS0000000000000000000001";

fn id(s: &str) -> IdRef {
    IdRef::classify(s).unwrap()
}

fn phase(p: SessionPhase, reason: Option<&str>) -> AnyPayload {
    AgentSession {
        session_id: id(SESSION),
        phase: p,
        harness: Harness::ClaudeCode,
        provider: AGENT_PROVIDERS.get("anthropic").unwrap(),
        model: AGENT_MODELS.get("claude-opus-5-5").unwrap(),
        model_hash: None,
        weights: Weights::Closed,
        sign_in: None,
        role: AGENT_ROLES.get("editor").unwrap(),
        mode: AgentMode::Auto,
        parent: None,
        trigger: SessionTrigger::User,
        reason: reason.map(|r| Code::parse(r).unwrap()),
        totals: None,
    }
    .into()
}

fn synthetic(call: &str, text: &str) -> AnyPayload {
    AgentToolResult {
        call_id: id(call),
        ok: false,
        code: Some(Code::parse("SESSION_INTERRUPTED").unwrap()),
        exit: None,
        micros: 0,
        tx_ids: vec![],
        attribution: Attribution::None,
        provenance: None,
        truncated: false,
        output: Some(Content(Body::Inline(json!({ "text": text })))),
    }
    .into()
}

#[test]
fn redelivered_synthetic_results_and_phases_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let on = Consent {
        purposes: Purposes::ALL,
        rev: 1,
        basis: Basis::User,
    };
    let traces = Journal::open(
        &dir.path().join("agent-traces"),
        on,
        JournalOptions::agent_traces(),
    );
    let c1 = "01JC0000000000000000000001";
    let c2 = "01JC0000000000000000000002";
    // The outbox redelivers after a crash: `interrupted` and each synthetic result twice.
    for p in [
        phase(SessionPhase::Started, None),
        phase(SessionPhase::Interrupted, Some("SERVER_RESTART")),
        synthetic(c1, "first"),
        phase(SessionPhase::Interrupted, Some("SERVER_RESTART")),
        synthetic(c1, "again"),
        synthetic(c2, "first"),
        synthetic(c2, "again"),
        phase(SessionPhase::Resumed, None),
        phase(SessionPhase::Interrupted, Some("SERVER_RESTART")),
    ] {
        traces.record(Source::Agent, None, p);
    }
    traces.close();
    let all = entries(&Disk, dir.path(), Store::AgentTraces).unwrap();
    assert_eq!(all.len(), 9);
    let kept = dedup(all);
    let seen: Vec<(String, Value)> = kept
        .iter()
        .map(|e| {
            let p = &e.payload;
            let what = p.get("phase").or_else(|| p.get("output")).cloned().unwrap();
            (e.kind.as_str().to_owned(), what)
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("agent.session".into(), json!("started")),
            ("agent.session".into(), json!("interrupted")),
            (
                "agent.toolResult".into(),
                json!({ "inline": { "text": "first" } })
            ),
            (
                "agent.toolResult".into(),
                json!({ "inline": { "text": "first" } })
            ),
            ("agent.session".into(), json!("resumed")),
            // A later interruption is a new phase, not a repeat.
            ("agent.session".into(), json!("interrupted")),
        ]
    );
}

// The replay ---------------------------------------------------------------------------------

const SYSTEM: &str = r#"{"role":"system","content":"S"}"#;
const TOOLS: &str = r#"[{"name":"fin","parameters":{"type":"object"}}]"#;
const M1: &str = r#"{"role":"user","content":"a"}"#;
const M2: &str = r#"{"role":"assistant","z":1,"content":"b"}"#;
const M3: &str = r#"{"role":"user","content":"c"}"#;

/// A body as the client sends it: `model`, `messages` (system first), `tools`, `stream`.
fn body(messages: &[&str]) -> String {
    format!(
        r#"{{"model":"m","messages":[{SYSTEM},{}],"tools":{TOOLS},"stream":true}}"#,
        messages.join(",")
    )
}

fn delta(from: u64, base: Option<&str>, messages: &[&str]) -> Value {
    let mut d = json!({
        "v": 1, "from": from, "messages": format!("[{}]", messages.join(",")),
        "keys": ["model", "messages", "tools", "stream"], "rest": r#"{"model":"m","stream":true}"#,
    });
    if let Some(b) = base {
        d["baseTurnId"] = json!(b);
    }
    d
}

fn turn<'a>(turn_id: &'a str, delta: &'a Value, hash: Option<&'a str>) -> ReplayTurn<'a> {
    ReplayTurn {
        turn_id,
        delta,
        system: SYSTEM,
        tools: TOOLS,
        body_blake3: hash,
    }
}

#[test]
fn replay_rebuilds_exact_bodies_and_drops_what_does_not_check() {
    let b1 = body(&[M1]);
    let b2 = body(&[M1, M2, M3]);
    let (h1, h2) = (hash_of(&b1), hash_of(&b2));
    let d1 = delta(0, None, &[M1]);
    let d2 = delta(1, Some("T1"), &[M2, M3]);
    // In order: both rebuild byte for byte (the message key order kept).
    let out = replay(&[turn("T1", &d1, Some(&h1)), turn("T2", &d2, Some(&h2))]);
    assert_eq!(out, [Some(b1.clone()), Some(b2.clone())]);
    // A wrong hash drops the turn, and the turn based on it.
    let bad = hash_of("other");
    let out = replay(&[turn("T1", &d1, Some(&bad)), turn("T2", &d2, Some(&h2))]);
    assert_eq!(out, [None, None]);
    // No hash: dropped too.
    assert_eq!(replay(&[turn("T1", &d1, None)]), [None]);
    // A base other than the turn replayed just before (T1 was not recorded): dropped; a later
    // turn that stands alone replays again.
    let d3 = delta(0, None, &[M1, M2, M3]);
    let out = replay(&[turn("T2", &d2, Some(&h2)), turn("T3", &d3, Some(&h2))]);
    assert_eq!(out, [None, Some(b2.clone())]);
    let d2_other = delta(1, Some("T0"), &[M2, M3]);
    let out = replay(&[turn("T1", &d1, Some(&h1)), turn("T2", &d2_other, Some(&h2))]);
    assert_eq!(out, [Some(b1.clone()), None]);
    // A raw body replays as itself and is the next delta's base.
    let raw = json!({ "v": 1, "raw": b1 });
    let out = replay(&[turn("T1", &raw, Some(&h1)), turn("T2", &d2, Some(&h2))]);
    assert_eq!(out, [Some(b1), Some(b2)]);
}

#[test]
fn replay_matches_finisherds_deltas() {
    // `apps/finisherd/test/agent-journal-review.test.ts` writes this fixture from finisherd's
    // own `splitRequest` (and checks it is current): the Rust replay rebuilds the same bytes.
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/replay.json"),
    )
    .unwrap();
    let fixture: Value = serde_json::from_str(&text).unwrap();
    let turns = fixture["turns"].as_array().unwrap();
    let input: Vec<ReplayTurn<'_>> = turns
        .iter()
        .map(|t| ReplayTurn {
            turn_id: t["turnId"].as_str().unwrap(),
            delta: &t["delta"],
            system: t["system"].as_str().unwrap(),
            tools: t["tools"].as_str().unwrap(),
            body_blake3: t["bodyBlake3"].as_str(),
        })
        .collect();
    let want: Vec<Option<String>> = turns
        .iter()
        .map(|t| Some(t["body"].as_str().unwrap().to_owned()))
        .collect();
    assert_eq!(turns.len(), 4);
    assert_eq!(replay(&input), want);
    // Past the first, each delta is based on the turn before it.
    assert!(turns[1]["delta"]["baseTurnId"] == turns[0]["turnId"]);
}
