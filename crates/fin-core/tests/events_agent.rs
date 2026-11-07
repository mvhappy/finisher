//! The agent kinds, version 1 (sub-project 7A Task 1; spec §12.2): versions, round trips,
//! redaction, blobs, the agent name tables, provenance and the txId cap.

use fin_core::events::class::{CONTENT, is_valid_name};
use fin_core::events::names::{
    AGENT_MODELS, AGENT_PROVIDERS, AGENT_ROLES, AGENT_TOOLS, CLI_COMMANDS, MEDIA_TYPES,
};
use fin_core::events::payloads::*;
use fin_core::events::*;
use serde_json::{Value, json};

const SESSION: &str = "01JC0000000000000000000003";
const TX: &str = "01JC0000000000000000000002";

fn key(data: &[u8]) -> [u8; 16] {
    let full = blake3::keyed_hash(&[9; 32], data);
    full.as_bytes()[..16].try_into().unwrap()
}

fn h(c: char) -> Hashed {
    Hashed::parse(&format!("k1:{}", c.to_string().repeat(32))).unwrap()
}

fn id(s: &str) -> IdRef {
    IdRef::classify(s).unwrap()
}

fn blob(b: u8) -> BlobRef {
    BlobRef {
        hash: BlobHash::from_bytes(&[b; 32]),
        bytes: 5000 + u64::from(b),
        media_type: MEDIA_TYPES.get("application/json").unwrap(),
    }
}

fn inline(v: Value) -> Option<Content<Body>> {
    Some(Content(Body::Inline(v)))
}

fn blobbed(b: u8) -> Option<Content<Body>> {
    Some(Content(Body::Blob(blob(b))))
}

fn session() -> AgentSession {
    AgentSession {
        session_id: id(SESSION),
        phase: SessionPhase::Started,
        harness: Harness::Builtin,
        provider: AGENT_PROVIDERS.get("openrouter").unwrap(),
        model: AGENT_MODELS.get("claude-sonnet-5-5").unwrap(),
        model_hash: None,
        weights: Weights::Closed,
        sign_in: Some(SignIn::Key),
        role: AGENT_ROLES.get("editor").unwrap(),
        mode: AgentMode::Auto,
        parent: Some(id("01JC0000000000000000000009")),
        trigger: SessionTrigger::Subagent,
        reason: None,
        totals: None,
    }
}

fn turn() -> AgentTurn {
    AgentTurn {
        turn_id: id("01JC0000000000000000000005"),
        run_id: id("01JC0000000000000000000007"),
        seq: 0,
        parent_turn_id: Some(id("01JC0000000000000000000004")),
        harness: Harness::Builtin,
        provider: AGENT_PROVIDERS.get("openrouter").unwrap(),
        model: Name::OTHER,
        model_hash: Some(h('3')),
        weights: Weights::Open,
        upstream: Some(AGENT_PROVIDERS.get("mistral").unwrap()),
        input_tokens: 9000,
        output_tokens: 120,
        cache_read_tokens: 8000,
        cache_write_tokens: 900,
        reasoning_tokens: 40,
        latency_ms: 1800,
        ttft_ms: None,
        stop_reason: StopReason::EndTurn,
        cost_micros: Some(310),
        cost_basis: CostBasis::Reported,
        capture: Capture::Exact,
        system_hash: h('e'),
        tools_hash: h('f'),
        body_hash: Some(h('b')),
        messages_delta: inline(json!([{ "role": "user", "content": "tighten the intro" }])),
        output: blobbed(2),
        system: blobbed(3),
        tools: blobbed(4),
    }
}

fn tool_call() -> AgentToolCall {
    AgentToolCall {
        call_id: id("01JC0000000000000000000006"),
        turn_id: id("01JC0000000000000000000005"),
        tool: AGENT_TOOLS.get("workspace").unwrap(),
        command: None,
        args_hash: h('a'),
        args: blobbed(5),
    }
}

fn tool_result(tx_ids: usize) -> AgentToolResult {
    AgentToolResult {
        call_id: id("01JC0000000000000000000006"),
        ok: false,
        code: Some(Code::parse("SESSION_INTERRUPTED").unwrap()),
        exit: Some(-1),
        micros: 5,
        tx_ids: (0..tx_ids).map(|_| id(TX)).collect(),
        attribution: Attribution::Ambiguous,
        truncated: true,
        output: blobbed(6),
    }
}

fn approval() -> AgentApproval {
    AgentApproval {
        approval_id: id("01JC0000000000000000000008"),
        session_id: id(SESSION),
        kind: ApprovalKind::Rule,
        decision: ApprovalDecision::Expired,
        by: DecidedBy::Policy,
        latency_ms: 90_000,
        subject: blobbed(7),
    }
}

fn agent_samples() -> Vec<AnyPayload> {
    vec![
        session().into(),
        turn().into(),
        tool_call().into(),
        tool_result(3).into(),
        approval().into(),
    ]
}

#[test]
fn agent_kinds_are_version_1() {
    for name in [
        "agent.session",
        "agent.turn",
        "agent.toolCall",
        "agent.toolResult",
        "agent.approval",
    ] {
        let k = kind(name, 1).unwrap_or_else(|| panic!("{name} at 1"));
        assert!(!k.reserved, "{name}");
        assert!(kind(name, 0).is_none(), "{name} still at 0");
        assert_eq!(k.sources, [Source::Agent], "{name}");
        let training = matches!(name, "agent.turn" | "agent.toolCall" | "agent.toolResult");
        assert_eq!(k.training_only, training, "{name}");
    }
    assert!(kinds().iter().all(|k| k.name != "agent.editVerdict"));
    assert_eq!(
        require_kind("agent.editVerdict", 0).unwrap_err().code,
        "EVENT_KIND_UNKNOWN"
    );
}

#[test]
fn agent_kinds_round_trip() {
    for p in agent_samples() {
        let spec = p.kind();
        let v = serde_json::to_value(&p).unwrap();
        let back = decode_payload(spec, &v).unwrap_or_else(|e| panic!("{}: {e}", spec.name));
        assert_eq!(back, p, "{}", spec.name);
        assert_eq!(serde_json::to_value(&back).unwrap(), v, "{}", spec.name);
        let received = validate_payload(spec, &v, &key, &ScrubCtx::default()).unwrap();
        assert_eq!(received, p, "{}", spec.name);
    }
    // Inline and blob bodies, both ways round.
    let mut t = turn();
    t.messages_delta = blobbed(1);
    t.output = inline(json!([{ "type": "text", "text": "done" }]));
    let p = AnyPayload::from(t);
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(decode_payload(p.kind(), &v).unwrap(), p);
    // Optional fields are omitted, never null.
    let v = serde_json::to_value(AnyPayload::from(session())).unwrap();
    for absent in ["modelHash", "reason", "totals"] {
        assert!(v.get(absent).is_none(), "{absent}");
    }
    let mut r = tool_result(0);
    r.truncated = false;
    let v = serde_json::to_value(AnyPayload::from(r)).unwrap();
    assert!(v.get("txIds").is_none() && v.get("truncated").is_none());
}

#[test]
fn redact_drops_agent_content() {
    let want: [(&str, &[&str]); 5] = [
        ("agent.session", &[]),
        (
            "agent.turn",
            &["messagesDelta", "output", "system", "tools"],
        ),
        ("agent.toolCall", &["args"]),
        ("agent.toolResult", &["output"]),
        ("agent.approval", &["subject"]),
    ];
    for (p, (name, content)) in agent_samples().into_iter().zip(want) {
        let spec = p.kind();
        assert_eq!(spec.name, name);
        let full = serde_json::to_value(&p).unwrap();
        let mut v = full.clone();
        redact(spec, &mut v, Purposes::DIAGNOSTICS);
        for c in content {
            assert!(full.get(*c).is_some(), "{name}.{c} was filled");
            assert!(v.get(*c).is_none(), "{name}.{c} survived");
        }
        let kept = full.as_object().unwrap().len() - content.len();
        assert_eq!(v.as_object().unwrap().len(), kept, "{name}");
        // The typed twin agrees.
        let mut typed = p.clone();
        typed.strip_content();
        assert_eq!(serde_json::to_value(&typed).unwrap(), v, "{name}");
        // Training keeps everything.
        let mut all = full.clone();
        redact(spec, &mut all, Purposes::ALL);
        assert_eq!(all, full, "{name}");
    }
    let mut v = serde_json::to_value(AnyPayload::from(turn())).unwrap();
    redact(
        kind("agent.turn", 1).unwrap(),
        &mut v,
        Purposes::DIAGNOSTICS,
    );
    for kept in [
        "systemHash",
        "toolsHash",
        "bodyHash",
        "inputTokens",
        "reasoningTokens",
    ] {
        assert!(v.get(kept).is_some(), "{kept}");
    }
    let mut v = serde_json::to_value(AnyPayload::from(tool_call())).unwrap();
    redact(
        kind("agent.toolCall", 1).unwrap(),
        &mut v,
        Purposes::DIAGNOSTICS,
    );
    assert!(v.get("argsHash").is_some());
}

#[test]
fn blob_refs_listed() {
    let want: [Vec<BlobRef>; 5] = [
        vec![],
        vec![blob(2), blob(3), blob(4)],
        vec![blob(5)],
        vec![blob(6)],
        vec![blob(7)],
    ];
    for (p, want) in agent_samples().into_iter().zip(want) {
        let spec = p.kind();
        assert_eq!(
            p.blobs().cloned().collect::<Vec<_>>(),
            want,
            "{}",
            spec.name
        );
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(blob_refs(spec, &v), want, "{}", spec.name);
    }
}

#[test]
fn agent_tables_match_patterns() {
    for t in [&AGENT_PROVIDERS, &AGENT_MODELS, &AGENT_ROLES, &AGENT_TOOLS] {
        for e in t.entries() {
            assert!(is_valid_name(e), "{}: {e}", t.table);
        }
        let mut sorted = t.entries().to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), t.entries().len(), "{} repeats", t.table);
        assert!(
            names::TABLES.iter().any(|x| std::ptr::eq(*x, t)),
            "{}",
            t.table
        );
    }
}

#[test]
fn roles_and_tools_tables_have_other() {
    for t in [&AGENT_ROLES, &AGENT_TOOLS, &AGENT_PROVIDERS, &AGENT_MODELS] {
        assert_eq!(t.entries().last(), Some(&"other"), "{}", t.table);
    }
    assert!(AGENT_ROLES.get("editor").is_some() && AGENT_ROLES.get("critic").is_some());
    assert_eq!(AGENT_ROLES.get_or_other("plugin-role"), Name::OTHER);
    assert_eq!(AGENT_TOOLS.get_or_other("bash"), Name::OTHER);
    // A value outside the table decodes as `other`.
    let mut v = serde_json::to_value(AnyPayload::from(tool_call())).unwrap();
    v["tool"] = json!("teleport");
    v["command"] = json!("not.a.command");
    let back = decode_payload(kind("agent.toolCall", 1).unwrap(), &v).unwrap();
    let AnyPayload::AgentToolCall(c) = back else {
        panic!("kind")
    };
    assert_eq!((c.tool, c.command), (Name::OTHER, Some(Name::OTHER)));
    assert_eq!(CLI_COMMANDS.get("item.trim").unwrap().as_str(), "item.trim");
}

#[test]
fn agent_providers_has_openrouter() {
    for p in ["openrouter", "local", "anthropic", "openai"] {
        assert!(AGENT_PROVIDERS.get(p).is_some(), "{p}");
    }
    let mut v = serde_json::to_value(AnyPayload::from(turn())).unwrap();
    v["upstream"] = json!("some-new-host");
    let AnyPayload::AgentTurn(t) = decode_payload(kind("agent.turn", 1).unwrap(), &v).unwrap()
    else {
        panic!("kind")
    };
    assert_eq!(t.upstream, Some(Name::OTHER));
}

#[test]
fn provenance_on_every_session_and_turn() {
    for (name, extra) in [
        ("agent.session", &[][..]),
        ("agent.turn", &["upstream"][..]),
    ] {
        let schema = (kind(name, 1).unwrap().schema)();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap())
            .collect();
        for field in ["harness", "provider", "model", "weights"]
            .iter()
            .chain(extra)
        {
            let prop = &schema["properties"][*field];
            assert!(!prop.is_null(), "{name}.{field} missing");
            assert_ne!(prop["x-fin-class"], json!(CONTENT), "{name}.{field}");
            if *field != "upstream" {
                assert!(required.contains(field), "{name}.{field} optional");
            }
        }
        // Removing any of them makes the payload invalid.
        let full = serde_json::to_value(match name {
            "agent.session" => AnyPayload::from(session()),
            _ => AnyPayload::from(turn()),
        })
        .unwrap();
        for field in ["harness", "provider", "model", "weights"] {
            let mut v = full.clone();
            v.as_object_mut().unwrap().remove(field);
            let spec = kind(name, 1).unwrap();
            assert_eq!(
                decode_payload(spec, &v).unwrap_err().code,
                "EVENT_INVALID",
                "{name} without {field}"
            );
        }
        // They survive diagnostics-only redaction.
        let mut v = full.clone();
        redact(kind(name, 1).unwrap(), &mut v, Purposes::DIAGNOSTICS);
        for field in ["harness", "provider", "model", "weights"]
            .iter()
            .chain(extra)
        {
            assert_eq!(v[*field], full[*field], "{name}.{field}");
        }
    }
    let v = serde_json::to_value(AnyPayload::from(turn())).unwrap();
    assert_eq!(
        (&v["harness"], &v["provider"], &v["weights"], &v["upstream"]),
        (
            &json!("builtin"),
            &json!("openrouter"),
            &json!("open"),
            &json!("mistral")
        )
    );
}

#[test]
fn tx_ids_capped_at_64() {
    let spec = kind("agent.toolResult", 1).unwrap();
    let ok = serde_json::to_value(AnyPayload::from(tool_result(MAX_TOOL_TX_IDS))).unwrap();
    validate_payload(spec, &ok, &key, &ScrubCtx::default()).unwrap();
    let over = serde_json::to_value(AnyPayload::from(tool_result(MAX_TOOL_TX_IDS + 1))).unwrap();
    assert_eq!(
        validate_payload(spec, &over, &key, &ScrubCtx::default())
            .unwrap_err()
            .code,
        "EVENT_INVALID"
    );
    assert_eq!(
        decode_payload(spec, &over).unwrap_err().code,
        "EVENT_INVALID"
    );
    let schema = (spec.schema)();
    assert_eq!(schema["properties"]["txIds"]["maxItems"], json!(64));
}

/// `AGENT_MODELS` is the bundled catalogue's ids plus `other` (sub-project 7 §4.7).
#[test]
fn catalogue_ids_in_agent_models() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/finisherd/src/agent/catalogue/models.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut ids: Vec<&str> = doc["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    ids.push("other");
    assert_eq!(AGENT_MODELS.entries(), ids.as_slice());
}
