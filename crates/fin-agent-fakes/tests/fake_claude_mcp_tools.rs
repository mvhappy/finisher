//! `fake-claude` calling a tool of an SDK MCP server (`ask {mcp: true}`, plan 7D Task 13): the
//! call announced, asked, then `tools/call` over `mcp_message` and the answer's content as the
//! call's `tool_result`; denied, refused, failed and interrupted calls.

mod common;

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

/// Starts the fake with the `finisher` server, answers the whole handshake with `tools`
/// listed, and sends the first user line; returns the fake after the turn's `system/init`.
fn connected(dir: &Dir, script: Value, tools: &[&str]) -> (Fake, Value) {
    let cfg = mcp_config(dir);
    let mut f = Fake::start(dir, Some(script), &["--mcp-config", &cfg], &[]);
    f.control("init-1", "initialize");
    let first = f.read();
    assert_eq!(first["request"]["message"]["method"], "initialize");
    f.answer(
        &first,
        rpc(
            &json!(0),
            json!({"protocolVersion": "2025-11-25", "capabilities": {"tools": {}}}),
        ),
    );
    assert_eq!(f.read()["response"]["request_id"], "init-1");
    let note = f.read();
    assert_eq!(
        note["request"]["message"]["method"],
        "notifications/initialized"
    );
    f.answer(
        &note,
        json!({"mcp_response": {"jsonrpc": "2.0", "result": {}}}),
    );
    let list = f.read();
    assert_eq!(list["request"]["message"]["method"], "tools/list");
    let listed: Vec<Value> = tools
        .iter()
        .map(|t| json!({"name": t, "inputSchema": {"type": "object"}}))
        .collect();
    f.answer(
        &list,
        rpc(&list["request"]["message"]["id"], json!({"tools": listed})),
    );
    f.user("go");
    let init = f.read();
    assert_eq!(init["subtype"], "init");
    (f, init)
}

fn shell_step(extra: Value) -> Value {
    let mut ask = json!({"tool": "mcp__finisher__shell", "input": {"command": "ls -la"}, "mcp": true, "toolUseId": "toolu_s1"});
    ask.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    json!({"steps": [{"expect": "user", "ask": ask}]})
}

/// The announced call and its permission prompt.
fn announced_and_asked(f: &mut Fake) -> Value {
    let a = f.read();
    assert_eq!(a["type"], "assistant");
    assert_eq!(
        a["message"]["content"][0],
        json!({"type": "tool_use", "id": "toolu_s1", "name": "mcp__finisher__shell", "input": {"command": "ls -la"}})
    );
    let ask = f.read();
    assert_eq!(ask["request"]["subtype"], "can_use_tool");
    assert_eq!(ask["request"]["tool_name"], "mcp__finisher__shell");
    assert_eq!(ask["request"]["tool_use_id"], "toolu_s1");
    ask
}

fn allow(f: &mut Fake, ask: &Value) {
    f.answer(
        ask,
        json!({"behavior": "allow", "updatedInput": ask["request"]["input"]}),
    );
}

#[test]
fn fake_mcp_tool_call_round_trip() {
    let dir = Dir::new("mcpcall");
    let (mut f, init) = connected(&dir, shell_step(json!({})), &["shell"]);
    assert!(
        init["tools"]
            .as_array()
            .unwrap()
            .contains(&json!("mcp__finisher__shell"))
    );
    let ask = announced_and_asked(&mut f);
    allow(&mut f, &ask);
    let call = f.read();
    assert_eq!(call["request"]["subtype"], "mcp_message");
    assert_eq!(call["request"]["server_name"], "finisher");
    let msg = &call["request"]["message"];
    assert_eq!(
        (msg["jsonrpc"].as_str(), msg["method"].as_str()),
        (Some("2.0"), Some("tools/call"))
    );
    assert!(
        msg["id"].is_number(),
        "a request, not a notification: {msg}"
    );
    assert_eq!(msg["params"]["name"], "shell");
    assert_eq!(msg["params"]["arguments"], json!({"command": "ls -la"}));
    assert_eq!(msg["params"]["_meta"]["claudecode/toolUseId"], "toolu_s1");
    let content =
        json!([{"type": "text", "text": "{\"exit\":0,\"stdout\":\"a\\n\",\"stderrTail\":\"\"}"}]);
    f.answer(
        &call,
        rpc(&msg["id"], json!({"content": content, "isError": false})),
    );
    let result = f.read();
    assert_eq!(result["type"], "user");
    assert_eq!(
        result["message"]["content"][0],
        json!({"type": "tool_result", "tool_use_id": "toolu_s1", "content": content, "is_error": false})
    );
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    let obs = observed(&dir);
    assert_eq!(obs["mcpCalls"][0]["tool"], "mcp__finisher__shell");
    assert_eq!(obs["mcpCalls"][0]["toolUseId"], "toolu_s1");
    assert_eq!(obs["mcpCalls"][0]["result"]["isError"], false);
}

#[test]
fn fake_mcp_tool_call_is_error_when_the_tool_says_so_or_the_rpc_fails() {
    for (answer, text) in [
        (
            json!({"result": {"content": [{"type": "text", "text": "exit 3"}], "isError": true}}),
            "exit 3",
        ),
        (
            json!({"error": {"code": -32602, "message": "no ticket"}}),
            "no ticket",
        ),
    ] {
        let dir = Dir::new("mcpcallerr");
        let (mut f, _) = connected(&dir, shell_step(json!({})), &["shell"]);
        let ask = announced_and_asked(&mut f);
        allow(&mut f, &ask);
        let call = f.read();
        let id = call["request"]["message"]["id"].clone();
        let mut rpc = json!({"jsonrpc": "2.0", "id": id});
        rpc.as_object_mut()
            .unwrap()
            .extend(answer.as_object().unwrap().clone());
        f.answer(&call, json!({"mcp_response": rpc}));
        let result = f.read();
        let block = &result["message"]["content"][0];
        assert_eq!(block["is_error"], true, "{result}");
        assert!(block["content"].to_string().contains(text), "{result}");
        assert_eq!(f.read()["type"], "result");
        assert!(f.finish().success());
    }
}

#[test]
fn fake_mcp_tool_call_denied_never_calls() {
    let dir = Dir::new("mcpdeny");
    let (mut f, _) = connected(&dir, shell_step(json!({})), &["shell"]);
    let ask = announced_and_asked(&mut f);
    f.answer(&ask, json!({"behavior": "deny", "message": "no"}));
    let result = f.read();
    assert_eq!(result["type"], "user", "no tools/call: {result}");
    assert_eq!(result["message"]["content"][0]["is_error"], true);
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    assert!(observed(&dir)["mcpCalls"].is_null());
}

#[test]
fn fake_mcp_tool_not_listed_is_refused_before_any_prompt() {
    let dir = Dir::new("mcpabsent");
    let (f, init) = connected(&dir, shell_step(json!({})), &[]);
    assert!(
        !init["tools"]
            .as_array()
            .unwrap()
            .contains(&json!("mcp__finisher__shell"))
    );
    assert_eq!(f.read()["type"], "assistant");
    let result = f.read();
    assert_eq!(result["type"], "user", "no prompt: {result}");
    let block = &result["message"]["content"][0];
    assert_eq!(block["is_error"], true);
    assert!(
        block["content"]
            .as_str()
            .unwrap()
            .contains("No such tool available")
    );
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
}

#[test]
fn fake_mcp_tool_call_interrupted_sends_cancelled() {
    let dir = Dir::new("mcpint");
    let (mut f, _) = connected(&dir, shell_step(json!({})), &["shell"]);
    let ask = announced_and_asked(&mut f);
    allow(&mut f, &ask);
    let call = f.read();
    let id = call["request"]["message"]["id"].clone();
    f.control("int-1", "interrupt");
    let mut lines = vec![];
    while !lines
        .iter()
        .any(|l: &Value| l["request"]["message"]["method"] == "notifications/cancelled")
    {
        lines.push(f.read());
    }
    assert!(
        lines.iter().any(|l| l["type"] == "result"),
        "the turn ended: {lines:?}"
    );
    let cancel = lines.last().unwrap();
    assert_eq!(cancel["request"]["message"]["params"]["requestId"], id);
    assert!(
        cancel["request"]["message"]["id"].is_null(),
        "a notification"
    );
    // A late answer to the cancelled call is consumed quietly.
    f.answer(&call, rpc(&id, json!({"content": []})));
    f.answer(
        cancel,
        json!({"mcp_response": {"jsonrpc": "2.0", "result": {}}}),
    );
    assert!(f.silent_for(200));
    // Stdin's end after an error `result` (the interrupted turn's) exits 1, as 2.1.321 does.
    assert_eq!(f.finish().code(), Some(1));
}
