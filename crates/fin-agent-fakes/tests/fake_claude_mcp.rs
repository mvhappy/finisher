//! `fake-claude`'s SDK MCP server, the MCP permission tool, cancelled prompts, and its
//! configuration through the sidecar file.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::*;
use serde_json::{Value, json};

fn mcp_config(dir: &Dir) -> String {
    let p = dir.write(
        "mcp.json",
        &json!({"mcpServers": {"finisher": {"type": "sdk", "name": "finisher"}}}),
    );
    p.to_string_lossy().into_owned()
}

fn rpc(id: &Value, result: Value) -> Value {
    json!({"mcp_response": {"jsonrpc": "2.0", "id": id, "result": result}})
}

/// Answers the SDK server's handshake as `finisherd` would, with one tool `shell`.
fn serve_handshake(f: &mut Fake) {
    let note = f.read();
    assert_eq!(
        note["request"]["message"]["method"],
        "notifications/initialized"
    );
    f.answer(
        &note,
        json!({"mcp_response": {"jsonrpc": "2.0", "result": {}, "id": null}}),
    );
    let list = f.read();
    assert_eq!(list["request"]["message"]["method"], "tools/list");
    let id = list["request"]["message"]["id"].clone();
    f.answer(
        &list,
        rpc(
            &id,
            json!({"tools": [{"name": "shell", "inputSchema": {"type": "object"}}]}),
        ),
    );
}

#[test]
fn fake_sdk_mcp_handshake_starts_before_initialize_answer() {
    let dir = Dir::new("mcp");
    let cfg = mcp_config(&dir);
    let mut f = Fake::start(
        &dir,
        None,
        &["--mcp-config", &cfg, "--strict-mcp-config"],
        &[],
    );
    f.control("init-1", "initialize");
    let first = f.read();
    assert_eq!(
        first["type"], "control_request",
        "the MCP initialize comes first: {first}"
    );
    assert_eq!(first["request"]["subtype"], "mcp_message");
    assert_eq!(first["request"]["server_name"], "finisher");
    let msg = &first["request"]["message"];
    assert_eq!(
        (msg["method"].as_str(), msg["id"].as_i64()),
        (Some("initialize"), Some(0))
    );
    assert_eq!(msg["params"]["protocolVersion"], "2025-11-25");
    let answer = f.read();
    assert_eq!(
        answer["response"]["request_id"], "init-1",
        "the host's initialize is answered unblocked"
    );
    f.answer(
        &first,
        rpc(
            &json!(0),
            json!({"protocolVersion": "2025-11-25", "capabilities": {"tools": {}}}),
        ),
    );
    serve_handshake(&mut f);
    f.user("go");
    let init = f.read();
    assert_eq!(
        init["mcp_servers"],
        json!([{"name": "finisher", "status": "connected", "source": "sdk"}])
    );
    let tools = init["tools"].as_array().unwrap();
    assert_eq!(tools.last().unwrap(), "mcp__finisher__shell");
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    let obs = observed(&dir);
    assert_eq!(
        obs["controlResponses"].as_array().unwrap().len(),
        3,
        "every answer is recorded"
    );
    assert_eq!(obs["controlRequests"][0]["subtype"], "initialize");
}

#[test]
fn fake_sdk_mcp_unanswered_server_fails() {
    let dir = Dir::new("mcpfail");
    let cfg = mcp_config(&dir);
    let mut f = Fake::start(
        &dir,
        Some(json!({"mcpTimeoutMs": 200})),
        &["--mcp-config", &cfg],
        &[],
    );
    f.user("go");
    let first = f.read();
    assert_eq!(
        first["request"]["subtype"], "mcp_message",
        "without initialize the first turn starts it"
    );
    let init = f.read();
    assert_eq!(init["subtype"], "init");
    assert_eq!(
        init["mcp_servers"],
        json!([{"name": "finisher", "status": "failed", "source": "sdk"}])
    );
    assert!(
        init["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| !t.as_str().unwrap().starts_with("mcp__"))
    );
    assert!(f.finish().success());
}

#[test]
fn fake_ask_goes_through_the_mcp_permission_tool() {
    let dir = Dir::new("mcpask");
    let cfg = mcp_config(&dir);
    let script = json!({"steps": [{"expect": "user", "ask": {"tool": "Bash", "input": {"command": "ls"}},
        "onAllow": [{"type": "system", "subtype": "allowed"}], "onDeny": [{"type": "system", "subtype": "denied"}]}]});
    let mut f = Fake::start(
        &dir,
        Some(script),
        &[
            "--mcp-config",
            &cfg,
            "--permission-prompt-tool",
            "mcp__finisher__approve",
        ],
        &[],
    );
    f.control("init-1", "initialize");
    let first = f.read();
    f.answer(&first, rpc(&json!(0), json!({})));
    f.read(); // the initialize answer
    serve_handshake(&mut f);
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    let call = f.read();
    assert_eq!(call["request"]["subtype"], "mcp_message");
    let msg = &call["request"]["message"];
    assert_eq!(msg["method"], "tools/call");
    assert_eq!(msg["params"]["name"], "approve");
    assert_eq!(msg["params"]["arguments"]["tool_name"], "Bash");
    assert_eq!(
        msg["params"]["arguments"]["input"],
        json!({"command": "ls"})
    );
    assert!(msg["params"]["arguments"]["tool_use_id"].is_string());
    let decision = json!({"behavior": "allow", "updatedInput": {"command": "ls"}}).to_string();
    f.answer(
        &call,
        rpc(
            &msg["id"],
            json!({"content": [{"type": "text", "text": decision}]}),
        ),
    );
    assert_eq!(f.read()["subtype"], "allowed");
    assert!(f.finish().success());
    let obs = observed(&dir);
    assert_eq!(obs["answers"][0]["via"], "mcp");
    assert_eq!(obs["answers"][0]["answer"]["behavior"], "allow");
}

#[test]
fn fake_ask_cancel_after_ms_withdraws_the_request() {
    let dir = Dir::new("cancel");
    let script = json!({"steps": [{"expect": "user", "ask": {"tool": "Bash", "input": {}, "cancelAfterMs": 200},
        "onAllow": [{"type": "system", "subtype": "allowed"}], "onDeny": [{"type": "system", "subtype": "denied"}],
        "emit": []}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    let req = f.read();
    assert_eq!(req["request"]["subtype"], "can_use_tool");
    let cancel = f.read();
    assert_eq!(
        cancel,
        json!({"type": "control_cancel_request", "request_id": req["request_id"]})
    );
    assert_eq!(
        f.read()["type"],
        "result",
        "neither branch runs; the turn ends"
    );
    // A late answer is still recorded.
    f.answer(&req, json!({"behavior": "allow", "updatedInput": {}}));
    assert!(f.finish().success());
    let obs = observed(&dir);
    assert_eq!(obs["cancelled"], json!([req["request_id"]]));
    assert_eq!(obs["controlResponses"].as_array().unwrap().len(), 1);
}

/// A copy of the binary in its own directory, so a sidecar can sit next to it.
fn private_copy(dir: &Dir) -> PathBuf {
    let bin = PathBuf::from(BIN);
    let copy = dir.path(bin.file_name().unwrap().to_str().unwrap());
    std::fs::copy(&bin, &copy).unwrap();
    copy
}

#[test]
fn fake_reads_its_sidecar_and_env_wins() {
    let dir = Dir::new("sidecar");
    let copy = private_copy(&dir);
    dir.write("turn.json", &json!({"steps": [{"expect": "user", "cumulativeUsage": {"outputTokens": 4}, "text": "from the sidecar script"}]}));
    dir.write(
        "fake-claude.json",
        &json!({"script": "turn.json", "observed": "@config/observed.json", "version": "2.1.500"}),
    );
    let config = dir.path("config");
    std::fs::create_dir_all(&config).unwrap();
    let start = |envs: &[(&str, &str)]| {
        let mut cmd = Command::new(&copy);
        cmd.args(BASE_ARGS)
            .env_clear()
            .env("CLAUDE_CONFIG_DIR", &config)
            .envs(envs.iter().copied())
            .current_dir(&dir.0);
        Fake::spawn(cmd)
    };
    let mut f = start(&[]);
    assert_eq!(initialize(&mut f)["claude_code_version"], "2.1.500");
    f.user("go");
    f.read();
    assert_eq!(f.read()["result"], "from the sidecar script");
    assert!(f.finish().success());
    let mut f = start(&[("FAKE_CLAUDE_VERSION", "2.1.600")]);
    assert_eq!(
        initialize(&mut f)["claude_code_version"],
        "2.1.600",
        "the environment wins"
    );
    assert!(f.finish().success());
    let raw = std::fs::read_to_string(config.join("observed.json")).unwrap();
    let all: Vec<Value> = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        all.len(),
        2,
        "@config/<name> is under CLAUDE_CONFIG_DIR, one record per process"
    );
    assert!(all.iter().all(|r| {
        !r["envNames"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().starts_with("FAKE_CLAUDE_SCRIPT"))
    }));
}
