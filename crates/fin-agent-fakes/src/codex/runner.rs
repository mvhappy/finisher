//! The JSON-RPC loop of `fake-codex`: one thread reads stdin, everything else (requests, the
//! script of a turn, the waits for the client's answers) runs on the main thread, which keeps
//! serving requests while a turn waits.

mod lists;
mod read;
mod requests;
mod thread_api;
mod turn;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::argv;
use super::config::{self, Effective};
use super::observed::Observed;
use super::script::{AccountRead, OnInterrupt, Script, Steer};
use super::threads;

/// The longest line read from stdin (the client's frame limit, Decision 3). A longer one is
/// dropped with a line on stderr and the server goes on.
/// UNOBSERVED(7E Task 0 Step 3): what 0.167.0 does with an over-long line.
const MAX_LINE: usize = 16 << 20;

/// The positive set of methods the fake answers (Decision 3's allow-list plus `initialize`).
const METHODS: &[&str] = &[
    "initialize",
    "thread/start",
    "thread/resume",
    "thread/revert",
    "thread/read",
    "turn/start",
    "turn/steer",
    "turn/interrupt",
    "model/list",
    "account/read",
    "config/read",
    "configRequirements/read",
    "skills/list",
    "hooks/list",
    "plugin/installed",
    "mcpServerStatus/list",
    "app/installed",
    "experimentalFeature/list",
];

struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

fn rpc(message: impl Into<String>) -> RpcError {
    RpcError {
        code: -32600,
        message: message.into(),
        data: None,
    }
}

type Reply = Result<Value, RpcError>;

/// A list the script plants, else `[]`.
fn planted(v: &Option<Value>) -> Value {
    v.clone().unwrap_or_else(|| json!([]))
}

/// Work to do after a response is written (the real server's notifications follow it).
enum Task {
    Notify(&'static str, Value),
    PlayTurn {
        thread: String,
        turn: String,
        user: Value,
    },
}

/// The turn being played.
struct Active {
    thread: String,
    turn: String,
    steer: Steer,
    on_interrupt: OnInterrupt,
    interrupted: bool,
    interrupt_seen: bool,
    steers_seen: usize,
    steers_used: usize,
    items: Vec<Value>,
}

struct Fake {
    rx: Receiver<Option<String>>,
    eff: Effective,
    script: Script,
    obs: Observed,
    home: PathBuf,
    version: String,
    initialized: bool,
    experimental: bool,
    responses: HashMap<String, Value>,
    /// Threads started or resumed by this process.
    loaded: HashMap<String, Value>,
    /// The dynamic tools the model of each loaded thread can call.
    tools: HashMap<String, Vec<String>>,
    active: Option<Active>,
    tasks: Vec<Task>,
    next_server_id: i64,
    next_call: u32,
    /// Server requests withdrawn by an interrupt (an answer to one is recorded as late).
    withdrawn: HashMap<String, String>,
    /// `thread/revert`s that wait for the turn they interrupted to end: (id, params).
    deferred: Vec<(Value, Value)>,
    /// Threads whose last turn failed (`systemError` until the next turn).
    failed: HashSet<String>,
}

/// Reads stdin line by line (at most `MAX_LINE` bytes each) into `tx`; `None` at EOF.
fn read_stdin(tx: std::sync::mpsc::Sender<Option<String>>) {
    let stdin = std::io::stdin();
    let mut r = stdin.lock();
    loop {
        let mut buf = Vec::new();
        match (&mut r)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut buf)
        {
            Ok(0) | Err(_) => break,
            Ok(_) if buf.len() > MAX_LINE && !buf.ends_with(b"\n") => {
                eprintln!("ERROR fake-codex: dropped a line longer than {MAX_LINE} bytes");
                // The rest of that line.
                loop {
                    let mut skip = Vec::new();
                    match (&mut r).take(1 << 20).read_until(b'\n', &mut skip) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if skip.ends_with(b"\n") => break,
                        Ok(_) => {}
                    }
                }
            }
            Ok(_) => {
                while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                    buf.pop();
                }
                if tx
                    .send(Some(String::from_utf8_lossy(&buf).into_owned()))
                    .is_err()
                {
                    return;
                }
            }
        }
    }
    let _ = tx.send(None);
}

pub fn main(args: Vec<String>) -> i32 {
    let argv = match argv::parse(&args) {
        Ok(a) => a,
        Err(u) => {
            eprintln!("{u}");
            return 2;
        }
    };
    let setup = match config::setup() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fake-codex: {e}");
            return 1;
        }
    };
    let script: Script = match &setup.script {
        None => Script::default(),
        Some(p) => match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fake-codex: script {}: {e}", p.display());
                return 1;
            }
        },
    };
    let version = setup
        .version
        .clone()
        .or(script.version.clone())
        .unwrap_or_else(|| config::PIN.to_string());
    if argv.version {
        println!("codex-cli {version}");
        return 0;
    }
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"));
    if let Err(e) = config::validate(&argv, &home) {
        eprintln!("Error: {e}");
        return 1;
    }
    if argv.listen != "stdio://" {
        eprintln!(
            "Error: fake-codex only listens on stdio://, not {}",
            argv.listen
        );
        return 1;
    }
    let observed = setup.observed.clone().or(script.observed.clone()).map(|o| {
        match o.strip_prefix("@codex/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(o),
        }
    });
    let mut obs = Observed::new(observed);
    let config: Map<String, Value> = argv
        .config
        .iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    obs.start(
        &args,
        json!({"strictConfig": argv.strict, "listen": argv.listen, "config": config,
        "enable": argv.enable, "disable": argv.disable, "version": version}),
    );
    let (tx, rx) = channel();
    std::thread::spawn(move || read_stdin(tx));
    let eff = Effective::new(&argv, &home);
    if eff.bundled_skills() {
        lists::install_bundled_skills(&home);
    }
    let mut fake = Fake {
        rx,
        eff,
        script,
        obs,
        home,
        version,
        initialized: false,
        experimental: false,
        responses: HashMap::new(),
        loaded: HashMap::new(),
        tools: HashMap::new(),
        active: None,
        tasks: vec![],
        next_server_id: 0,
        next_call: 0,
        withdrawn: HashMap::new(),
        deferred: vec![],
        failed: HashSet::new(),
    };
    loop {
        fake.pump(None);
    }
}

impl Fake {
    fn exit(&mut self, code: i32, reason: &str) -> ! {
        self.obs
            .set("exit", json!({"code": code, "reason": reason}));
        std::process::exit(code);
    }

    /// Takes one line from stdin (waiting at most `wait`) and handles it; `false` on timeout.
    fn pump(&mut self, wait: Option<Duration>) -> bool {
        let line = match wait {
            None => self.rx.recv().unwrap_or(None),
            Some(d) => match self.rx.recv_timeout(d) {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => return false,
                Err(RecvTimeoutError::Disconnected) => None,
            },
        };
        let Some(line) = line else {
            self.exit(0, "eof")
        };
        if line.trim().is_empty() {
            return true;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(v) if v.get("method").is_some() || v.get("id").is_some() => self.inbound(v),
            // As 0.167.0: logged, never answered.
            Ok(_) => Self::undecodable("not a JSON-RPC message"),
            Err(e) => Self::undecodable(&e.to_string()),
        }
        true
    }

    fn undecodable(why: &str) {
        eprintln!(
            "ERROR codex_app_server_transport::transport: Failed to deserialize JSONRPCMessage: {why}"
        );
    }

    fn write(&mut self, v: &Value) {
        let mut out = std::io::stdout().lock();
        if writeln!(out, "{v}").and_then(|()| out.flush()).is_err() {
            drop(out);
            self.exit(0, "stdout closed");
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write(&json!({"method": method, "params": params, "emittedAtMs": threads::now_ms()}));
    }

    /// Sends a server request; returns the id to wait on.
    fn server_request(&mut self, method: &str, params: Value) -> Value {
        let id = json!(self.next_server_id);
        self.next_server_id += 1;
        self.write(&json!({"id": id, "method": method, "params": params}));
        id
    }

    fn inbound(&mut self, v: Value) {
        let method = v.get("method").and_then(Value::as_str).map(str::to_string);
        let id = v.get("id").cloned();
        match (method, id) {
            (None, Some(id)) if self.withdrawn.contains_key(&id.to_string()) => {
                let method = self.withdrawn[&id.to_string()].clone();
                let mut rec = json!({"id": id, "method": method, "late": true});
                for k in ["result", "error"] {
                    if let Some(x) = v.get(k) {
                        rec[k] = x.clone();
                    }
                }
                self.obs.push("answers", rec);
            }
            (None, Some(id)) => {
                self.responses.insert(id.to_string(), v);
            }
            (Some(m), id) => {
                let mut rec =
                    json!({"method": m, "params": v.get("params").cloned().unwrap_or(Value::Null)});
                if let Some(i) = &id {
                    rec["id"] = i.clone();
                }
                self.obs.push("received", rec);
                if let Some(id) = id {
                    self.request(&m, id, v.get("params").cloned().unwrap_or(Value::Null));
                }
            }
            _ => Self::undecodable("not a JSON-RPC message"),
        }
    }

    fn request(&mut self, method: &str, id: Value, params: Value) {
        let res = if !METHODS.contains(&method) {
            let list: Vec<String> = METHODS.iter().map(|m| format!("`{m}`")).collect();
            Err(rpc(format!(
                "Invalid request: unknown variant `{method}`, expected one of {}",
                list.join(", ")
            )))
        } else if method == "initialize" {
            self.initialize(&params)
        } else if !self.initialized {
            Err(rpc("Not initialized"))
        } else if method == "account/read" && self.script.account_read.is_some() {
            // FAKE-ONLY (`Script::account_read`): no answer.
            if self.script.account_read == Some(AccountRead::Crash) {
                self.exit(1, "crash");
            }
            return;
        } else if method == "thread/revert" && self.revert_waits(&id, &params) {
            // Answered once the turn it interrupted has ended.
            return;
        } else {
            self.dispatch(method, &params)
        };
        self.answer(&id, res);
        for task in std::mem::take(&mut self.tasks) {
            match task {
                Task::Notify(m, p) => self.notify(m, p),
                Task::PlayTurn { thread, turn, user } => self.play_turn(&thread, &turn, user),
            }
        }
    }

    fn answer(&mut self, id: &Value, res: Reply) {
        match res {
            Ok(result) => self.write(&json!({"id": id, "result": result})),
            Err(e) => {
                let mut err = json!({"code": e.code, "message": e.message});
                if let Some(d) = e.data {
                    err["data"] = d;
                }
                self.write(&json!({"id": id, "error": err}));
            }
        }
    }

    fn initialize(&mut self, p: &Value) -> Reply {
        if self.initialized {
            return Err(rpc("Already initialized"));
        }
        let Some(name) = p["clientInfo"]["name"].as_str() else {
            return Err(rpc("Invalid request: missing field `clientInfo`"));
        };
        let (os, family) = match std::env::consts::OS {
            "linux" => ("Linux", "unix"),
            "macos" => ("Mac OS", "unix"),
            "windows" => ("Windows", "windows"),
            o => (o, "unix"),
        };
        let platform_os = std::env::consts::OS;
        let ua = format!(
            "{name}/{} ({os}; {}) unknown ({name}; {})",
            self.version,
            std::env::consts::ARCH,
            p["clientInfo"]["version"].as_str().unwrap_or("unknown")
        );
        self.initialized = true;
        self.experimental = p["capabilities"]["experimentalApi"] == true;
        self.obs.set("clientInfo", p["clientInfo"].clone());
        self.obs.set("capabilities", p["capabilities"].clone());
        self.tasks.push(Task::Notify(
            "remoteControl/status/changed",
            json!({"status": "disabled",
            "serverName": "host", "installationId": threads::new_id(), "environmentId": null}),
        ));
        Ok(
            json!({"userAgent": ua, "codexHome": self.home, "platformFamily": family,
            "platformOs": platform_os}),
        )
    }

    fn dispatch(&mut self, method: &str, p: &Value) -> Reply {
        match method {
            "config/read" => Ok(self.eff.read(p["includeLayers"] == true)),
            "configRequirements/read" => Ok(json!({"supportsIndependentSpeedModes": true,
                "requirements": self.script.requirements.clone().unwrap_or(Value::Null)})),
            "skills/list" => Ok(self.skills_list(p)),
            "hooks/list" => Ok(self.hooks_list(p)),
            "plugin/installed" => Ok(json!({"marketplaces": planted(&self.script.plugins),
                "marketplaceLoadErrors": []})),
            "mcpServerStatus/list" => Ok(match &self.script.mcp_servers {
                Some(v) => json!({"data": v, "nextCursor": null}),
                None => self.mcp_status(),
            }),
            "app/installed" => Ok(json!({"apps": planted(&self.script.apps)})),
            "experimentalFeature/list" => self.feature_list(p),
            "account/read" => Ok(self.account_read()),
            "model/list" => self.model_list(p),
            "thread/start" => self.thread_start(p),
            "thread/resume" => self.thread_resume(p),
            "thread/read" => self.thread_read(p),
            "thread/revert" => self.thread_revert(p),
            "turn/start" => self.turn_start(p),
            "turn/steer" => self.turn_steer(p),
            "turn/interrupt" => self.turn_interrupt(p),
            _ => Err(rpc(format!("unhandled {method}"))),
        }
    }
}
