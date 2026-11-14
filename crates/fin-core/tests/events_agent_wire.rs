//! The agent kinds' wire forms (review H1, H2, M3): model ids and upstreams become a table
//! name or a keyed hash, unkeyed hashes are keyed, raw txIds become `IdRef`s; the upstream
//! normaliser against OpenRouter's provider list and the SSE fixtures; and the agent tables
//! are append-only against a committed snapshot.

use std::collections::BTreeMap;
use std::path::Path;

use fin_core::events::names::{
    self, AGENT_MODELS, AGENT_PROVIDERS, AGENT_ROLES, AGENT_TOOLS, AGENT_UPSTREAMS, NameTable,
    agent_upstream,
};
use fin_core::events::*;
use serde_json::{Value, json};

const SESSION: &str = "01JC0000000000000000000003";
const B3: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000001";

fn key(data: &[u8]) -> [u8; 16] {
    let full = blake3::keyed_hash(&[9; 32], data);
    full.as_bytes()[..16].try_into().unwrap()
}

fn received(kind_name: &str, v: Value) -> AnyPayload {
    let spec = kind(kind_name, 1).unwrap();
    validate_payload(spec, &v, &key, &ScrubCtx::default())
        .unwrap_or_else(|e| panic!("{kind_name}: {e:?}"))
}

fn keyed(s: &str) -> Hashed {
    Hashed::keyed(s.as_bytes(), &key)
}

fn session_in(model: &str) -> Value {
    json!({
        "sessionId": SESSION, "phase": "started", "harness": "builtin", "provider": "openrouter",
        "model": model, "weights": "unknown", "role": "editor", "mode": "draft", "trigger": "user",
    })
}

fn turn_in(model: &str, upstream: Option<&str>) -> Value {
    let mut v = json!({
        "turnId": "01JC0000000000000000000005", "runId": "01JC0000000000000000000007", "seq": 1,
        "harness": "builtin", "provider": "openrouter", "model": model, "weights": "open",
        "inputTokens": 10, "outputTokens": 2, "cacheReadTokens": 0, "cacheWriteTokens": 0,
        "reasoningTokens": 0, "latencyMs": 5, "stopReason": "endTurn", "costBasis": "reported",
        "capture": "exact", "systemHash": B3, "toolsHash": B3, "bodyHash": B3,
    });
    if let Some(u) = upstream {
        v["upstream"] = json!(u);
    }
    v
}

#[test]
fn a_known_model_is_named_and_any_other_is_hashed() {
    let AnyPayload::AgentSession(s) = received("agent.session", session_in("claude-opus-5-5"))
    else {
        panic!("kind")
    };
    assert_eq!((s.model.as_str(), s.model_hash), ("claude-opus-5-5", None));
    // Any id the user typed at OpenRouter, or a local model: `other` plus its keyed hash.
    for id in ["acme/brand-new-model:free", "llama3.3:70b", "other"] {
        let AnyPayload::AgentSession(s) = received("agent.session", session_in(id)) else {
            panic!("kind")
        };
        assert_eq!(s.model, Name::OTHER, "{id}");
        assert_eq!(s.model_hash, Some(keyed(id)), "{id}");
        if id != "other" {
            assert!(
                !serde_json::to_string(&s).unwrap().contains(id),
                "{id} stored raw"
            );
        }
    }
    // Too long for a raw id.
    let spec = kind("agent.session", 1).unwrap();
    let long = session_in(&"m".repeat(300));
    assert_eq!(
        validate_payload(spec, &long, &key, &ScrubCtx::default())
            .unwrap_err()
            .code,
        "EVENT_INVALID"
    );
}

#[test]
fn trace_copies_carry_provenance_keyed_and_stream_turns_have_no_prompt_hashes() {
    // Task 10 review B1: the agent-trace store stamps the session's provenance on its calls,
    // results and approvals; a model id is named or hashed as on a session, never raw.
    let prov = json!({ "harness": "claudeCode", "provider": "anthropic", "model": "acme/x", "weights": "closed" });
    let call = json!({ "callId": "01JC0000000000000000000006", "turnId": "01JC0000000000000000000005",
        "tool": "fin", "argsHash": B3, "provenance": prov });
    let result = json!({ "callId": "01JC0000000000000000000006", "ok": true, "micros": 1,
        "attribution": "inferred", "provenance": prov });
    let approval = json!({ "approvalId": "01JC0000000000000000000008", "sessionId": SESSION,
        "kind": "rule", "decision": "allow", "by": "user", "latencyMs": 3, "provenance": prov });
    for (name, v) in [
        ("agent.toolCall", call),
        ("agent.toolResult", result),
        ("agent.approval", approval),
    ] {
        let stored = serde_json::to_value(received(name, v)).unwrap();
        assert_eq!(stored["provenance"]["harness"], "claudeCode", "{name}");
        assert_eq!(stored["provenance"]["model"], "other", "{name}");
        assert_eq!(
            stored["provenance"]["modelHash"],
            json!(keyed("acme/x")),
            "{name}"
        );
        assert_eq!(stored["provenance"]["weights"], "closed", "{name}");
        assert!(
            !stored.to_string().contains("acme/x"),
            "{name}: model stored raw"
        );
    }
    // Task 10 review L1: a stream turn sends no system or tools hash, and stores none.
    let mut stream = turn_in("acme/x", None);
    let o = stream.as_object_mut().unwrap();
    for k in ["systemHash", "toolsHash", "bodyHash"] {
        o.remove(k);
    }
    o.insert("capture".into(), json!("stream"));
    let stored = serde_json::to_value(received("agent.turn", stream)).unwrap();
    assert!(stored.get("systemHash").is_none() && stored.get("toolsHash").is_none());
    // Task 10 review H1: `bodyBlake3` is stored as sent (content, beside the body).
    let mut exact = turn_in("acme/x", None);
    exact["bodyBlake3"] = json!(B3);
    let stored = serde_json::to_value(received("agent.turn", exact)).unwrap();
    assert_eq!(stored["bodyBlake3"], B3);
    assert_eq!(stored["bodyHash"], json!(keyed(B3)));
}

#[test]
fn turns_key_their_hashes_and_normalise_the_upstream() {
    let AnyPayload::AgentTurn(t) = received("agent.turn", turn_in("acme/x", Some("Moonshot AI")))
    else {
        panic!("kind")
    };
    assert_eq!(t.upstream, AGENT_UPSTREAMS.get("moonshotai"));
    assert_eq!(t.upstream_hash, None);
    assert_eq!(t.system_hash, Some(keyed(B3)));
    assert_eq!(t.tools_hash, Some(keyed(B3)));
    assert_eq!(t.body_hash, Some(keyed(B3)));
    assert_eq!(
        (t.model, t.model_hash),
        (Name::OTHER, Some(keyed("acme/x")))
    );
    let AnyPayload::AgentTurn(t) =
        received("agent.turn", turn_in("acme/x", Some("Brand New Cloud")))
    else {
        panic!("kind")
    };
    assert_eq!(t.upstream, None);
    assert_eq!(t.upstream_hash, Some(keyed("Brand New Cloud")));
    let AnyPayload::AgentTurn(t) = received("agent.turn", turn_in("acme/x", None)) else {
        panic!("kind")
    };
    assert_eq!((t.upstream, t.upstream_hash), (None, None));
    // An unkeyed hash that is not `blake3:<hex>` is refused.
    let mut bad = turn_in("acme/x", None);
    bad["systemHash"] = json!("k1:00000000000000000000000000000000");
    let spec = kind("agent.turn", 1).unwrap();
    assert!(validate_payload(spec, &bad, &key, &ScrubCtx::default()).is_err());
}

#[test]
fn tool_calls_key_args_hash_and_results_key_raw_tx_ids() {
    let AnyPayload::AgentToolCall(c) = received(
        "agent.toolCall",
        json!({
            "callId": "01JC0000000000000000000006", "turnId": "01JC0000000000000000000005",
            "tool": "fin", "command": "item.trim", "argsHash": B3,
        }),
    ) else {
        panic!("kind")
    };
    assert_eq!(c.args_hash, keyed(B3));
    let AnyPayload::AgentToolResult(r) = received(
        "agent.toolResult",
        json!({
            "callId": "01JC0000000000000000000006", "ok": true, "micros": 3,
            "txIds": ["01JC0000000000000000000002", "my-idem-key"], "attribution": "exact",
        }),
    ) else {
        panic!("kind")
    };
    assert_eq!(r.tx_ids[0].as_str(), "01JC0000000000000000000002");
    assert!(r.tx_ids[1].is_hashed());
    let over: Vec<String> = (0..65).map(|i| format!("k{i}")).collect();
    let spec = kind("agent.toolResult", 1).unwrap();
    let v = json!({ "callId": "01JC0000000000000000000006", "ok": true, "micros": 3,
        "txIds": over, "attribution": "exact" });
    assert_eq!(
        validate_payload(spec, &v, &key, &ScrubCtx::default())
            .unwrap_err()
            .code,
        "EVENT_INVALID"
    );
}

fn fixture(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn upstreams_normalise_from_openrouters_provider_list_and_the_fixtures() {
    let doc: Value =
        serde_json::from_str(&fixture("tests/fixtures/openrouter_providers.json")).unwrap();
    let providers = doc["providers"].as_array().unwrap();
    assert!(providers.len() > 100);
    for p in providers {
        let (name, slug) = (p["name"].as_str().unwrap(), p["slug"].as_str().unwrap());
        assert_eq!(
            agent_upstream(name).map(|n| n.as_str()),
            Some(slug),
            "{name}"
        );
        assert_eq!(
            agent_upstream(slug).map(|n| n.as_str()),
            Some(slug),
            "{slug}"
        );
    }
    for (reported, slug) in [
        ("Moonshot AI", "moonshotai"),
        ("OpenAI", "openai"),
        ("DeepInfra", "deepinfra"),
        ("Together", "together"),
        ("Fireworks", "fireworks"),
        ("Z.AI", "z-ai"),
        ("Google", "google-vertex"),
        ("Google AI Studio", "google-ai-studio"),
        ("  amazon bedrock ", "amazon-bedrock"),
    ] {
        assert_eq!(
            agent_upstream(reported).map(|n| n.as_str()),
            Some(slug),
            "{reported}"
        );
    }
    for unknown in ["", "Other", "Brand New Cloud", "---"] {
        assert_eq!(agent_upstream(unknown), None, "{unknown:?}");
    }
    // Every upstream the client's SSE fixtures name is known.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/finisherd/test/fixtures/model-streams/openrouter");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in text.lines().filter_map(|l| l.strip_prefix("data: ")) {
            let Ok(chunk) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(p) = chunk["provider"].as_str() {
                assert!(agent_upstream(p).is_some(), "fixture upstream {p:?}");
                seen += 1;
            }
        }
    }
    assert!(seen > 0);
}

const SNAPSHOT: &str = "tests/fixtures/agent_tables.snapshot.json";

fn agent_tables() -> [&'static NameTable; 5] {
    [
        &AGENT_PROVIDERS,
        &AGENT_MODELS,
        &AGENT_ROLES,
        &AGENT_TOOLS,
        &AGENT_UPSTREAMS,
    ]
}

/// Review H2: no agent table loses an entry, so a name recorded once never re-reads as
/// `other`. `FIN_EVENTS_BLESS=1` adds new entries to the snapshot; it never removes one.
#[test]
fn agent_tables_are_append_only() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(SNAPSHOT);
    let mut snap: BTreeMap<String, Vec<String>> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into()))
            .unwrap();
    for t in agent_tables() {
        let old = snap.entry(t.table.to_string()).or_default();
        for name in old.iter() {
            assert!(
                t.get(name).is_some(),
                "{} lost {name:?}: agent tables are append-only",
                t.table
            );
        }
        if std::env::var("FIN_EVENTS_BLESS").is_ok_and(|v| v == "1") {
            for e in t.entries() {
                if !old.iter().any(|o| o == e) {
                    old.push(e.to_string());
                }
            }
        }
        assert_eq!(
            old.len(),
            t.entries().len(),
            "{}: snapshot not blessed",
            t.table
        );
    }
    if std::env::var("FIN_EVENTS_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, serde_json::to_string_pretty(&snap).unwrap() + "\n").unwrap();
    }
    assert!(
        names::TABLES
            .iter()
            .any(|t| std::ptr::eq(*t, &AGENT_UPSTREAMS))
    );
}
