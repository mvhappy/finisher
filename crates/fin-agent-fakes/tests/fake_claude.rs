//! `fake-claude` against its own binary: handshake, flags, control protocol, resume.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use serde_json::{Value, json};

fn one_turn(delta: Value) -> Value {
    json!({"steps": [{"expect": "user", "cumulativeUsage": delta, "text": "hi"}]})
}

const AGENTS: [&str; 5] = [
    "claude",
    "Explore",
    "general-purpose",
    "Plan",
    "statusline-setup",
];

#[test]
fn fake_answers_initialize_before_prompt() {
    let dir = Dir::new("init");
    let mut f = Fake::start(&dir, None, &["--session-id", UUID], &[]);
    f.control("req-1", "initialize");
    let ui = f.read_any();
    assert_eq!(
        (ui["type"].as_str(), ui["subtype"].as_str()),
        (Some("system"), Some("ui_invalidate"))
    );
    assert_eq!(ui["session_id"], UUID);
    let r = f.read_any();
    assert_eq!(r["type"], "control_response");
    assert_eq!(r["response"]["subtype"], "success");
    assert_eq!(r["response"]["request_id"], "req-1");
    assert_eq!(r["response"]["pending_permission_requests"], json!([]));
    let body = &r["response"]["response"];
    assert_eq!(body["commands"], json!([]));
    assert_eq!(body["models"][0]["value"], "default");
    assert_eq!(body["models"][0]["resolvedModel"], "claude-opus-5-5");
    let names: Vec<&str> = body["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, AGENTS);
    assert_eq!(body["account"]["tokenSource"], "claude.ai");
    assert_eq!(
        body["capabilities"],
        json!([
            "ui_surface_v1",
            "mcp_call_result_not_for_model_v1",
            "mcp_call_check_permissions_v1"
        ])
    );
    assert_eq!(body["output_style"], "default");
    assert_eq!(body["current_permission_mode"], "default");
    assert!(body["claude_code_version"].is_string() && body["pid"].is_number());
    assert!(
        f.silent_for(300),
        "nothing but the initialize response before a prompt"
    );
    assert!(f.finish().success(), "EOF after initialize only exits 0");
}

#[test]
fn fake_emits_init_on_first_user_line() {
    let dir = Dir::new("sysinit");
    let args = [
        "--session-id",
        UUID,
        "--permission-mode",
        "manual",
        "--tools",
        "Read,Edit",
    ];
    let mut f = Fake::start(
        &dir,
        Some(one_turn(json!({"outputTokens": 5, "costUsd": 0.01}))),
        &args,
        &[],
    );
    initialize(&mut f);
    f.user("hello");
    let init = f.read_any();
    assert_eq!(
        (init["type"].as_str(), init["subtype"].as_str()),
        (Some("system"), Some("init"))
    );
    assert_eq!(init["session_id"], UUID);
    assert_eq!(init["tools"], json!(["Edit", "Read"]), "the CLI's order");
    assert_eq!(init["permissionMode"], "default");
    assert_eq!(init["model"], "claude-opus-5-5");
    assert_eq!(init["agents"], json!(AGENTS));
    assert_eq!(init["mcp_servers"], json!([]));
    for k in ["plugins", "skills", "slash_commands", "capabilities"] {
        assert!(init[k].is_array(), "{k}");
    }
    assert!(init.get("memory_paths").is_none(), "absent, not null");
    assert_eq!(init["apiKeySource"], "none");
    assert!(init["messaging_socket_path"].is_string() || cfg!(not(unix)));
    let status = f.read_any();
    assert_eq!(
        (status["subtype"].as_str(), status["status"].as_str()),
        (Some("status"), Some("requesting"))
    );
    let result = f.read();
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], false);
    assert_eq!(result["session_id"], UUID);
    assert_eq!(result["queued_turn_count"], 0);
    assert_eq!(result["usage"]["output_tokens"], 5);
    assert!(f.finish().success());
}

#[test]
fn fake_init_honours_model_disallowed_tools_and_settings() {
    let dir = Dir::new("initflags");
    let settings = dir.write(
        "s.json",
        &json!({"outputStyle": "Concise", "disableAllHooks": true, "unknownKey": 1}),
    );
    let s = settings.to_str().unwrap();
    let mut f = Fake::start(
        &dir,
        Some(json!({"init": {"plugins": "$remove", "memory_paths": null}})),
        &[
            "--tools",
            "default",
            "--disallowedTools",
            "Cron*,Task,Bash(rm *)",
            "--model",
            "sonnet",
            "--settings",
            s,
            "--restricted",
        ],
        &[],
    );
    assert_eq!(initialize(&mut f)["output_style"], "Concise");
    f.user("x");
    let init = f.read();
    assert_eq!(init["model"], "claude-sonnet-5-5");
    assert_eq!(init["output_style"], "Concise");
    let tools: Vec<&str> = init["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    for gone in [
        "CronDelete",
        "CronList",
        "Task",
        "Bash",
        "WebFetch",
        "Monitor",
        "Workflow",
    ] {
        assert!(!tools.contains(&gone), "{gone} in {tools:?}");
    }
    assert!(tools.contains(&"Read") && tools.contains(&"ScheduleWakeup"));
    assert!(init.get("plugins").is_none(), "$remove deletes a key");
    assert!(init["memory_paths"].is_null() && init.get("memory_paths").is_some());
    assert!(
        init.get("messaging_socket_path").is_none(),
        "no inbox socket under --restricted"
    );
    assert!(f.finish().success());
    assert_eq!(observed(&dir)["settings"], "applied");

    // One wrongly typed known key: the whole file is ignored, outputStyle included.
    let dir = Dir::new("badsettings");
    let bad = dir.write("s.json", &json!({"outputStyle": "Concise", "permissions": {"blockWritesOutsideWorkingDirectories": "yes"}}));
    let mut f = Fake::start(&dir, None, &["--settings", bad.to_str().unwrap()], &[]);
    assert_eq!(initialize(&mut f)["output_style"], "default");
    assert!(f.finish().success());
    assert_eq!(observed(&dir)["settings"], "ignored");
}

#[test]
fn fake_refuses_unknown_flag() {
    let dir = Dir::new("flags");
    let out = Command::new(BIN)
        .args(BASE_ARGS)
        .args(["--bogus-flag", "x"])
        .env("FAKE_CLAUDE_OBSERVED", dir.path("observed.json"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error: unknown option '--bogus-flag'"));
    assert!(out.stdout.is_empty());
    assert!(
        observed(&dir)["refused"]
            .as_str()
            .unwrap()
            .contains("--bogus-flag")
    );
    for bad in [
        ["--permission-mode", "sometimes"],
        ["--session-id", "not-a-uuid"],
        ["--max-budget-usd", "-1"],
    ] {
        let st = Command::new(BIN)
            .args(BASE_ARGS)
            .args(bad)
            .output()
            .unwrap();
        assert_eq!(st.status.code(), Some(1), "{bad:?}");
    }
    // The documented launch shape is accepted, in both `--resume` forms and with `--restricted`.
    let ok = Fake::start(
        &dir,
        None,
        &[
            "--include-partial-messages",
            "--permission-mode",
            "manual",
            "--permission-prompts",
            "host",
            "--permission-prompt-tool",
            "stdio",
            "--setting-sources",
            "user",
            "--settings",
            "s.json",
            "--strict-mcp-config",
            "--mcp-config",
            "m.json",
            "--tools",
            "Read",
            "--disallowedTools",
            "Task,Agent",
            "--disable-slash-commands",
            "--append-system-prompt-file",
            "role.md",
            "--model",
            "default",
            "--max-budget-usd",
            "0",
            "--add-dir",
            "a",
            "--add-dir",
            "b",
            "--bare",
            "--restricted",
            &format!("--resume={UUID}"),
        ],
        &[],
    );
    let out = ok.child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "resume of an unknown session fails, not the flags"
    );
    let obs = observed(&dir);
    assert!(obs["refused"].is_null());
    assert_eq!(obs["flags"]["--resume"], UUID);
    assert_eq!(obs["flags"]["--permission-mode"], "manual");
    assert_eq!(obs["flags"]["--restricted"], true);
    assert_eq!(obs["flags"]["--bare"], true);
    assert_eq!(obs["flags"]["--add-dir"], json!(["a", "b"]));
    assert_eq!(obs["flags"]["--max-budget-usd"], "0");
}

#[test]
fn fake_refuses_a_flag_value_the_script_drops() {
    let dir = Dir::new("refusevalue");
    let script = json!({"refuseFlagValues": {"--permission-prompt-tool": ["stdio"]}});
    let mut cmd = command(
        &dir,
        Some(script),
        &["--permission-prompt-tool", "stdio"],
        &[],
    );
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--permission-prompt-tool"));
    assert!(
        observed(&dir)["refused"]
            .as_str()
            .unwrap()
            .contains("stdio")
    );
    let script = json!({"refuseFlagValues": {"--permission-prompt-tool": ["stdio"]}});
    let f = Fake::start(
        &dir,
        Some(script),
        &["--permission-prompt-tool", "mcp__finisher__approve"],
        &[],
    );
    assert!(f.finish().success(), "another value is still accepted");
}

#[test]
fn fake_can_use_tool_round_trip() {
    let dir = Dir::new("ask");
    let ask = |tool: &str| {
        json!({"expect": "user", "ask": {"tool": tool, "input": {"command": "ls"}},
            "onAllow": [{"type": "system", "subtype": "allowed", "session_id": "$session_id"}],
            "onDeny": [{"type": "system", "subtype": "denied"}]})
    };
    let script = json!({"steps": [ask("Bash"), ask("Write")]});
    let mut f = Fake::start(&dir, Some(script), &["--session-id", UUID], &[]);
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    let req = f.read();
    assert_eq!(req["type"], "control_request");
    assert_eq!(req["request"]["subtype"], "can_use_tool");
    assert_eq!(req["request"]["tool_name"], "Bash");
    assert_eq!(req["request"]["input"], json!({"command": "ls"}));
    assert!(req["request"]["tool_use_id"].is_string());
    // Keys out of serde's order: the record keeps the line as sent, byte for byte.
    let raw = format!(
        r#"{{"type":"control_response","response":{{"subtype":"success","request_id":{},"response":{{"updatedInput":{{"z":1,"command":"ls"}},"behavior":"allow"}}}}}}"#,
        req["request_id"]
    );
    f.send_raw(&raw);
    let allowed = f.read();
    assert_eq!(
        (allowed["subtype"].as_str(), allowed["session_id"].as_str()),
        (Some("allowed"), Some(UUID))
    );
    assert_eq!(
        f.read()["type"],
        "result",
        "a turn always ends with a result"
    );
    f.user("again");
    let req = f.read();
    assert_eq!(req["request"]["tool_name"], "Write");
    f.answer(&req, json!({"behavior": "deny", "message": "no"}));
    assert_eq!(f.read()["subtype"], "denied");
    assert!(f.finish().success());
    let obs = observed(&dir);
    let answers = &obs["answers"];
    assert_eq!(answers[0]["answer"]["behavior"], "allow");
    assert_eq!(answers[0]["raw"], raw);
    assert_eq!(answers[0]["via"], "stdio");
    assert_eq!(answers[1]["tool"], "Write");
    assert_eq!(
        answers[1]["answer"],
        json!({"behavior": "deny", "message": "no"})
    );
    assert_eq!(obs["controlResponses"].as_array().unwrap().len(), 2);
    assert_eq!(obs["controlResponses"][0]["raw"], raw);
}

#[test]
fn fake_interrupt_then_result() {
    let dir = Dir::new("interrupt");
    let script = json!({"onInterrupt": "exit", "steps": [
        {"expect": "user", "sleep": 60000, "emit": [{"type": "system", "subtype": "never"}]},
    ]});
    let mut f = Fake::start(&dir, Some(script), &["--session-id", UUID], &[]);
    f.user("work");
    assert_eq!(f.read()["subtype"], "init");
    f.control("req-9", "interrupt");
    let resp = f.read();
    assert_eq!(
        (
            resp["type"].as_str(),
            resp["response"]["request_id"].as_str()
        ),
        (Some("control_response"), Some("req-9"))
    );
    assert_eq!(resp["response"]["subtype"], "success");
    let result = f.read();
    assert_eq!(result["type"], "result");
    assert_eq!(result["subtype"], "error_during_execution");
    assert_eq!(result["session_id"], UUID);
    assert!(
        f.child.wait().unwrap().success(),
        "onInterrupt exit ends the process"
    );
    assert_eq!(
        f.drain(),
        Vec::<Value>::new(),
        "the skipped step emitted nothing"
    );
}

#[cfg(unix)]
#[test]
fn fake_sigint_ends_the_turn_and_keeps_running() {
    let dir = Dir::new("sigint");
    let mut f = Fake::start(
        &dir,
        Some(json!({"steps": [{"expect": "user", "sleep": 60000}]})),
        &["--session-id", UUID],
        &[],
    );
    f.user("work");
    assert_eq!(f.read()["subtype"], "init");
    assert!(
        Command::new("kill")
            .args(["-INT", &f.child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(f.read()["subtype"], "error_during_execution");
    f.user("next");
    assert_eq!(f.read()["type"], "result", "idle turns get a plain result");
    assert!(f.finish().success());
}

fn run_session(dir: &Dir, flag: &[&str], delta: Value) -> Value {
    let mut f = Fake::start(dir, Some(one_turn(delta)), flag, &[]);
    f.user("work");
    assert_eq!(f.read()["subtype"], "init");
    let result = f.read();
    let ok = f.finish().success();
    assert_eq!(
        ok,
        result["is_error"] == false,
        "EOF exits 1 after an is_error result"
    );
    result
}

fn transcripts(dir: &Dir) -> Vec<PathBuf> {
    let mut out = vec![];
    let root = dir.path("config").join("projects");
    for project in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        out.extend(
            std::fs::read_dir(project.path())
                .unwrap()
                .flatten()
                .map(|e| e.path()),
        );
    }
    out
}

#[test]
fn fake_resume_reads_its_transcript() {
    let dir = Dir::new("resume");
    run_session(&dir, &["--session-id", UUID], json!({"costUsd": 0.01}));
    let files = transcripts(&dir);
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with(format!("{UUID}.jsonl")));
    assert!(Path::new(&files[0]).metadata().unwrap().len() > 0);

    let resume = format!("--resume={UUID}");
    let mut f = Fake::start(&dir, Some(one_turn(json!({}))), &[&resume], &[]);
    f.user("more");
    let init = f.read();
    assert_eq!(init["session_id"], UUID, "a resume keeps the session id");
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    let all = observed_all(&dir);
    assert_eq!(all.len(), 2, "one record per process");
    assert!(all[0]["resume"].is_null());
    assert_eq!(all[1]["resume"]["found"], true);
    assert!(all[1]["resume"]["transcriptLines"].as_u64().unwrap() >= 2);

    let bad = Command::new(BIN)
        .args(BASE_ARGS)
        .arg(format!("--resume={OTHER_UUID}"))
        .env("CLAUDE_CONFIG_DIR", dir.path("config"))
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("No conversation found with session ID"));
    let line: Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(
        (
            line["type"].as_str(),
            line["subtype"].as_str(),
            line["is_error"].as_bool(),
            line["num_turns"].as_u64()
        ),
        (
            Some("result"),
            Some("error_during_execution"),
            Some(true),
            Some(0)
        )
    );
    assert_eq!(line["session_id"], OTHER_UUID);
}

#[test]
fn fake_reports_cumulative_usage_after_resume() {
    let dir = Dir::new("usage");
    let first = run_session(
        &dir,
        &["--session-id", UUID],
        json!({"inputTokens": 100, "outputTokens": 10, "costUsd": 0.01}),
    );
    assert_eq!(first["usage"]["output_tokens"], 10);
    let resume = format!("--resume={UUID}");
    let second = run_session(
        &dir,
        &[&resume],
        json!({"inputTokens": 50, "outputTokens": 20, "costUsd": 0.02}),
    );
    assert_eq!(second["usage"]["input_tokens"], 150);
    assert_eq!(second["usage"]["output_tokens"], 30);
    assert_eq!(second["modelUsage"]["claude-opus-5-5"]["outputTokens"], 30);
    assert!((second["total_cost_usd"].as_f64().unwrap() - 0.03).abs() < 1e-9);
    assert_eq!(second["num_turns"], 2);
    // The budget backstop counts the restored total.
    let third = run_session(
        &dir,
        &[&resume, "--max-budget-usd", "0.04"],
        json!({"costUsd": 0.02}),
    );
    assert_eq!(third["subtype"], "error_max_budget_usd");
    assert_eq!(third["is_error"], true);
}

#[test]
fn fake_resume_reset_counts_from_zero() {
    let dir = Dir::new("usage-reset");
    run_session(
        &dir,
        &["--session-id", UUID],
        json!({"inputTokens": 100, "outputTokens": 10, "costUsd": 0.01}),
    );
    let resume = format!("--resume={UUID}");
    let mut f = Fake::start(
        &dir,
        Some(
            json!({"resumeTotals": "reset", "steps": [{"expect": "user", "cumulativeUsage": {"inputTokens": 50, "outputTokens": 20, "costUsd": 0.02}}]}),
        ),
        &[&resume],
        &[],
    );
    f.user("work");
    assert_eq!(f.read()["subtype"], "init");
    let r = f.read();
    assert!(f.finish().success());
    assert_eq!(r["usage"]["input_tokens"], 50);
    assert_eq!(r["usage"]["output_tokens"], 20);
    assert!((r["total_cost_usd"].as_f64().unwrap() - 0.02).abs() < 1e-9);
}

#[test]
fn fake_live_and_hidden_usage_reach_the_totals() {
    let dir = Dir::new("usage-live");
    let script = json!({"steps": [{"expect": "user",
        "hiddenUsage": {"inputTokens": 7, "costUsd": 0.001},
        "liveUsage": {"inputTokens": 40, "outputTokens": 9, "cacheReadInputTokens": 3, "costUsd": 0.002},
        "usageModel": "claude-haiku-5-5"},
        {"cumulativeUsage": {"inputTokens": 1, "outputTokens": 1}}]});
    let mut f = Fake::start(&dir, Some(script), &["--session-id", UUID], &[]);
    f.user("work");
    assert_eq!(f.read()["subtype"], "init");
    let start = f.read();
    assert_eq!(start["type"], "stream_event");
    assert_eq!(start["event"]["type"], "message_start");
    assert_eq!(start["event"]["message"]["usage"]["input_tokens"], 40);
    assert_eq!(start["event"]["message"]["usage"]["output_tokens"], 1);
    assert_eq!(start["event"]["message"]["model"], "claude-haiku-5-5");
    let delta = f.read();
    assert_eq!(delta["event"]["type"], "message_delta");
    assert_eq!(delta["event"]["usage"]["output_tokens"], 9);
    assert_eq!(f.read()["event"]["type"], "message_stop");
    let r = f.read();
    assert!(f.finish().success());
    assert_eq!(r["usage"]["input_tokens"], 48);
    assert_eq!(r["usage"]["output_tokens"], 10);
    assert_eq!(r["modelUsage"]["claude-haiku-5-5"]["inputTokens"], 47);
    assert_eq!(r["modelUsage"]["claude-opus-5-5"]["inputTokens"], 1);
    assert!((r["total_cost_usd"].as_f64().unwrap() - 0.003).abs() < 1e-9);
}

#[test]
fn fake_records_env_names_only() {
    let dir = Dir::new("env");
    let secret = "s3cret-value-that-must-not-be-recorded";
    let mut f = Fake::start(
        &dir,
        None,
        &["--session-id", UUID],
        &[
            ("CLAUDE_CODE_OAUTH_TOKEN", secret),
            ("DISABLE_AUTOUPDATER", "1"),
        ],
    );
    f.user("x");
    f.read();
    f.read();
    assert!(f.finish().success());
    let raw = std::fs::read_to_string(dir.path("observed.json")).unwrap();
    assert!(!raw.contains(secret) && !raw.contains(&dir.0.to_string_lossy().to_string()));
    let obs = observed(&dir);
    let names: Vec<&str> = obs["envNames"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(names.contains(&"CLAUDE_CODE_OAUTH_TOKEN") && names.contains(&"DISABLE_AUTOUPDATER"));
    assert!(names.iter().all(|n| !n.contains('=')));
    assert_eq!(obs["flags"]["--session-id"], UUID);
}

#[test]
fn fake_reports_version_and_auth_status() {
    let run = |args: &[&str], script: Option<&Dir>| {
        let mut c = Command::new(BIN);
        c.args(args)
            .env("FAKE_CLAUDE_VERSION", "2.1.400")
            .env("CLAUDE_CONFIG_DIR", "/nonexistent/cfg");
        if let Some(d) = script {
            c.env("FAKE_CLAUDE_SCRIPT", d.path("script.json"));
        }
        c.output().unwrap()
    };
    assert_eq!(
        String::from_utf8_lossy(&run(&["--version"], None).stdout).trim(),
        "2.1.400 (Claude Code)"
    );
    let dir = Dir::new("auth");
    dir.write("script.json", &json!({"auth": {"loggedIn": false}}));
    for args in [&["auth", "status"][..], &["auth", "status", "--json"]] {
        let out = run(args, Some(&dir));
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["loggedIn"], false, "{args:?}");
        assert_eq!(v["authMethod"], "none");
        assert_eq!(v["apiProvider"], "firstParty");
        let cfg = Path::new("/nonexistent/cfg");
        assert_eq!(v["configDirectory"], cfg.to_string_lossy().as_ref());
        assert_eq!(
            v["projectsDirectory"],
            cfg.join("projects").to_string_lossy().as_ref()
        );
        assert!(v["loginExpiresAt"].is_null() && v["analyticsDisabled"] == true);
        assert_eq!(out.status.code(), Some(1));
    }
    let text = run(&["auth", "status", "--text"], Some(&dir));
    assert_eq!(
        String::from_utf8_lossy(&text.stdout).trim(),
        "Not logged in. Run claude auth login to authenticate."
    );
    assert_eq!(text.status.code(), Some(1));
    let signed = run(&["auth", "status"], None);
    assert_eq!(
        serde_json::from_slice::<Value>(&signed.stdout).unwrap()["loggedIn"],
        true
    );
    assert!(signed.status.success());
}

#[test]
fn fake_shipped_scripts_parse() {
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/claude");
    let mut n = 0;
    for entry in std::fs::read_dir(scripts).unwrap().flatten() {
        let out = Command::new(BIN)
            .arg("--version")
            .env("FAKE_CLAUDE_SCRIPT", entry.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?}: {}",
            entry.path(),
            String::from_utf8_lossy(&out.stderr)
        );
        n += 1;
    }
    assert!(n >= 5);
}
