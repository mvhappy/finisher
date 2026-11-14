//! `fake-codex` turns: scripted notifications, the server requests it awaits, steer, interrupt,
//! crashes, and threads that outlive the process.

mod codex_common;

use codex_common::*;
use serde_json::{Value, json};

fn tid(t: &Value) -> String {
    t["thread"]["id"].as_str().unwrap().to_string()
}

fn methods(lines: &[Value]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l["method"].as_str().unwrap_or("?").to_string())
        .collect()
}

fn item_types(lines: &[Value], method: &str) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l["method"] == method)
        .map(|l| {
            l["params"]["item"]["type"]
                .as_str()
                .unwrap_or("?")
                .to_string()
        })
        .collect()
}

/// Reads lines until `want` (a server request), answering nothing; returns the lines so far.
fn until_request(c: &mut Client, want: &str) -> (Vec<Value>, Value) {
    let mut seen = vec![];
    loop {
        let v = c.next();
        if v["method"] == want && v.get("id").is_some() {
            return (seen, v);
        }
        seen.push(v);
    }
}

fn shell_script(extra_turns: Value) -> Value {
    let mut turns = vec![json!({"steps": [
        {"dynamicTool": {"tool": "shell", "arguments": {"command": "ls", "timeoutMs": 1000}}}, {"say": "done"}]})];
    turns.extend(extra_turns.as_array().cloned().unwrap_or_default());
    json!({"turns": turns})
}

#[test]
fn fake_dynamic_tool_round_trip() {
    let dir = Dir::new("tool");
    let script = shell_script(
        json!([{"steps": [{"dynamicTool": {"tool": "shell", "arguments": {"command": "false"}, "callId": "call_x"}}]}]),
    );
    let mut c = Client::start(&dir, Some(script));
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({"dynamicTools": [Client::shell_tool()]})));
    let turn = c.start_turn(&t, "run ls");
    let (before, req) = until_request(&mut c, "item/tool/call");
    let p = &req["params"];
    assert_eq!(
        (
            p["threadId"].as_str(),
            p["turnId"].as_str(),
            p["tool"].as_str()
        ),
        (Some(t.as_str()), Some(turn.as_str()), Some("shell"))
    );
    assert_eq!(p["arguments"], json!({"command": "ls", "timeoutMs": 1000}));
    let call_id = p["callId"].as_str().unwrap().to_string();
    // The call is announced as an in-progress item first, and the turn waits for the answer.
    let started = before
        .iter()
        .find(|l| l["method"] == "item/started" && l["params"]["item"]["type"] == "dynamicToolCall")
        .expect("item/started");
    assert_eq!(
        (
            started["params"]["item"]["id"].as_str(),
            started["params"]["item"]["status"].as_str()
        ),
        (Some(call_id.as_str()), Some("inProgress"))
    );
    assert!(
        c.next_within(300).is_none(),
        "the turn must wait for the answer"
    );
    c.answer(
        &req["id"],
        json!({"contentItems": [{"type": "inputText", "text": "exit 0\na.ts\n"}], "success": true}),
    );
    let rest = c.turn_lines();
    let done = rest
        .iter()
        .find(|l| {
            l["method"] == "item/completed" && l["params"]["item"]["type"] == "dynamicToolCall"
        })
        .unwrap();
    let item = &done["params"]["item"];
    assert_eq!(
        (
            item["status"].as_str(),
            item["success"].as_bool(),
            item["tool"].as_str()
        ),
        (Some("completed"), Some(true), Some("shell"))
    );
    assert_eq!(item["contentItems"][0]["text"], "exit 0\na.ts\n");
    assert_eq!(
        rest.last().unwrap()["params"]["turn"]["status"],
        "completed"
    );
    assert!(item_types(&rest, "item/completed").contains(&"agentMessage".to_string()));
    // A `success: false` answer is a failed item; the turn still ends.
    c.start_turn(&t, "again");
    let (_, req) = until_request(&mut c, "item/tool/call");
    assert_eq!(req["params"]["callId"], "call_x");
    c.answer(
        &req["id"],
        json!({"contentItems": [{"type": "inputText", "text": "denied"}], "success": false}),
    );
    let rest = c.turn_lines();
    let item = &rest
        .iter()
        .find(|l| {
            l["method"] == "item/completed" && l["params"]["item"]["type"] == "dynamicToolCall"
        })
        .unwrap()["params"]["item"];
    assert_eq!(
        (item["status"].as_str(), item["success"].as_bool()),
        (Some("failed"), Some(false))
    );
    c.close_stdin();
    assert_eq!(c.finish().code(), Some(0));
    let answers = observed(&dir)["answers"].clone();
    assert_eq!(answers.as_array().unwrap().len(), 2);
    assert_eq!(answers[0]["method"], "item/tool/call");
    assert_eq!(answers[0]["result"]["success"], true);
}

#[test]
fn fake_dynamic_tool_needs_declaration_and_capability() {
    let dir = Dir::new("tooldecl");
    let mut c = Client::start(&dir, Some(shell_script(json!([]))));
    // `dynamicTools` is experimental: without the capability the call is refused.
    c.call("initialize", json!({"clientInfo": {"name": "finisher", "version": "0"}, "capabilities": {"experimentalApi": false}}));
    let e = c.call_err(
        "thread/start",
        json!({"cwd": dir.work(), "dynamicTools": [Client::shell_tool()]}),
    );
    assert_eq!(e["code"], -32600);
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("thread/start.dynamicTools requires experimentalApi capability"),
        "{e}"
    );
    drop(c);
    // A tool the thread never declared cannot be called by the model: the step is skipped.
    let mut c = Client::start(&dir, Some(shell_script(json!([]))));
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    c.start_turn(&t, "run ls");
    let lines = c.turn_lines();
    assert!(
        !methods(&lines).contains(&"item/tool/call".to_string()),
        "{:?}",
        methods(&lines)
    );
    c.close_stdin();
    c.finish();
    assert_eq!(observed(&dir)["skippedTools"], json!(["shell"]));
}

fn patch_script() -> Value {
    let changes = json!([{"path": "src/a.ts", "kind": {"type": "update", "move_path": null}, "diff": "@@ -1 +1 @@\n-a\n+b\n"},
        {"path": "new.txt", "kind": {"type": "add"}, "diff": "+hi\n"}]);
    json!({"turns": [{"steps": [{"patch": {"changes": changes}}]}, {"steps": [{"patch": {"changes": changes, "itemId": "fc_2"}}]}]})
}

#[test]
fn fake_patch_approval_round_trip() {
    let dir = Dir::new("patch");
    let mut c = Client::start(&dir, Some(patch_script()));
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let turn = c.start_turn(&t, "edit");
    let (before, req) = until_request(&mut c, "item/fileChange/requestApproval");
    let item_id = req["params"]["itemId"].as_str().unwrap().to_string();
    assert_eq!(
        (
            req["params"]["threadId"].as_str(),
            req["params"]["turnId"].as_str()
        ),
        (Some(t.as_str()), Some(turn.as_str()))
    );
    let started = before
        .iter()
        .find(|l| l["method"] == "item/started" && l["params"]["item"]["type"] == "fileChange")
        .unwrap();
    assert_eq!(started["params"]["item"]["id"], item_id.as_str());
    assert_eq!(started["params"]["item"]["status"], "inProgress");
    assert_eq!(started["params"]["item"]["changes"][1]["path"], "new.txt");
    assert!(
        c.next_within(300).is_none(),
        "the turn waits for the decision"
    );
    c.answer(&req["id"], json!({"decision": "accept"}));
    let rest = c.turn_lines();
    let resolved = rest
        .iter()
        .find(|l| l["method"] == "serverRequest/resolved")
        .expect("serverRequest/resolved");
    assert_eq!(resolved["params"]["requestId"], req["id"]);
    let done = rest
        .iter()
        .find(|l| l["method"] == "item/completed" && l["params"]["item"]["type"] == "fileChange")
        .unwrap();
    assert_eq!(done["params"]["item"]["status"], "completed");
    // Declined: the item says so and the turn continues to its end.
    c.start_turn(&t, "edit again");
    let (_, req) = until_request(&mut c, "item/fileChange/requestApproval");
    assert_eq!(req["params"]["itemId"], "fc_2");
    c.answer(&req["id"], json!({"decision": "decline"}));
    let rest = c.turn_lines();
    let done = rest
        .iter()
        .find(|l| l["method"] == "item/completed" && l["params"]["item"]["type"] == "fileChange")
        .unwrap();
    assert_eq!(done["params"]["item"]["status"], "declined");
    assert_eq!(
        rest.last().unwrap()["params"]["turn"]["status"],
        "completed"
    );
    c.close_stdin();
    c.finish();
    let o = observed(&dir);
    assert_eq!(o["answers"][0]["result"], json!({"decision": "accept"}));
    assert_eq!(o["answers"][1]["method"], "item/fileChange/requestApproval");
}

fn user_text(l: &Value) -> Option<&str> {
    l["params"]["item"]["content"][0]["text"].as_str()
}

#[test]
fn fake_steer_joins_active_turn() {
    let dir = Dir::new("steer");
    let mut c = Client::start(
        &dir,
        Some(json!({"turns": [{"steps": [{"expect": "steer"}, {"say": "with steer"}]}]})),
    );
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let turn = c.start_turn(&t, "first");
    // `turn/started` and the first user message are already past before the steer.
    c.until("turn/started");
    c.until("item/completed");
    // A stale turn id and a wrong thread are refused; the turn goes on.
    let e = c.call_err("turn/steer", json!({"threadId": t, "expectedTurnId": "nope", "input": [{"type": "text", "text": "x", "text_elements": []}]}));
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("expected active turn id"),
        "{e}"
    );
    let r = c.call("turn/steer", json!({"threadId": t, "expectedTurnId": turn, "input": [{"type": "text", "text": "more please", "text_elements": []}]}));
    assert_eq!(r, json!({"turnId": turn}));
    let rest = c.turn_lines();
    let joined: Vec<&str> = rest
        .iter()
        .filter(|l| l["method"] == "item/completed")
        .filter_map(user_text)
        .collect();
    assert_eq!(
        joined,
        ["more please"],
        "the steer is a user message of the same turn"
    );
    assert_eq!(rest.last().unwrap()["params"]["turn"]["id"], turn.as_str());
    assert_eq!(
        rest.last().unwrap()["params"]["turn"]["status"],
        "completed"
    );
    // No active turn any more.
    let e = c.call_err(
        "turn/steer",
        json!({"threadId": t, "expectedTurnId": turn, "input": []}),
    );
    assert!(
        e["message"].as_str().unwrap().contains("no active turn"),
        "{e}"
    );
    c.close_stdin();
    c.finish();
    let o = observed(&dir);
    assert_eq!(o["steers"].as_array().unwrap().len(), 1);
    assert_eq!(o["steers"][0]["joined"], true);
    assert_eq!(o["steers"][0]["input"][0]["text"], "more please");
}

#[test]
fn fake_steer_refusal_is_active_turn_not_steerable() {
    let dir = Dir::new("refuse");
    let mut c = Client::start(
        &dir,
        Some(json!({"steer": "refuse", "turns": [{"steps": [{"expect": "interrupt"}]}]})),
    );
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let turn = c.start_turn(&t, "first");
    c.until("turn/started");
    let e = c.call_err("turn/steer", json!({"threadId": t, "expectedTurnId": turn, "input": [{"type": "text", "text": "x", "text_elements": []}]}));
    assert_eq!(e["code"], -32600);
    assert_eq!(
        e["data"]["codexErrorInfo"],
        json!({"activeTurnNotSteerable": {"turnKind": "review"}})
    );
    c.call("turn/interrupt", json!({"threadId": t, "turnId": turn}));
    let rest = c.turn_lines();
    assert_eq!(
        rest.last().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
    assert_eq!(observed(&dir)["steers"][0]["joined"], false);
}

#[test]
fn fake_interrupt_then_completed_interrupted() {
    let dir = Dir::new("interrupt");
    let mut c = Client::start(
        &dir,
        Some(
            json!({"turns": [{"steps": [{"expect": "interrupt"}, {"say": "never"}]}, {"steps": [{"say": "second"}]}]}),
        ),
    );
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let turn = c.start_turn(&t, "long job");
    c.until("turn/started");
    let r = c.call("turn/interrupt", json!({"threadId": t, "turnId": turn}));
    assert_eq!(r, json!({}));
    let rest = c.turn_lines();
    let done = &rest.last().unwrap()["params"]["turn"];
    assert_eq!(
        (done["id"].as_str(), done["status"].as_str()),
        (Some(turn.as_str()), Some("interrupted"))
    );
    assert!(
        !methods(&rest)
            .iter()
            .any(|m| m == "item/agentMessage/delta"),
        "the rest of the script is skipped: {:?}",
        methods(&rest)
    );
    // The thread is usable afterwards and the next turn plays its own script.
    c.start_turn(&t, "next");
    let rest = c.turn_lines();
    assert_eq!(
        rest.last().unwrap()["params"]["turn"]["status"],
        "completed"
    );
    assert!(rest.iter().any(|l| l["params"]["item"]["text"] == "second"));
    // An interrupt with nothing running is answered and changes nothing.
    assert_eq!(
        c.call("turn/interrupt", json!({"threadId": t, "turnId": turn})),
        json!({})
    );
    assert!(c.next_within(300).is_none());
}

#[test]
fn fake_interrupt_can_be_ignored() {
    let dir = Dir::new("ignore");
    let mut c = Client::start(
        &dir,
        Some(
            json!({"onInterrupt": "ignore", "turns": [{"steps": [{"sleep": 600}, {"say": "still here"}]}]}),
        ),
    );
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let turn = c.start_turn(&t, "x");
    c.until("turn/started");
    c.call("turn/interrupt", json!({"threadId": t, "turnId": turn}));
    let rest = c.turn_lines();
    assert_eq!(
        rest.last().unwrap()["params"]["turn"]["status"],
        "completed"
    );
    assert!(
        rest.iter()
            .any(|l| l["params"]["item"]["text"] == "still here")
    );
}

#[test]
fn fake_resume_reads_its_thread() {
    let dir = Dir::new("resume");
    let thread;
    {
        let mut c = Client::start(&dir, None);
        c.initialize();
        let t = c.start_thread(&dir, json!({"dynamicTools": [Client::shell_tool()]}));
        thread = tid(&t);
        assert!(
            t["thread"]["path"]
                .as_str()
                .is_some_and(|p| p.starts_with(dir.home().join("sessions").to_str().unwrap())),
            "{t}"
        );
        c.start_turn(&thread, "hello");
        c.turn_lines();
        c.close_stdin();
        assert_eq!(c.finish().code(), Some(0));
    }
    // An ephemeral thread leaves nothing behind.
    let eph;
    {
        let mut c = Client::start(&dir, None);
        c.initialize();
        let t = c.start_thread(&dir, json!({"ephemeral": true}));
        assert_eq!(t["thread"]["path"], Value::Null);
        eph = tid(&t);
    }
    let mut c = Client::start(&dir, None);
    c.initialize();
    // Before it is resumed in this process, a thread is not loaded.
    let e = c.call_err(
        "thread/revert",
        json!({"threadId": thread, "beforeTurnId": "x"}),
    );
    assert_eq!(e["message"], format!("thread not found: {thread}"));
    let e = c.call_err("turn/start", json!({"threadId": thread, "input": []}));
    assert_eq!(e["message"], format!("thread not found: {thread}"));
    let e = c.call_err("thread/resume", json!({"threadId": eph}));
    assert_eq!(
        e["message"],
        format!("no rollout found for thread id {eph}")
    );
    let r = c.call("thread/resume", json!({"threadId": thread, "cwd": dir.work(), "model": null, "sandbox": "read-only",
        "approvalPolicy": "on-request", "approvalsReviewer": "user", "developerInstructions": "x", "config": {}}));
    assert_eq!(r["thread"]["id"], thread.as_str());
    assert_eq!(r["thread"]["turns"].as_array().unwrap().len(), 1);
    assert_eq!(r["thread"]["turns"][0]["items"][0]["type"], "userMessage");
    assert_eq!(r["approvalPolicy"], "on-request");
    let started = c.until("thread/started");
    assert_eq!(started["params"]["thread"]["id"], thread.as_str());
    c.start_turn(&thread, "again");
    c.turn_lines();
    let read = c.call(
        "thread/read",
        json!({"threadId": thread, "includeTurns": true}),
    );
    assert_eq!(read["thread"]["turns"].as_array().unwrap().len(), 2);
    let read = c.call(
        "thread/read",
        json!({"threadId": thread, "includeTurns": false}),
    );
    assert_eq!(read["thread"]["turns"], json!([]));
    c.close_stdin();
    c.finish();
    assert_eq!(observed_all(&dir).len(), 3, "one record per process");
}

#[test]
fn fake_resume_restores_dynamic_tools_per_pin() {
    let dir = Dir::new("resumetools");
    let thread;
    {
        let mut c = Client::start(&dir, None);
        c.initialize();
        thread = tid(&c.start_thread(&dir, json!({"dynamicTools": [Client::shell_tool()]})));
        c.close_stdin();
        c.finish();
    }
    for restores in [true, false] {
        let mut script = shell_script(json!([]));
        script["resumeRestoresDynamicTools"] = json!(restores);
        let mut c = Client::start(&dir, Some(script));
        c.initialize();
        c.call(
            "thread/resume",
            json!({"threadId": thread, "cwd": dir.work()}),
        );
        c.start_turn(&thread, "run ls");
        if restores {
            let (_, req) = until_request(&mut c, "item/tool/call");
            c.answer(&req["id"], json!({"contentItems": [], "success": true}));
        }
        let lines = c.turn_lines();
        assert!(!methods(&lines).iter().any(|m| m == "item/tool/call"));
        c.close_stdin();
        c.finish();
        let skipped = observed(&dir)["skippedTools"].clone();
        assert_eq!(
            skipped,
            if restores {
                Value::Null
            } else {
                json!(["shell"])
            }
        );
    }
}

#[test]
fn fake_revert_drops_turns() {
    let dir = Dir::new("revert");
    let (thread, turns);
    {
        let mut c = Client::start(&dir, None);
        c.initialize();
        thread = tid(&c.start_thread(&dir, json!({})));
        turns = (0..3)
            .map(|i| {
                let t = c.start_turn(&thread, &format!("turn {i}"));
                c.turn_lines();
                t
            })
            .collect::<Vec<_>>();
        let e = c.call_err(
            "thread/revert",
            json!({"threadId": thread, "beforeTurnId": "no-such-turn"}),
        );
        assert_eq!(e["code"], -32600);
        assert!(
            e["message"].as_str().unwrap().contains("no-such-turn"),
            "{e}"
        );
        assert_eq!(e["data"]["codexErrorInfo"], "threadRollbackFailed");
        let r = c.call(
            "thread/revert",
            json!({"threadId": thread, "beforeTurnId": turns[1]}),
        );
        let ids: Vec<&str> = r["thread"]["turns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [turns[0].as_str()]);
        assert_eq!(
            c.until("thread/reverted")["params"]["threadId"],
            thread.as_str()
        );
        // A new turn goes on from the kept prefix.
        let t4 = c.start_turn(&thread, "after revert");
        c.turn_lines();
        let read = c.call(
            "thread/read",
            json!({"threadId": thread, "includeTurns": true}),
        );
        let ids: Vec<&str> = read["thread"]["turns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [turns[0].as_str(), t4.as_str()]);
        c.close_stdin();
        c.finish();
    }
    // What was dropped stays dropped across a process.
    let mut c = Client::start(&dir, None);
    c.initialize();
    let r = c.call("thread/resume", json!({"threadId": thread}));
    assert_eq!(r["thread"]["turns"].as_array().unwrap().len(), 2);
    // `revertFails` models Codex refusing: nothing changes.
    drop(c);
    let mut c = Client::start(&dir, Some(json!({"revertFails": true})));
    c.initialize();
    c.call("thread/resume", json!({"threadId": thread}));
    let e = c.call_err(
        "thread/revert",
        json!({"threadId": thread, "beforeTurnId": turns[0]}),
    );
    assert_eq!(e["data"]["codexErrorInfo"], "threadRollbackFailed");
    assert_eq!(
        c.call(
            "thread/read",
            json!({"threadId": thread, "includeTurns": true})
        )["thread"]["turns"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn fake_script_knobs_usage_fail_not_logged_in_account() {
    let dir = Dir::new("knobs");
    dir.write("codex-home/auth.json", "{}");
    let usage = |i: u64, o: u64| json!({"inputTokens": i, "cachedInputTokens": 10, "outputTokens": o, "reasoningOutputTokens": 5});
    let script = json!({"turns": [
        {"steps": [{"usage": {"last": usage(100, 20), "total": usage(1100, 220)}},
            {"usage": {"last": usage(50, 10)}}, {"unknownNotification": "fake/unknownNotification"},
            {"emit": [{"method": "warning", "params": {"threadId": "$threadId", "message": "careful"}}]}]},
        {"steps": [{"fail": {"codexErrorInfo": {"httpConnectionFailed": {"httpStatusCode": 401}}, "message": "unexpected status 401", "retries": 2}}, {"say": "skipped"}]},
        {"steps": [{"fail": {"codexErrorInfo": "unauthorized"}}]},
        {"steps": [{"notLoggedIn": true}]},
        {"steps": [{"accountChange": {"type": "chatgpt", "email": "new@x.test", "planType": "pro"}}]}]});
    let mut c = Client::start(&dir, Some(script));
    c.unvalidated.push("fake/unknownNotification".into());
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    c.start_turn(&t, "one");
    let lines = c.turn_lines();
    let usages: Vec<&Value> = lines
        .iter()
        .filter(|l| l["method"] == "thread/tokenUsage/updated")
        .collect();
    assert_eq!(usages.len(), 2);
    assert_eq!(
        usages[0]["params"]["tokenUsage"]["total"]["totalTokens"],
        1320
    );
    assert_eq!(
        usages[0]["params"]["tokenUsage"]["last"]["totalTokens"],
        120
    );
    // `total` defaults to the running sum of the lasts.
    assert_eq!(
        usages[1]["params"]["tokenUsage"]["total"]["inputTokens"],
        1150
    );
    assert_eq!(
        usages[1]["params"]["tokenUsage"]["total"]["totalTokens"],
        1320 + 60
    );
    assert!(methods(&lines).contains(&"fake/unknownNotification".to_string()));
    assert_eq!(
        lines.iter().find(|l| l["method"] == "warning").unwrap()["params"]["threadId"],
        t.as_str()
    );
    // fail: willRetry errors, then a failed turn carrying the error info.
    c.start_turn(&t, "two");
    let lines = c.turn_lines();
    let errs: Vec<&Value> = lines.iter().filter(|l| l["method"] == "error").collect();
    assert_eq!(errs.len(), 2);
    assert!(errs.iter().all(|e| e["params"]["willRetry"] == true));
    let done = &lines.last().unwrap()["params"]["turn"];
    assert_eq!(done["status"], "failed");
    assert_eq!(
        done["error"]["codexErrorInfo"],
        json!({"httpConnectionFailed": {"httpStatusCode": 401}})
    );
    assert!(
        !lines
            .iter()
            .any(|l| l["params"]["item"]["text"] == "skipped")
    );
    c.start_turn(&t, "three");
    let done = c.turn_lines().pop().unwrap();
    assert_eq!(
        done["params"]["turn"]["error"]["codexErrorInfo"],
        "unauthorized"
    );
    // notLoggedIn: the unsigned turn of the real binary (Task 0): 401 retries, then a 401 failure.
    c.start_turn(&t, "four");
    let lines = c.turn_lines();
    let n = lines.iter().filter(|l| l["method"] == "error").count();
    assert!(n >= 2, "{n}");
    let done = &lines.last().unwrap()["params"]["turn"];
    assert_eq!(
        done["error"]["codexErrorInfo"],
        json!({"httpConnectionFailed": {"httpStatusCode": 401}})
    );
    // accountChange rewrites what account/read says and tells the client.
    assert_eq!(
        c.call("account/read", json!({}))["account"]["email"],
        "person@example.com"
    );
    c.start_turn(&t, "five");
    let lines = c.turn_lines();
    let upd = lines
        .iter()
        .find(|l| l["method"] == "account/updated")
        .expect("account/updated");
    assert_eq!(
        (
            upd["params"]["authMode"].as_str(),
            upd["params"]["planType"].as_str()
        ),
        (Some("chatgpt"), Some("pro"))
    );
    assert_eq!(
        c.call("account/read", json!({}))["account"],
        json!({"type": "chatgpt", "email": "new@x.test", "planType": "pro"})
    );
    // No script left: a plain turn with one message.
    c.start_turn(&t, "six");
    let lines = c.turn_lines();
    assert_eq!(
        lines.last().unwrap()["params"]["turn"]["status"],
        "completed"
    );
    assert_eq!(
        item_types(&lines, "item/completed"),
        ["userMessage", "agentMessage"]
    );
}

#[cfg(unix)]
#[test]
fn fake_crash_exits_the_way_the_script_says() {
    use std::os::unix::process::ExitStatusExt;
    for (step, code, signal) in [
        (json!({"code": 3}), Some(3), None),
        (json!({"signal": "KILL"}), None, Some(9)),
        (json!({"signal": "SEGV"}), None, Some(11)),
    ] {
        let dir = Dir::new("crash");
        let mut c = Client::start(
            &dir,
            Some(json!({"turns": [{"steps": [{"say": "partial"}, {"crash": step}]}]})),
        );
        c.initialize();
        let t = tid(&c.start_thread(&dir, json!({})));
        c.start_turn(&t, "boom");
        assert!(c.child.wait().is_ok());
        let st = c.child.try_wait().unwrap().unwrap();
        assert_eq!((st.code(), st.signal()), (code, signal));
        c.validate_on_drop = false;
    }
}
