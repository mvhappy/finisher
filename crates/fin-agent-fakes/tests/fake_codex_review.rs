//! `fake-codex` after the 7E Task 4 review: open server requests on an interrupt, the turn's
//! items, the real binary's error texts and orders, patches that reach the disk, scripts that
//! survive a crash, and the request checks 0.167.0 makes.

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

fn until_request(c: &mut Client, want: &str) -> Value {
    loop {
        let v = c.next();
        if v["method"] == want && v.get("id").is_some() {
            return v;
        }
    }
}

fn completed_item<'a>(lines: &'a [Value], kind: &str) -> &'a Value {
    &lines
        .iter()
        .find(|l| l["method"] == "item/completed" && l["params"]["item"]["type"] == kind)
        .unwrap_or_else(|| panic!("no completed {kind} in {:?}", methods(lines)))["params"]["item"]
}

fn script_file(name: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/codex")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

fn text(t: &str) -> Value {
    json!([{"type": "text", "text": t, "text_elements": []}])
}

#[test]
fn interrupt_while_a_tool_call_waits_resolves_it() {
    let dir = Dir::new("b1tool");
    let script = json!({"turns": [{"steps": [
        {"dynamicTool": {"tool": "shell", "arguments": {"command": "sleep 9"}}}, {"say": "never"}]}]});
    let mut c = Client::start(&dir, Some(script));
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({"dynamicTools": [Client::shell_tool()]})));
    let turn = c.start_turn(&t, "run");
    let req = until_request(&mut c, "item/tool/call");
    assert_eq!(
        c.call("turn/interrupt", json!({"threadId": t, "turnId": turn})),
        json!({})
    );
    let rest = c.turn_lines();
    let resolved = rest
        .iter()
        .find(|l| l["method"] == "serverRequest/resolved")
        .expect("the open request is resolved");
    assert_eq!(
        resolved["params"],
        json!({"threadId": t, "requestId": req["id"]})
    );
    let item = completed_item(&rest, "dynamicToolCall");
    assert_eq!(
        (item["status"].as_str(), item["success"].as_bool()),
        (Some("failed"), Some(false))
    );
    let done = &rest.last().unwrap()["params"]["turn"];
    assert_eq!(done["status"], "interrupted");
    assert!(!methods(&rest).contains(&"item/agentMessage/delta".to_string()));
    // An answer that arrives after the request was withdrawn is recorded as late, nothing else.
    c.answer(&req["id"], json!({"contentItems": [], "success": true}));
    c.start_turn(&t, "next");
    assert_eq!(
        c.turn_lines().last().unwrap()["params"]["turn"]["status"],
        "completed"
    );
    c.close_stdin();
    c.finish();
    let o = observed(&dir);
    assert_eq!(o["answers"][0]["late"], true, "{}", o["answers"]);
    assert_eq!(o["answers"][0]["method"], "item/tool/call");
}

fn patch_steps(after: &str) -> Value {
    json!([{"patch": {"changes": [{"path": "p.txt", "kind": {"type": "add"}, "diff": "patched\n"}]}}, {"say": after}])
}

#[test]
fn interrupt_while_an_approval_waits_resolves_it() {
    let dir = Dir::new("b1patch");
    let mut c = Client::start(
        &dir,
        Some(json!({"turns": [{"steps": patch_steps("never")}]})),
    );
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let turn = c.start_turn(&t, "edit");
    let req = until_request(&mut c, "item/fileChange/requestApproval");
    c.call("turn/interrupt", json!({"threadId": t, "turnId": turn}));
    let rest = c.turn_lines();
    let resolved = rest
        .iter()
        .find(|l| l["method"] == "serverRequest/resolved")
        .unwrap();
    assert_eq!(resolved["params"]["requestId"], req["id"]);
    assert_eq!(completed_item(&rest, "fileChange")["status"], "declined");
    assert_eq!(
        rest.last().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
    assert!(
        !dir.work().join("p.txt").exists(),
        "an unanswered patch writes nothing"
    );
}

#[test]
fn cancel_declines_the_patch_and_interrupts_the_turn() {
    let dir = Dir::new("cancel");
    // The patch is the last step: nothing after it would notice an interrupt.
    let steps = json!([patch_steps("unused")[0].clone()]);
    let mut c = Client::start(
        &dir,
        Some(json!({"turns": [{"steps": steps}, {"steps": patch_steps("after cancel")}]})),
    );
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    c.start_turn(&t, "edit");
    let req = until_request(&mut c, "item/fileChange/requestApproval");
    c.answer(&req["id"], json!({"decision": "cancel"}));
    let rest = c.turn_lines();
    assert_eq!(completed_item(&rest, "fileChange")["status"], "declined");
    assert_eq!(
        rest.last().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
    assert!(!dir.work().join("p.txt").exists());
    // With a step after it, that step is skipped.
    c.start_turn(&t, "edit");
    let req = until_request(&mut c, "item/fileChange/requestApproval");
    c.answer(&req["id"], json!({"decision": "cancel"}));
    let rest = c.turn_lines();
    assert_eq!(
        rest.last().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
    assert!(
        !rest
            .iter()
            .any(|l| l["params"]["item"]["text"] == "after cancel")
    );
}

fn read_items(c: &mut Client, t: &str) -> Vec<Vec<String>> {
    let r = c.call("thread/read", json!({"threadId": t, "includeTurns": true}));
    r["thread"]["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|turn| {
            turn["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["type"].as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

#[test]
fn a_turn_holds_its_user_message_once() {
    let dir = Dir::new("b2");
    let mut c = Client::start(&dir, None);
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let r = c.call("turn/start", json!({"threadId": t, "input": text("hi")}));
    // As 0.167.0: the response's turn has not started yet.
    assert_eq!(r["turn"]["startedAt"], Value::Null);
    let lines = c.turn_lines();
    let done = &lines.last().unwrap()["params"]["turn"];
    // As 0.167.0's turn/completed: the items are not carried; thread/read has them.
    assert_eq!(
        (done["items"].clone(), done["itemsView"].as_str()),
        (json!([]), Some("notLoaded"))
    );
    assert_eq!(read_items(&mut c, &t), [["userMessage", "agentMessage"]]);
}

#[test]
fn interrupt_errors_are_the_real_ones() {
    let dir = Dir::new("interr");
    let mut c = Client::start(
        &dir,
        Some(json!({"turns": [{"steps": [{"expect": "interrupt"}]}]})),
    );
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let e = c.call_err("turn/interrupt", json!({"threadId": t, "turnId": t}));
    assert_eq!(
        (e["code"].as_i64(), e["message"].as_str()),
        (Some(-32600), Some("no active turn to interrupt"))
    );
    let turn = c.start_turn(&t, "x");
    let e = c.call_err("turn/interrupt", json!({"threadId": t, "turnId": t}));
    assert_eq!(
        e["message"],
        format!("expected active turn id {t} but found {turn}")
    );
    let other = "01a1d975-8b3e-7243-b450-777afc2c2824";
    let e = c.call_err("turn/interrupt", json!({"threadId": other, "turnId": turn}));
    assert_eq!(e["message"], format!("thread not found: {other}"));
    let e = c.call_err("turn/interrupt", json!({"threadId": t}));
    assert_eq!(e["message"], "Invalid request: missing field `turnId`");
    c.call("turn/interrupt", json!({"threadId": t, "turnId": turn}));
    assert_eq!(
        c.turn_lines().last().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
}

#[test]
fn turn_start_during_a_turn_joins_it() {
    let dir = Dir::new("join");
    let mut c = Client::start(
        &dir,
        Some(json!({"turns": [{"steps": [{"expect": "interrupt"}]}]})),
    );
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let turn = c.start_turn(&t, "first");
    c.until("turn/started");
    let r = c.call(
        "turn/start",
        json!({"threadId": t, "input": text("second")}),
    );
    assert_eq!(
        (r["turn"]["id"].as_str(), r["turn"]["status"].as_str()),
        (Some(turn.as_str()), Some("inProgress"))
    );
    let joined = loop {
        let v = c.until("item/completed");
        if v["params"]["item"]["content"][0]["text"] == "second" {
            break v;
        }
    };
    assert_eq!(joined["params"]["turnId"], turn.as_str());
    assert_eq!(joined["params"]["item"]["content"][0]["text"], "second");
    c.call("turn/interrupt", json!({"threadId": t, "turnId": turn}));
    c.turn_lines();
    assert_eq!(read_items(&mut c, &t), [["userMessage", "userMessage"]]);
}

#[test]
fn revert_of_a_running_turn_interrupts_it_first() {
    let dir = Dir::new("revertrun");
    let mut c = Client::start(
        &dir,
        Some(json!({"turns": [{"steps": [{"say": "one"}]}, {"steps": [{"expect": "interrupt"}]}]})),
    );
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    let first = c.start_turn(&t, "a");
    c.turn_lines();
    let e = c.call_err(
        "thread/revert",
        json!({"threadId": t, "beforeTurnId": "nope"}),
    );
    assert_eq!(
        e,
        json!({"code": -32600, "message": "turn not found: nope"})
    );
    let second = c.start_turn(&t, "b");
    c.until("turn/started");
    let id = c.request(
        "thread/revert",
        json!({"threadId": t, "beforeTurnId": second}),
    );
    let mut lines = vec![];
    loop {
        let v = c.next();
        let reverted = v["method"] == "thread/reverted";
        lines.push(v);
        if reverted {
            break;
        }
    }
    let at = |pred: &dyn Fn(&Value) -> bool| lines.iter().position(pred).unwrap();
    let completed = at(&|l| l["method"] == "turn/completed");
    let response = at(&|l| l["id"] == id && l.get("method").is_none());
    assert!(completed < response, "{:?}", methods(&lines));
    assert_eq!(lines[completed]["params"]["turn"]["status"], "interrupted");
    // As the schema says, the response's `turns` is always empty.
    assert_eq!(lines[response]["result"]["thread"]["turns"], json!([]));
    assert_eq!(
        lines[response]["result"]["turnsBackwardsCursor"],
        Value::Null
    );
    let r = c.call("thread/read", json!({"threadId": t, "includeTurns": true}));
    let ids: Vec<&str> = r["thread"]["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [first.as_str()]);
}

#[test]
fn a_failed_turn_ends_as_the_capture_shows() {
    let dir = Dir::new("failorder");
    let mut c = Client::start(&dir, Some(script_file("not-logged-in.json")));
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    c.start_turn(&t, "hi");
    let lines = c.turn_lines();
    let m = methods(&lines);
    assert_eq!(
        &m[m.len() - 3..],
        ["thread/status/changed", "error", "turn/completed"],
        "{m:?}"
    );
    assert_eq!(
        lines[lines.len() - 3]["params"]["status"],
        json!({"type": "systemError"})
    );
    let last_err = &lines[lines.len() - 2]["params"];
    assert_eq!(last_err["willRetry"], false);
    assert_eq!(
        last_err["error"]["codexErrorInfo"],
        json!({"httpConnectionFailed": {"httpStatusCode": 401}})
    );
    let retries: Vec<&str> = lines
        .iter()
        .filter(|l| l["method"] == "error" && l["params"]["willRetry"] == true)
        .map(|l| l["params"]["error"]["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        retries,
        [
            "Reconnecting... 2/5",
            "Reconnecting... 3/5",
            "Reconnecting... 4/5",
            "Reconnecting... 5/5",
            "Reconnecting... 1/5",
            "Reconnecting... 2/5",
            "Reconnecting... 3/5",
            "Reconnecting... 4/5",
            "Reconnecting... 5/5"
        ]
    );
    let fallback = lines
        .iter()
        .position(|l| l["method"] == "warning")
        .expect("the HTTPS fallback warning");
    assert!(
        lines[fallback]["params"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Falling back from WebSockets to HTTPS transport.")
    );
    assert!(!m.iter().any(|x| x == "item/agentMessage/delta"));
}

#[test]
fn an_accepted_patch_reaches_the_disk() {
    let dir = Dir::new("patchdisk");
    dir.write("work/src/a.ts", "a\nkeep\n");
    dir.write("work/gone.txt", "bye\n");
    dir.write("work/old.txt", "1\n2\n");
    let changes = json!([
        {"path": "src/a.ts", "kind": {"type": "update", "move_path": null}, "diff": "@@ -1,2 +1,2 @@\n-a\n+b\n keep\n"},
        {"path": "new.txt", "kind": {"type": "add"}, "diff": "hi\n"},
        {"path": "gone.txt", "kind": {"type": "delete"}, "diff": "bye\n"},
        {"path": dir.work().join("old.txt"), "kind": {"type": "update", "move_path": "moved/new.txt"}, "diff": "@@ -2 +2 @@\n-2\n+two\n"}]);
    let script = json!({"turns": [{"steps": [{"patch": {"changes": changes}}]},
        {"steps": [{"patch": {"changes": [{"path": "never.txt", "kind": {"type": "add"}, "diff": "x"}]}}]}]});
    let mut c = Client::start(&dir, Some(script));
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    c.start_turn(&t, "edit");
    let req = until_request(&mut c, "item/fileChange/requestApproval");
    assert_eq!(
        std::fs::read_to_string(dir.work().join("src/a.ts")).unwrap(),
        "a\nkeep\n",
        "nothing before the answer"
    );
    c.answer(&req["id"], json!({"decision": "accept"}));
    let rest = c.turn_lines();
    assert_eq!(completed_item(&rest, "fileChange")["status"], "completed");
    let read = |p: &str| std::fs::read_to_string(dir.work().join(p)).ok();
    assert_eq!(read("src/a.ts").as_deref(), Some("b\nkeep\n"));
    assert_eq!(read("new.txt").as_deref(), Some("hi\n"));
    assert_eq!(read("gone.txt"), None);
    assert_eq!(read("old.txt"), None);
    assert_eq!(read("moved/new.txt").as_deref(), Some("1\ntwo\n"));
    c.start_turn(&t, "edit again");
    let req = until_request(&mut c, "item/fileChange/requestApproval");
    c.answer(&req["id"], json!({"decision": "decline"}));
    c.turn_lines();
    assert_eq!(read("never.txt"), None, "a declined patch writes nothing");
}

#[test]
fn a_patch_that_does_not_apply_fails_its_item() {
    let dir = Dir::new("patchbad");
    dir.write("work/a.txt", "x\n");
    let script = json!({"turns": [{"steps": [{"patch": {"changes": [
        {"path": "a.txt", "kind": {"type": "update", "move_path": null}, "diff": "@@ -1 +1 @@\n-nope\n+y\n"}]}}]}]});
    let mut c = Client::start(&dir, Some(script));
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({})));
    c.start_turn(&t, "edit");
    let req = until_request(&mut c, "item/fileChange/requestApproval");
    c.answer(&req["id"], json!({"decision": "accept"}));
    let rest = c.turn_lines();
    assert_eq!(completed_item(&rest, "fileChange")["status"], "failed");
    assert_eq!(
        std::fs::read_to_string(dir.work().join("a.txt")).unwrap(),
        "x\n"
    );
}

#[cfg(unix)]
#[test]
fn a_crash_then_a_resume_plays_the_next_turn() {
    let dir = Dir::new("crashresume");
    let script = script_file("crash-then-resume.json");
    let thread;
    {
        let mut c = Client::start(&dir, Some(script.clone()));
        c.initialize();
        thread = tid(&c.start_thread(&dir, json!({})));
        c.start_turn(&thread, "first");
        assert!(c.child.wait().is_ok(), "turn 0 crashes the process");
    }
    let mut c = Client::start(&dir, Some(script));
    c.initialize();
    let r = c.call("thread/resume", json!({"threadId": thread}));
    let turns = r["thread"]["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1, "the crashed turn is in the history");
    assert_eq!(turns[0]["status"], "interrupted");
    assert_eq!(turns[0]["items"][0]["type"], "userMessage");
    c.start_turn(&thread, "again");
    let lines = c.turn_lines();
    assert!(
        lines
            .iter()
            .any(|l| l["params"]["item"]["text"] == "resumed: turn 1"),
        "{:?}",
        methods(&lines)
    );
}

#[test]
fn request_params_are_checked_like_the_real_server() {
    let dir = Dir::new("params");
    let mut c = Client::start(&dir, None);
    c.call("initialize", json!({"clientInfo": {"name": "finisher", "version": "0"}, "capabilities": {"experimentalApi": false}}));
    let w = dir.work();
    let e = c.call_err(
        "thread/start",
        json!({"cwd": w, "multiAgentMode": "proactive"}),
    );
    assert_eq!(
        e["message"],
        "thread/start.multiAgentMode requires experimentalApi capability"
    );
    drop(c);
    let mut c = Client::start(&dir, None);
    c.initialize();
    let tool = |m: Value| {
        let mut t = Client::shell_tool();
        for (k, v) in m.as_object().unwrap() {
            if v.is_null() {
                t.as_object_mut().unwrap().remove(k);
            } else {
                t[k] = v.clone();
            }
        }
        t
    };
    for (params, want) in [
        (
            json!({"sandbox": "readOnly"}),
            "Invalid request: unknown variant `readOnly`, expected one of `read-only`, `workspace-write`, `danger-full-access`",
        ),
        (
            json!({"approvalPolicy": "sometimes"}),
            "Invalid request: unknown variant `sometimes`, expected one of `untrusted`, `on-request`, `granular`, `never`",
        ),
        (
            json!({"approvalsReviewer": "bogus"}),
            "Invalid request: unknown variant `bogus`, expected one of `user`, `auto_review`, `guardian_subagent`",
        ),
        (
            json!({"multiAgentMode": "bogus"}),
            "Invalid request: unknown variant `bogus`, expected one of `none`, `custom`, `explicitRequestOnly`, `proactive`",
        ),
        (
            json!({"cwd": 5}),
            "Invalid request: invalid type: integer `5`, expected a string",
        ),
        (
            json!({"ephemeral": "yes"}),
            "Invalid request: invalid type: string \"yes\", expected a boolean",
        ),
        (
            json!({"dynamicTools": "x"}),
            "Invalid request: invalid type: string \"x\", expected a sequence",
        ),
        (
            json!({"dynamicTools": [tool(json!({"description": null}))]}),
            "Invalid request: missing field `description`",
        ),
        (
            json!({"dynamicTools": [tool(json!({"inputSchema": null}))]}),
            "Invalid request: missing field `inputSchema`",
        ),
        (
            json!({"dynamicTools": [tool(json!({"name": null}))]}),
            "Invalid request: missing field `name`",
        ),
        (
            json!({"dynamicTools": [Client::shell_tool(), Client::shell_tool()]}),
            "duplicate dynamic tool name: shell",
        ),
        (
            json!({"dynamicTools": [tool(json!({"name": ""}))]}),
            "dynamic tool name must not be empty",
        ),
        (
            json!({"dynamicTools": [tool(json!({"inputSchema": 5}))]}),
            "dynamic tool input schema is not supported for shell: invalid type: number, expected struct JsonSchema",
        ),
    ] {
        let mut p = json!({"cwd": w});
        for (k, v) in params.as_object().unwrap() {
            p[k] = v.clone();
        }
        let e = c.call_err("thread/start", p.clone());
        assert_eq!(
            (e["code"].as_i64(), e["message"].as_str()),
            (Some(-32600), Some(want)),
            "{p}"
        );
    }
    // `type` is not required on a dynamic tool (0.167.0 takes it without).
    let t = tid(&c.start_thread(&dir, json!({"dynamicTools": [tool(json!({"type": null}))]})));
    for (input, want) in [
        (
            json!("hello"),
            "Invalid request: invalid type: string \"hello\", expected a sequence",
        ),
        (
            json!([{"type": "bogus"}]),
            "Invalid request: unknown variant `bogus`, expected one of `text`, `image`, `localImage`, `audio`, `localAudio`, `skill`, `mention`",
        ),
        (
            json!([{"type": "text"}]),
            "Invalid request: missing field `text`",
        ),
        (
            json!([{"type": "text", "text": 5}]),
            "Invalid request: invalid type: integer `5`, expected a string",
        ),
    ] {
        let e = c.call_err("turn/start", json!({"threadId": t, "input": input}));
        assert_eq!(e["message"], want, "{input}");
    }
    assert_eq!(
        c.call_err("turn/start", json!({"threadId": t}))["message"],
        "Invalid request: missing field `input`"
    );
    let e = c.call_err(
        "turn/steer",
        json!({"threadId": t, "expectedTurnId": "x", "input": "x"}),
    );
    assert_eq!(
        e["message"],
        "Invalid request: invalid type: string \"x\", expected a sequence"
    );
    let e = c.call_err("thread/revert", json!({"threadId": t}));
    assert_eq!(
        e["message"],
        "Invalid request: missing field `beforeTurnId`"
    );
    // Ids are UUIDs.
    let bad = "invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `n` at 1";
    for (m, p) in [
        ("thread/read", json!({"threadId": "not-a-uuid"})),
        ("turn/start", json!({"threadId": "not-a-uuid", "input": []})),
        (
            "thread/revert",
            json!({"threadId": "not-a-uuid", "beforeTurnId": "x"}),
        ),
    ] {
        assert_eq!(
            c.call_err(m, p)["message"],
            format!("invalid thread id: {bad}"),
            "{m}"
        );
    }
    assert_eq!(
        c.call_err("thread/resume", json!({"threadId": "not-a-uuid"}))["message"],
        format!("invalid session id: {bad}")
    );
}

#[test]
fn malformed_answers_are_recorded_and_count_as_errors() {
    let dir = Dir::new("badanswer");
    let script = json!({"turns": [{"steps": [
        {"dynamicTool": {"tool": "shell", "arguments": {"command": "ls"}}},
        {"patch": {"changes": [{"path": "b.txt", "kind": {"type": "add"}, "diff": "b\n"}]}}]}]});
    let mut c = Client::start(&dir, Some(script));
    c.initialize();
    let t = tid(&c.start_thread(&dir, json!({"dynamicTools": [Client::shell_tool()]})));
    c.start_turn(&t, "go");
    let req = until_request(&mut c, "item/tool/call");
    c.answer(&req["id"], json!({"success": true}));
    let req2 = until_request(&mut c, "item/fileChange/requestApproval");
    c.answer(&req2["id"], json!({"decision": "maybe"}));
    let rest = c.turn_lines();
    assert_eq!(completed_item(&rest, "fileChange")["status"], "declined");
    assert!(!dir.work().join("b.txt").exists());
    c.close_stdin();
    c.finish();
    let o = observed(&dir);
    let bad = o["badAnswers"].as_array().expect("badAnswers");
    assert_eq!(bad.len(), 2, "{bad:?}");
    assert_eq!(
        (bad[0]["method"].as_str(), bad[0]["id"].clone()),
        (Some("item/tool/call"), req["id"].clone())
    );
    assert!(
        bad[0]["reason"].as_str().unwrap().contains("contentItems"),
        "{bad:?}"
    );
    assert!(
        bad[1]["reason"].as_str().unwrap().contains("maybe"),
        "{bad:?}"
    );
}

#[test]
fn small_differences_from_the_real_server() {
    let dir = Dir::new("nits");
    let mut c = Client::start_logged(&dir, None, D4_ARGS);
    c.initialize();
    // A line that is not JSON is logged, never answered.
    c.send_raw("this is not json");
    c.call("config/read", json!({}));
    assert!(
        !c.log
            .iter()
            .any(|(l, _)| l["id"].is_null() && l.get("error").is_some())
    );
    let err = std::fs::read_to_string(dir.path("stderr.log")).unwrap();
    assert!(
        err.contains("Failed to deserialize JSONRPCMessage"),
        "{err}"
    );
    // A line past the frame limit is dropped; the server goes on.
    let huge = format!(
        "{{\"id\":99,\"method\":\"config/read\",\"params\":{{\"x\":\"{}\"}}}}",
        "a".repeat(17 << 20)
    );
    c.send_raw(&huge);
    c.call("config/read", json!({}));
    assert!(!c.log.iter().any(|(l, _)| l["id"] == 99));
    // Unknown, unloaded and stored threads.
    let unknown = "01a1d975-8b3e-7243-b450-777afc2c2824";
    assert_eq!(
        c.call_err("thread/read", json!({"threadId": unknown}))["message"],
        format!("thread not loaded: {unknown}")
    );
    assert_eq!(
        c.call_err(
            "turn/steer",
            json!({"threadId": unknown, "expectedTurnId": "x", "input": []})
        )["message"],
        format!("thread not found: {unknown}")
    );
    // model/list cursors.
    assert_eq!(
        c.call_err("model/list", json!({"cursor": "zzz"}))["message"],
        "invalid cursor: zzz"
    );
    let n = c.call("model/list", json!({}))["data"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(
        c.call_err("model/list", json!({"cursor": "100"}))["message"],
        format!("cursor 100 exceeds total models {n}")
    );
    let t = tid(&c.start_thread(&dir, json!({})));
    c.start_turn(&t, "hi");
    c.turn_lines();
    assert_eq!(
        c.call("thread/read", json!({"threadId": t}))["thread"]["status"],
        json!({"type": "idle"})
    );
    drop(c);
    let mut c = Client::start(&dir, None);
    c.initialize();
    let r = c.call("thread/read", json!({"threadId": t, "includeTurns": true}));
    assert_eq!(
        r["thread"]["status"],
        json!({"type": "notLoaded"}),
        "a stored thread is readable, not loaded"
    );
    assert_eq!(r["thread"]["turns"].as_array().unwrap().len(), 1);
}

#[test]
fn observed_file_lock_of_a_live_holder_is_never_broken() {
    let dir = Dir::new("livelock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.path("observed.json.lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut kid = command(&dir, None, &["app-server"], &[])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2600));
    assert!(
        !dir.path("observed.json").exists(),
        "the fake wrote while another process held the lock"
    );
    drop(lock);
    assert!(kid.wait().unwrap().success());
    let all = observed_all(&dir);
    assert_eq!(
        (all.len(), all[0]["exit"]["reason"].as_str()),
        (1, Some("eof"))
    );
}

#[test]
fn observed_file_keeps_every_process_under_contention() {
    let dir = Dir::new("lock");
    // A lock file a dead process left behind.
    dir.write("observed.json.lock", "");
    let n = 12;
    let kids: Vec<_> = (0..n)
        .map(|_| {
            command(&dir, None, &["app-server"], &[])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut k in kids {
        assert!(k.wait().unwrap().success());
    }
    let all = observed_all(&dir);
    assert_eq!(all.len(), n);
    assert!(
        all.iter().all(|r| r["exit"]["reason"] == "eof"),
        "an update was lost: {all:?}"
    );
}

#[test]
fn the_example_scripts_parse() {
    let dir = Dir::new("examples");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/codex");
    let mut seen = 0;
    for e in std::fs::read_dir(&root).unwrap() {
        let p = e.unwrap().path();
        let script: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let (code, out, err) = run_to_end_with(&dir, Some(&script), &["--version"]);
        assert_eq!(code, Some(0), "{}: {err}", p.display());
        assert!(out.starts_with("codex-cli "));
        seen += 1;
    }
    assert!(seen >= 5, "{seen}");
}
