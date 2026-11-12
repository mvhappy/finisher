//! Helpers for the `fake-claude` tests: a temp dir, a running fake, the observed file.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

pub const UUID: &str = "0194f2a0-7c3e-4b1a-9d52-3f6a1c8e5b70";
pub const OTHER_UUID: &str = "0194f2a0-7c3e-4b1a-9d52-3f6a1c8e5b71";
pub const LINE_WAIT: Duration = Duration::from_secs(10);
pub const BIN: &str = env!("CARGO_BIN_EXE_fake-claude");

pub const BASE_ARGS: &[&str] = &[
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
];

pub struct Dir(pub PathBuf);

impl Dir {
    pub fn new(tag: &str) -> Dir {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("fake-claude-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    pub fn write(&self, name: &str, v: &Value) -> PathBuf {
        let p = self.path(name);
        std::fs::write(&p, v.to_string()).unwrap();
        p
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Fake {
    pub child: Child,
    pub stdin: Option<ChildStdin>,
    pub lines: Receiver<String>,
}

/// The fake's command with a clean environment: `PATH`, a config dir and the observed file.
pub fn command(dir: &Dir, script: Option<Value>, extra: &[&str], envs: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(BASE_ARGS)
        .args(extra)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("CLAUDE_CONFIG_DIR", dir.path("config"))
        .env("FAKE_CLAUDE_OBSERVED", dir.path("observed.json"));
    if let Some(s) = script {
        cmd.env("FAKE_CLAUDE_SCRIPT", dir.write("script.json", &s));
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.current_dir(&dir.0);
    cmd
}

impl Fake {
    pub fn start(dir: &Dir, script: Option<Value>, extra: &[&str], envs: &[(&str, &str)]) -> Fake {
        Fake::spawn(command(dir, script, extra, envs))
    }

    pub fn spawn(mut cmd: Command) -> Fake {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // A freshly copied binary can be briefly busy (ETXTBSY): another test thread's fork
        // may still hold the copy's write descriptor until its exec.
        let mut tries = 0;
        let mut child = loop {
            match cmd.spawn() {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(26) && tries < 100 && cfg!(unix) => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("spawn fake-claude: {e}"),
            }
        };
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

    pub fn send_raw(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    pub fn send(&mut self, v: Value) {
        self.send_raw(&v.to_string());
    }

    pub fn user(&mut self, text: &str) {
        self.send(json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}, "parent_tool_use_id": null, "session_id": UUID}));
    }

    pub fn control(&mut self, id: &str, subtype: &str) {
        self.send(
            json!({"type": "control_request", "request_id": id, "request": {"subtype": subtype}}),
        );
    }

    /// Answers a control request of the fake with a success `response`.
    pub fn answer(&mut self, req: &Value, response: Value) {
        self.send(
            json!({"type": "control_response", "response": {"subtype": "success",
            "request_id": req["request_id"], "response": response}}),
        );
    }

    /// The next line, whatever it is.
    pub fn read_any(&self) -> Value {
        let l = self
            .lines
            .recv_timeout(LINE_WAIT)
            .expect("a line from fake-claude");
        serde_json::from_str(&l).unwrap_or_else(|_| panic!("not JSON: {l}"))
    }

    /// The next line that is not `system/status` or `system/ui_invalidate`.
    pub fn read(&self) -> Value {
        loop {
            let v = self.read_any();
            let noise = v["type"] == "system"
                && (v["subtype"] == "status" || v["subtype"] == "ui_invalidate");
            if !noise {
                return v;
            }
        }
    }

    pub fn silent_for(&self, ms: u64) -> bool {
        self.lines.recv_timeout(Duration::from_millis(ms)).is_err()
    }

    /// Every line left once the process exited and its stdout closed.
    pub fn drain(&self) -> Vec<Value> {
        let mut out = vec![];
        while let Ok(l) = self.lines.recv_timeout(LINE_WAIT) {
            out.push(serde_json::from_str(&l).unwrap_or(Value::String(l)));
        }
        out
    }

    pub fn finish(mut self) -> std::process::ExitStatus {
        self.stdin.take();
        self.child.wait().unwrap()
    }
}

/// Every process's record, in start order.
pub fn observed_all(dir: &Dir) -> Vec<Value> {
    let raw = std::fs::read_to_string(dir.path("observed.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// The last process's record.
pub fn observed(dir: &Dir) -> Value {
    observed_all(dir).pop().expect("a record")
}

/// The fake's `initialize` handshake; returns the response body.
pub fn initialize(f: &mut Fake) -> Value {
    f.control("req-init", "initialize");
    let r = f.read();
    assert_eq!(r["response"]["request_id"], "req-init", "{r}");
    r["response"]["response"].clone()
}
