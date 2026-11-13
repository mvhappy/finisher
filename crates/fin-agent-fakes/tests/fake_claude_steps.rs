//! `fake-claude` script steps and turn boundaries: side effects, crashes, the unsigned turn,
//! steers, stdin EOF mid-turn, and the interrupt knobs.

mod common;

use std::process::Command;

use common::*;
use serde_json::json;

#[test]
fn fake_spawn_runs_and_records_exit_code() {
    let dir = Dir::new("spawn");
    let script = json!({"steps": [{"expect": "user",
        "spawn": {"argv": [BIN, "-p", "--bogus"]}, "emit": [{"type": "system", "subtype": "spawned"}]}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(
        f.read()["subtype"],
        "spawned",
        "the spawn finished before the step went on"
    );
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    let obs = observed_all(&dir);
    let parent = obs
        .iter()
        .find(|r| r["spawned"].is_array())
        .expect("the parent's record");
    assert_eq!(
        parent["spawned"],
        json!([{"argv": [BIN, "-p", "--bogus"], "code": 1}])
    );
    assert_eq!(
        obs.len(),
        2,
        "the child is a fake too and appended its own record"
    );
}

#[test]
fn fake_write_file_writes_before_emitting() {
    let dir = Dir::new("write");
    let target = dir.path("out.txt");
    let script = json!({"steps": [{"expect": "user",
        "writeFile": {"path": target, "contents": "hello"}, "emit": [{"type": "system", "subtype": "wrote"}]}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    assert_eq!(f.read()["subtype"], "wrote");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    assert!(f.finish().success());
}

#[test]
fn fake_crash_exits_with_code_or_signal() {
    let dir = Dir::new("crash");
    let mut f = Fake::start(
        &dir,
        Some(json!({"steps": [{"expect": "user", "crash": {"code": 7}}]})),
        &[],
        &[],
    );
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(f.child.wait().unwrap().code(), Some(7));
    assert!(
        f.drain().iter().all(|l| l["type"] != "result"),
        "no result after a crash"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let script = json!({"steps": [{"expect": "user",
            "emit": [{"type": "system", "subtype": "partial"}], "crash": {"signal": "KILL"}}]});
        let mut f = Fake::start(&dir, Some(script), &[], &[]);
        f.user("go");
        f.read();
        assert_eq!(f.read()["subtype"], "partial");
        assert_eq!(f.child.wait().unwrap().signal(), Some(9));
    }
}

#[test]
fn fake_not_logged_in_turn_matches_the_cli() {
    let dir = Dir::new("unsigned");
    let script = json!({"initialize": {"account": {"tokenSource": "none", "apiProvider": "firstParty"}},
        "steps": [{"expect": "user", "notLoggedIn": true}]});
    let mut f = Fake::start(&dir, Some(script), &["--session-id", UUID], &[]);
    assert_eq!(initialize(&mut f)["account"]["tokenSource"], "none");
    f.user("hi");
    assert_eq!(f.read_any()["subtype"], "init");
    let status = f.read_any();
    assert_eq!(
        (status["subtype"].as_str(), status["status"].as_str()),
        (Some("status"), Some("requesting"))
    );
    let a = f.read_any();
    assert_eq!(a["type"], "assistant");
    assert_eq!(a["message"]["model"], "<synthetic>");
    assert_eq!(a["error"], "authentication_failed");
    assert_eq!(a["is_api_error_message"], true);
    assert_eq!(
        a["message"]["content"][0]["text"],
        "Not logged in · Please run /login"
    );
    let r = f.read_any();
    assert_eq!(r["type"], "result");
    assert_eq!(r["subtype"], "success");
    assert_eq!(r["is_error"], true);
    assert_eq!(r["result"], "Not logged in · Please run /login");
    assert_eq!(r["modelUsage"], json!({}));
    assert_eq!(r["terminal_reason"], "api_error");
    assert!(r["api_error_status"].is_null() && r.get("api_error_status").is_some());
    assert_eq!(r["total_cost_usd"], 0.0);
    assert_eq!(r["queued_turn_count"], 0);
    assert_eq!(
        f.finish().code(),
        Some(1),
        "EOF after an is_error turn exits 1"
    );
}

#[test]
fn fake_unknown_line_has_a_type_no_cli_has() {
    let dir = Dir::new("unknown");
    let mut f = Fake::start(
        &dir,
        Some(json!({"steps": [{"expect": "user", "unknownLine": true}]})),
        &[],
        &[],
    );
    f.user("go");
    f.read();
    let l = f.read();
    assert_eq!(l["type"], "fake_future_type");
    assert!(l["session_id"].is_string());
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
}

#[test]
fn fake_steer_is_queued_and_counted() {
    let dir = Dir::new("steer");
    let script = json!({"steps": [{"expect": "user", "sleep": 400, "cumulativeUsage": {"outputTokens": 1}, "text": "first"}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("one");
    assert_eq!(f.read()["subtype"], "init");
    f.user("steer");
    let first = f.read();
    assert_eq!(
        (
            first["result"].as_str(),
            first["queued_turn_count"].as_u64()
        ),
        (Some("first"), Some(1))
    );
    assert_eq!(f.read()["subtype"], "init", "every turn has its init");
    let second = f.read();
    assert_eq!(
        second["type"], "result",
        "the queued line runs as the next turn"
    );
    assert_eq!(second["queued_turn_count"], 0);
    assert!(f.finish().success());
}

#[test]
fn fake_eof_mid_turn_plays_the_turn_to_its_result() {
    let dir = Dir::new("eof");
    let script = json!({"steps": [
        {"expect": "user", "sleep": 300, "emit": [{"type": "system", "subtype": "after_sleep"}]},
        {"notLoggedIn": true},
        {"expect": "user", "emit": [{"type": "system", "subtype": "never"}]},
    ]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.stdin.take();
    let st = f.child.wait().unwrap();
    assert_eq!(st.code(), Some(1), "an is_error turn then EOF exits 1");
    let lines = f.drain();
    let kinds: Vec<String> = lines
        .iter()
        .map(|l| {
            format!(
                "{}/{}",
                l["type"].as_str().unwrap_or(""),
                l["subtype"].as_str().unwrap_or("")
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "system/ui_invalidate",
            "system/init",
            "system/status",
            "system/after_sleep",
            "assistant/",
            "result/success"
        ],
        "{lines:?}"
    );
    // A turn that ends without an error exits 0.
    let mut f = Fake::start(
        &dir,
        Some(json!({"steps": [{"expect": "user", "sleep": 200}]})),
        &[],
        &[],
    );
    f.user("go");
    f.stdin.take();
    assert!(f.child.wait().unwrap().success());
    assert!(f.drain().iter().any(|l| l["type"] == "result"));
}

#[test]
fn fake_interrupt_knobs() {
    // An interrupt with no turn open: answered, no result.
    let dir = Dir::new("knobs");
    let mut f = Fake::start(&dir, None, &[], &[]);
    f.control("i-1", "interrupt");
    assert_eq!(f.read()["response"]["request_id"], "i-1");
    assert!(f.silent_for(300), "no result without a turn");
    assert!(f.finish().success());

    // ignore: answered, and the turn plays on.
    let script = json!({"onInterrupt": "ignore", "steps": [{"expect": "user", "sleep": 300,
        "emit": [{"type": "system", "subtype": "kept_going"}]}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    f.control("i-2", "interrupt");
    assert_eq!(f.read()["response"]["request_id"], "i-2");
    assert_eq!(f.read()["subtype"], "kept_going");
    let r = f.read();
    assert_eq!(
        (r["type"].as_str(), r["is_error"].as_bool()),
        (Some("result"), Some(false))
    );
    assert!(f.finish().success());

    // result (the default): the turn ends, the process stays.
    let script = json!({"steps": [{"expect": "user", "sleep": 60000}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    f.control("i-3", "interrupt");
    f.read();
    assert_eq!(f.read()["subtype"], "error_during_execution");
    f.user("again");
    assert_eq!(f.read()["subtype"], "init", "every turn has its init");
    assert_eq!(f.read()["is_error"], false);
    assert!(f.finish().success());
}

#[cfg(unix)]
#[test]
fn fake_sigint_knobs() {
    let dir = Dir::new("sigknobs");
    let kill = |f: &Fake| {
        assert!(
            Command::new("kill")
                .args(["-INT", &f.child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
    };
    let script = json!({"onSigint": "exit", "steps": [{"expect": "user", "sleep": 60000}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    kill(&f);
    assert_eq!(f.read()["subtype"], "error_during_execution");
    assert_eq!(f.child.wait().unwrap().code(), Some(130));

    let script = json!({"onSigint": "ignore", "steps": [{"expect": "user", "sleep": 400,
        "emit": [{"type": "system", "subtype": "kept_going"}]}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    kill(&f);
    assert_eq!(f.read()["subtype"], "kept_going");
    assert!(f.finish().success());
}

#[test]
fn fake_spawn_in_on_allow_runs_only_when_allowed() {
    let dir = Dir::new("allowspawn");
    let step = json!({"expect": "user", "ask": {"tool": "Bash", "input": {"command": "fin view"}},
        "onAllow": {"spawn": {"argv": [BIN, "--version"]}, "emit": [{"type": "system", "subtype": "ran"}]},
        "onDeny": [{"type": "system", "subtype": "refused"}]});
    let mut f = Fake::start(&dir, Some(json!({"steps": [step.clone(), step]})), &[], &[]);
    f.user("one");
    f.read();
    let req = f.read();
    f.answer(&req, json!({"behavior": "deny", "message": "no"}));
    assert_eq!(f.read()["subtype"], "refused");
    assert_eq!(f.read()["type"], "result");
    f.user("two");
    assert_eq!(f.read()["subtype"], "init", "every turn has its init");
    let req = f.read();
    f.answer(
        &req,
        json!({"behavior": "allow", "updatedInput": {"command": "fin view"}}),
    );
    assert_eq!(f.read()["subtype"], "ran");
    assert!(f.finish().success());
    let obs = observed_all(&dir);
    let parent = obs.iter().find(|r| r["answers"].is_array()).unwrap();
    assert_eq!(
        parent["spawned"].as_array().map(Vec::len),
        Some(1),
        "{parent}"
    );
    assert_eq!(parent["spawned"][0]["code"], 0);
    assert_eq!(parent["answers"][1]["answer"]["behavior"], "allow");
}

#[test]
fn fake_bash_announces_runs_and_reports_the_result() {
    let dir = Dir::new("bash");
    let marker = dir.path("ran.txt");
    let bash =
        |input: serde_json::Value| json!({"ask": {"tool": "Bash", "input": input, "bash": true}});
    let script = json!({"steps": [
        {"expect": "user", "ask": {"tool": "Bash", "bash": true, "toolUseId": "toolu_a",
            "input": {"command": format!("echo hi; echo x > '{}'", marker.display()), "description": "d"}}},
        bash(json!({"command": "sleep 5; echo late", "run_in_background": true})),
        bash(json!({"command": "echo never"})),
    ]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    // The call is announced before its prompt, with the prompt's tool_use_id.
    let announced = f.read();
    assert_eq!(announced["type"], "assistant");
    assert_eq!(announced["message"]["content"][0]["id"], "toolu_a");
    assert_eq!(announced["message"]["content"][0]["name"], "Bash");
    let req = f.read();
    assert_eq!(req["request"]["tool_use_id"], "toolu_a");
    f.answer(
        &req,
        json!({"behavior": "allow", "updatedInput": req["request"]["input"]}),
    );
    let result = f.read();
    let block = &result["message"]["content"][0];
    assert_eq!(
        (result["type"].as_str(), block["tool_use_id"].as_str()),
        (Some("user"), Some("toolu_a"))
    );
    assert_eq!(
        (block["content"].as_str(), block["is_error"].as_bool()),
        (Some("hi\n"), Some(false))
    );
    assert!(marker.exists(), "the command ran before its result");
    // In the background: the result comes at once, before the process ends.
    let _ = f.read();
    let req = f.read();
    let started = std::time::Instant::now();
    f.answer(
        &req,
        json!({"behavior": "allow", "updatedInput": req["request"]["input"]}),
    );
    let result = f.read();
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    let text = result["message"]["content"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(text.starts_with("Command running in background"), "{text}");
    // Denied: an error result, nothing run.
    let _ = f.read();
    let req = f.read();
    f.answer(&req, json!({"behavior": "deny", "message": "no"}));
    let result = f.read();
    assert_eq!(result["message"]["content"][0]["is_error"], true);
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    let spawned = observed(&dir)["spawned"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(spawned.len(), 2);
    assert_eq!(spawned[1]["background"], true);
}

#[test]
fn fake_bash_in_background_is_rejected_while_background_tasks_are_disabled() {
    let dir = Dir::new("bash-bg-off");
    let marker = dir.path("bg.txt");
    let command = format!("echo x > '{}'", marker.display());
    let script = json!({"steps": [{"expect": "user",
        "ask": {"tool": "Bash", "bash": true, "input": {"command": command, "run_in_background": true}}}]});
    let mut f = Fake::start(
        &dir,
        Some(script),
        &[],
        &[("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1")],
    );
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(f.read()["type"], "assistant");
    // No prompt: the unknown parameter is refused at once (verified 2.1.321, Task 0).
    let result = f.read();
    let block = &result["message"]["content"][0];
    assert_eq!(block["is_error"], true);
    assert!(
        block["content"]
            .as_str()
            .unwrap_or_default()
            .contains("run_in_background")
    );
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    assert!(!marker.exists(), "nothing ran");
}

#[test]
fn fake_resume_reasks_a_dangling_call_only_under_reask() {
    let dir = Dir::new("dangling");
    // A process killed mid-tool: its transcript holds the call and no result.
    let use_line = json!({"type": "assistant", "session_id": "$session_id", "parent_tool_use_id": null,
        "message": {"id": "m1", "role": "assistant", "model": "m", "content": [
            {"type": "tool_use", "id": "toolu_D", "name": "Bash", "input": {"command": "fin bin add"}}]}});
    let first =
        json!({"steps": [{"expect": "user", "emit": [use_line], "crash": {"signal": "KILL"}}]});
    let mut f = Fake::start(&dir, Some(first), &["--session-id", UUID], &[]);
    f.user("go");
    f.read();
    assert_eq!(f.read()["type"], "assistant");
    assert!(!f.finish().success());

    let resume = format!("--resume={UUID}");
    let plain = |mode: &str| json!({"onResumeDangling": mode, "steps": [{"expect": "user", "cumulativeUsage": {"inputTokens": 1}}]});
    // `leave`: nothing is asked.
    let mut f = Fake::start(&dir, Some(plain("leave")), &[&resume], &[]);
    f.user("again");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    // `reask`: the same tool_use_id, run again only when allowed (here denied).
    let mut f = Fake::start(&dir, Some(plain("reask")), &[&resume], &[]);
    f.user("again");
    assert_eq!(f.read()["subtype"], "init");
    let req = f.read();
    assert_eq!(req["request"]["subtype"], "can_use_tool");
    assert_eq!(req["request"]["tool_use_id"], "toolu_D");
    f.answer(&req, json!({"behavior": "deny", "message": "no"}));
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    let all = observed_all(&dir);
    assert_eq!(all[1]["resume"]["dangling"], json!(["toolu_D"]));
    assert!(all[1]["rerunDenied"].is_null());
    assert_eq!(all[2]["rerunDenied"], json!(["toolu_D"]));
    assert!(all[2]["reran"].is_null());
}

/// Task 0 (verified 2.1.321, s1d/s1e): `--resume` restores the totals after a clean exit and
/// restarts them after a kill mid-turn.
#[test]
fn fake_resume_totals_follow_how_the_last_process_ended() {
    let turn = |cost: f64| json!({"steps": [{"expect": "user", "cumulativeUsage": {"inputTokens": 10, "costUsd": cost}}]});
    let resume = format!("--resume={UUID}");
    let total = |dir: &Dir| {
        let mut f = Fake::start(dir, Some(turn(0.02)), &[&resume], &[]);
        f.user("again");
        assert_eq!(f.read()["subtype"], "init");
        let r = f.read();
        assert!(f.finish().success());
        r["total_cost_usd"].as_f64().unwrap()
    };
    // A clean exit (stdin's end): the totals carry on.
    let clean = Dir::new("totals-clean");
    let mut f = Fake::start(&clean, Some(turn(0.01)), &["--session-id", UUID], &[]);
    f.user("go");
    f.read();
    f.read();
    assert!(f.finish().success());
    assert!((total(&clean) - 0.03).abs() < 1e-9);
    // Killed after its turn's result: they restart.
    let killed = Dir::new("totals-killed");
    let script = json!({"steps": [{"expect": "user", "cumulativeUsage": {"inputTokens": 10, "costUsd": 0.01}}, {"crash": {"signal": "KILL"}}]});
    let mut f = Fake::start(&killed, Some(script), &["--session-id", UUID], &[]);
    f.user("go");
    f.read();
    f.read();
    assert!(!f.finish().success());
    assert!((total(&killed) - 0.02).abs() < 1e-9);
}
