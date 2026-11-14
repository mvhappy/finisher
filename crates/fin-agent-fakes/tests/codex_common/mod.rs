//! Helpers for the `fake-codex` tests: a temp dir, a client that speaks JSON-RPC to the fake
//! and validates every line the fake wrote against the committed schema when it is dropped.
#![allow(dead_code)]

pub mod schema;

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

pub const BIN: &str = env!("CARGO_BIN_EXE_fake-codex");
pub const LINE_WAIT: Duration = Duration::from_secs(10);

/// Decision 4's argv, as `launch.ts` builds it (abridged to keys the fake knows).
pub const D4_ARGS: &[&str] = &[
    "app-server",
    "--listen",
    "stdio://",
    "--strict-config",
    "-c",
    "cli_auth_credentials_store=\"file\"",
    "-c",
    "project_doc_max_bytes=0",
    "-c",
    "project_doc_fallback_filenames=[]",
    "-c",
    "check_for_update_on_startup=false",
    "-c",
    "analytics.enabled=false",
    "-c",
    "history.persistence=\"none\"",
    "-c",
    "web_search=\"disabled\"",
    "-c",
    "shell_environment_policy.inherit=\"none\"",
    "-c",
    "shell_environment_policy.set={PATH=\"/usr/bin:/bin\"}",
    "-c",
    "skills.bundled.enabled=false",
    "-c",
    "mcp_servers={}",
    "--disable",
    "hooks",
    "--disable",
    "multi_agent",
    "--disable",
    "shell_tool",
    "--disable",
    "unified_exec",
    "--disable",
    "plugins",
];

pub struct Dir(pub PathBuf);

impl Dir {
    pub fn new(tag: &str) -> Dir {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("fake-codex-{tag}-{}-{n}", std::process::id()));
        for d in ["codex-home", "home", "work"] {
            std::fs::create_dir_all(p.join(d)).unwrap();
        }
        Dir(p)
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    pub fn home(&self) -> PathBuf {
        self.path("codex-home")
    }
    pub fn work(&self) -> PathBuf {
        self.path("work")
    }
    pub fn write(&self, name: &str, contents: &str) -> PathBuf {
        let p = self.path(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, contents).unwrap();
        p
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn command(dir: &Dir, script: Option<&Value>, args: &[&str], envs: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("CODEX_HOME", dir.home())
        .env("HOME", dir.path("home"))
        .env("FAKE_CODEX_OBSERVED", dir.path("observed.json"));
    if let Some(s) = script {
        cmd.env(
            "FAKE_CODEX_SCRIPT",
            dir.write("script.json", &s.to_string()),
        );
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.current_dir(dir.work());
    cmd
}

/// Runs the fake to completion with no stdin; returns (exit code, stdout, stderr).
pub fn run_to_end(dir: &Dir, args: &[&str]) -> (Option<i32>, String, String) {
    let o = command(dir, None, args, &[])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into(),
        String::from_utf8_lossy(&o.stderr).into(),
    )
}

pub struct Client {
    pub child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    inbox: VecDeque<Value>,
    methods: HashMap<i64, String>,
    next_id: i64,
    /// Every line the fake wrote, with the method of the request a response answers.
    pub log: Vec<(Value, Option<String>)>,
    /// Methods whose notifications are deliberately outside the schema.
    pub unvalidated: Vec<String>,
    pub validate_on_drop: bool,
}

impl Client {
    pub fn start(dir: &Dir, script: Option<Value>) -> Client {
        Client::start_with(dir, script, D4_ARGS, &[])
    }

    pub fn start_with(
        dir: &Dir,
        script: Option<Value>,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Client {
        Client::spawn(command(dir, script.as_ref(), args, envs))
    }

    pub fn spawn(mut cmd: Command) -> Client {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut tries = 0;
        let mut child = loop {
            match cmd.spawn() {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("spawn fake-codex: {e}"),
            }
        };
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Client {
            child,
            stdin,
            rx,
            inbox: VecDeque::new(),
            methods: HashMap::new(),
            next_id: 0,
            log: vec![],
            unvalidated: vec![],
            validate_on_drop: true,
        }
    }

    pub fn send_raw(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send_raw(&json!({"method": method, "params": params}).to_string());
    }

    /// Sends a request and returns its id without waiting.
    pub fn request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        self.methods.insert(id, method.to_string());
        self.send_raw(&json!({"id": id, "method": method, "params": params}).to_string());
        id
    }

    /// Answers a server request.
    pub fn answer(&mut self, id: &Value, result: Value) {
        self.send_raw(&json!({"id": id, "result": result}).to_string());
    }

    pub fn answer_error(&mut self, id: &Value, code: i64, message: &str) {
        self.send_raw(&json!({"id": id, "error": {"code": code, "message": message}}).to_string());
    }

    fn pull(&mut self) -> Option<Value> {
        self.pull_within(LINE_WAIT)
    }

    fn pull_within(&mut self, wait: Duration) -> Option<Value> {
        let l = self.rx.recv_timeout(wait).ok()?;
        let v: Value = serde_json::from_str(&l).unwrap_or_else(|_| panic!("not JSON: {l}"));
        let m = v
            .get("id")
            .and_then(Value::as_i64)
            .filter(|_| v.get("method").is_none())
            .and_then(|i| self.methods.get(&i).cloned());
        self.log.push((v.clone(), m));
        Some(v)
    }

    /// The next line the fake wrote (queued ones first).
    pub fn next(&mut self) -> Value {
        if let Some(v) = self.inbox.pop_front() {
            return v;
        }
        self.pull().expect("a line from fake-codex")
    }

    /// The next line, or `None` after `ms` of silence.
    pub fn next_within(&mut self, ms: u64) -> Option<Value> {
        if let Some(v) = self.inbox.pop_front() {
            return Some(v);
        }
        self.pull_within(Duration::from_millis(ms))
    }

    /// Sends a request and returns the full response line; other lines stay queued in order.
    pub fn call_raw(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        self.response_to(id)
    }

    pub fn response_to(&mut self, id: i64) -> Value {
        let mut held = vec![];
        let found = loop {
            let v = match self.inbox.pop_front() {
                Some(v) => v,
                None => self
                    .pull()
                    .unwrap_or_else(|| panic!("no response to request {id}")),
            };
            if v["id"] == id && v.get("method").is_none() {
                break v;
            }
            held.push(v);
        };
        for v in held.into_iter().rev() {
            self.inbox.push_front(v);
        }
        found
    }

    /// The `result` of a request that must succeed.
    pub fn call(&mut self, method: &str, params: Value) -> Value {
        let r = self.call_raw(method, params);
        assert!(r.get("error").is_none(), "{method} failed: {r}");
        r["result"].clone()
    }

    /// The `error` of a request that must fail.
    pub fn call_err(&mut self, method: &str, params: Value) -> Value {
        let r = self.call_raw(method, params);
        assert!(r.get("result").is_none(), "{method} should fail: {r}");
        r["error"].clone()
    }

    /// Reads lines until one has this `method`; returns it, dropping the ones before.
    pub fn until(&mut self, method: &str) -> Value {
        loop {
            let v = self.next();
            if v["method"] == method {
                return v;
            }
        }
    }

    /// Reads until `turn/completed`, returning every line up to and including it.
    pub fn turn_lines(&mut self) -> Vec<Value> {
        let mut out = vec![];
        loop {
            let v = self.next();
            let done = v["method"] == "turn/completed";
            out.push(v);
            if done {
                return out;
            }
        }
    }

    pub fn initialize(&mut self) -> Value {
        let r = self.call(
            "initialize",
            json!({
            "clientInfo": {"name": "finisher", "title": null, "version": "0.0.0"},
            "capabilities": {"experimentalApi": true}}),
        );
        self.notify("initialized", json!({}));
        r
    }

    /// `thread/start` with Finisher's params (+ `extra` merged over them); returns the result.
    pub fn start_thread(&mut self, dir: &Dir, extra: Value) -> Value {
        let mut p = json!({"cwd": dir.work(), "model": null, "sandbox": "read-only",
            "approvalPolicy": "on-request", "approvalsReviewer": "user", "ephemeral": false,
            "serviceName": "finisher", "developerInstructions": "be brief", "config": {}});
        if let (Some(m), Some(e)) = (p.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                m.insert(k.clone(), v.clone());
            }
        }
        self.call("thread/start", p)
    }

    pub fn shell_tool() -> Value {
        json!({"name": "shell", "description": "runs a command",
            "inputSchema": {"type": "object", "properties": {"command": {"type": "string"}}}})
    }

    /// Starts a turn; returns its id (the `turn/started` and the rest stay queued).
    pub fn start_turn(&mut self, thread: &str, text: &str) -> String {
        let r = self.call(
            "turn/start",
            json!({"threadId": thread,
            "input": [{"type": "text", "text": text, "text_elements": []}]}),
        );
        assert_eq!(r["turn"]["status"], "inProgress");
        r["turn"]["id"].as_str().unwrap().to_string()
    }

    pub fn close_stdin(&mut self) {
        self.stdin.take();
    }

    pub fn finish(mut self) -> ExitStatus {
        self.stdin.take();
        let st = self.child.wait().unwrap();
        while let Ok(l) = self.rx.recv_timeout(Duration::from_millis(200)) {
            let v: Value = serde_json::from_str(&l).unwrap();
            self.log.push((v, None));
        }
        st
    }

    /// Checks every line the fake wrote against the committed schema; returns how many.
    pub fn validate(&self) -> usize {
        let schemas = schema::Schemas::load();
        let mut n = 0;
        for (line, request) in &self.log {
            if let Some(m) = line["method"].as_str()
                && self.unvalidated.iter().any(|u| u == m)
            {
                continue;
            }
            if let Err(e) = schemas.check_line(line, request.as_deref()) {
                let shown: String = line.to_string().chars().take(500).collect();
                panic!("a line the fake wrote does not match the schema: {e}\n{shown}");
            }
            n += 1;
        }
        n
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.validate_on_drop && !std::thread::panicking() {
            self.validate();
        }
    }
}

/// Every process's record, in start order.
pub fn observed_all(dir: &Dir) -> Vec<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.path("observed.json")).unwrap()).unwrap()
}

pub fn observed(dir: &Dir) -> Value {
    observed_all(dir).pop().expect("a record")
}
