//! `fake-claude` against its own binary: handshake, flags, control protocol, resume.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const UUID: &str = "0194f2a0-7c3e-4b1a-9d52-3f6a1c8e5b70";
const OTHER_UUID: &str = "0194f2a0-7c3e-4b1a-9d52-3f6a1c8e5b71";
const LINE_WAIT: Duration = Duration::from_secs(10);

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("fake-claude-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const BASE_ARGS: &[&str] = &[
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
];

struct Fake {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Fake {
    fn start(dir: &Dir, script: Option<Value>, extra: &[&str], envs: &[(&str, &str)]) -> Fake {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_fake-claude"));
        cmd.args(BASE_ARGS)
            .args(extra)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default());
        cmd.env("CLAUDE_CONFIG_DIR", dir.path("config"))
            .env("FAKE_CLAUDE_OBSERVED", dir.path("observed.json"));
        if let Some(s) = script {
            std::fs::write(dir.path("script.json"), s.to_string()).unwrap();
            cmd.env("FAKE_CLAUDE_SCRIPT", dir.path("script.json"));
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.current_dir(&dir.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Fake {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, v: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    }

    fn user(&mut self, text: &str) {
        self.send(json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}, "parent_tool_use_id": null, "session_id": UUID}));
    }

    fn control(&mut self, id: &str, subtype: &str) {
        self.send(
            json!({"type": "control_request", "request_id": id, "request": {"subtype": subtype}}),
        );
    }

    fn read(&self) -> Value {
        let l = self
            .lines
            .recv_timeout(LINE_WAIT)
            .expect("a line from fake-claude");
        serde_json::from_str(&l).unwrap_or_else(|_| panic!("not JSON: {l}"))
    }

    fn silent_for(&self, ms: u64) -> bool {
        self.lines.recv_timeout(Duration::from_millis(ms)).is_err()
    }

    fn finish(mut self) -> std::process::ExitStatus {
        self.stdin.take();
        self.child.wait().unwrap()
    }
}

fn observed(dir: &Dir) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.path("observed.json")).unwrap()).unwrap()
}

fn one_turn(delta: Value) -> Value {
    json!({"steps": [{"expect": "user", "cumulativeUsage": delta, "text": "hi"}]})
}

#[test]
fn fake_answers_initialize_before_prompt() {
    let dir = Dir::new("init");
    let mut f = Fake::start(&dir, None, &["--session-id", UUID], &[]);
    f.control("req-1", "initialize");
    let r = f.read();
    assert_eq!(r["type"], "control_response");
    assert_eq!(r["response"]["subtype"], "success");
    assert_eq!(r["response"]["request_id"], "req-1");
    let body = &r["response"]["response"];
    assert_eq!(body["commands"], json!([]));
    assert_eq!(body["models"][0]["value"], "default");
    assert!(
        body["models"][0]["resolvedModel"].is_string()
            && body["models"][0]["displayName"].is_string()
    );
    assert_eq!(body["account"]["tokenSource"], "claude.ai");
    assert!(
        body["claude_code_version"].is_string()
            && body["capabilities"].is_array()
            && body["pid"].is_number()
    );
    assert!(
        f.silent_for(300),
        "nothing but the initialize response before a prompt"
    );
    assert!(f.finish().success());
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
        "--model",
        "opus",
    ];
    let mut f = Fake::start(
        &dir,
        Some(one_turn(json!({"outputTokens": 5, "costUsd": 0.01}))),
        &args,
        &[],
    );
    f.control("req-1", "initialize");
    assert_eq!(f.read()["type"], "control_response");
    f.user("hello");
    let init = f.read();
    assert_eq!(
        (init["type"].as_str(), init["subtype"].as_str()),
        (Some("system"), Some("init"))
    );
    assert_eq!(init["session_id"], UUID);
    assert_eq!(init["tools"], json!(["Read", "Edit"]));
    assert_eq!(init["permissionMode"], "default");
    for k in [
        "mcp_servers",
        "plugins",
        "skills",
        "slash_commands",
        "capabilities",
    ] {
        assert!(init[k].is_array(), "{k}");
    }
    assert!(init["memory_paths"].is_null() && init["apiKeySource"].is_string());
    let result = f.read();
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], false);
    assert_eq!(result["session_id"], UUID);
    assert_eq!(result["queued_turn_count"], 0);
    assert_eq!(result["usage"]["output_tokens"], 5);
    assert!(f.finish().success());
}

#[test]
fn fake_refuses_unknown_flag() {
    let dir = Dir::new("flags");
    let out = Command::new(env!("CARGO_BIN_EXE_fake-claude"))
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
        ["--max-budget-usd", "0"],
    ] {
        let st = Command::new(env!("CARGO_BIN_EXE_fake-claude"))
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
            "1.5",
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
    let rid = req["request_id"].clone();
    f.send(
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": rid,
        "response": {"behavior": "allow", "updatedInput": {"command": "ls"}}}}),
    );
    let allowed = f.read();
    assert_eq!(
        (allowed["subtype"].as_str(), allowed["session_id"].as_str()),
        (Some("allowed"), Some(UUID))
    );
    f.user("again");
    let req = f.read();
    assert_eq!(req["request"]["tool_name"], "Write");
    f.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": req["request_id"],
        "response": {"behavior": "deny", "message": "no"}}}));
    assert_eq!(f.read()["subtype"], "denied");
    assert!(f.finish().success());
    let answers = observed(&dir)["answers"].clone();
    assert_eq!(answers[0]["answer"]["behavior"], "allow");
    assert_eq!(
        answers[1],
        json!({"tool": "Write", "answer": {"behavior": "deny", "message": "no"}})
    );
}

#[test]
fn fake_interrupt_then_result() {
    let dir = Dir::new("interrupt");
    let script = json!({"steps": [
        {"exitOn": "interrupt", "expect": "user", "sleep": 60000, "emit": [{"type": "system", "subtype": "never"}]},
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
        "exitOn interrupt ends the process"
    );
    assert!(
        f.lines.try_recv().is_err(),
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
    assert!(f.finish().success());
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
    let obs = observed(&dir);
    assert_eq!(obs["resume"]["found"], true);
    assert!(obs["resume"]["transcriptLines"].as_u64().unwrap() >= 2);

    let bad = Command::new(env!("CARGO_BIN_EXE_fake-claude"))
        .args(BASE_ARGS)
        .arg(format!("--resume={OTHER_UUID}"))
        .env("CLAUDE_CONFIG_DIR", dir.path("config"))
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("No conversation found with session ID"));
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
    assert_eq!(second["modelUsage"]["claude-fake-1"]["outputTokens"], 30);
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
    let obs: Value = serde_json::from_str(&raw).unwrap();
    let names: Vec<&str> = obs["envNames"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(names.contains(&"CLAUDE_CODE_OAUTH_TOKEN") && names.contains(&"DISABLE_AUTOUPDATER"));
    assert!(
        obs["envNames"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| !n.as_str().unwrap().contains('='))
    );
    assert_eq!(obs["flags"]["--session-id"], UUID);
}

#[test]
fn fake_reports_version_and_auth_status() {
    let run = |args: &[&str], script: Option<&Dir>| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_fake-claude"));
        c.args(args).env("FAKE_CLAUDE_VERSION", "2.1.400");
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
    std::fs::write(
        dir.path("script.json"),
        json!({"auth": {"loggedIn": false, "authMethod": "none"}}).to_string(),
    )
    .unwrap();
    let out = run(&["auth", "status", "--json"], Some(&dir));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["loggedIn"], false);
    assert_eq!(out.status.code(), Some(1));
    let signed = run(&["auth", "status", "--json"], None);
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
        let out = Command::new(env!("CARGO_BIN_EXE_fake-claude"))
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
