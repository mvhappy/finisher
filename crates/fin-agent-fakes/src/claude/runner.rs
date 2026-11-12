//! The stream-JSON loop: handshake, control requests, and the script's steps.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod lines;

use super::argv::{self, Args, Command};
use super::observed::Observed;
use super::script::{self, Expect, Script, Step, Usage};
use super::transcript::{Totals, Transcript};

const POLL: Duration = Duration::from_millis(20);
const DEFAULT_VERSION: &str = "2.1.321";
const DEFAULT_TOOLS: &[&str] = &[
    "Bash",
    "Edit",
    "Glob",
    "Grep",
    "Read",
    "Write",
    "WebFetch",
    "WebSearch",
];

enum Ev {
    User(Value),
    Answer(Value),
    Control(String),
    Interrupted,
    Timeout,
    Eof,
}

enum Kind {
    Turn { text: String, delta: Usage },
    NotLoggedIn,
    Interrupted,
}

pub fn main(argv: Vec<String>) -> i32 {
    let script = match std::env::var("FAKE_CLAUDE_SCRIPT") {
        Ok(p) => match script::load(&p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        },
        Err(_) => Script::default(),
    };
    let version = std::env::var("FAKE_CLAUDE_VERSION")
        .ok()
        .or_else(|| script.version.clone())
        .unwrap_or_else(|| DEFAULT_VERSION.to_string());
    let observed_path = std::env::var("FAKE_CLAUDE_OBSERVED")
        .ok()
        .or_else(|| script.observed.clone());
    let mut observed = Observed::new(observed_path.map(Into::into));
    let args = match argv::parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            observed.start(&argv, json!({}));
            observed.set("refused", json!(e));
            eprintln!("{e}");
            return 1;
        }
    };
    observed.start(&argv, args.flags_json());
    match args.command {
        Command::Version => {
            println!("{version} (Claude Code)");
            0
        }
        Command::AuthStatus { json } => auth_status(&script, json),
        Command::Print => match Runner::new(args, script, version, observed) {
            Ok(r) => r.play(),
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
    }
}

fn auth_status(script: &Script, as_json: bool) -> i32 {
    let status = script.auth.clone().unwrap_or_else(
        || json!({"loggedIn": true, "authMethod": "claude.ai", "email": "user@example.invalid"}),
    );
    if as_json {
        println!("{status}");
    } else if status["loggedIn"] == true {
        println!(
            "Logged in (login method: {})",
            status["authMethod"].as_str().unwrap_or("unknown")
        );
    } else {
        println!("Not logged in");
    }
    if status["loggedIn"] == false { 1 } else { 0 }
}

struct Runner {
    rx: Receiver<Option<String>>,
    sigint: Arc<AtomicBool>,
    args: Args,
    script: Script,
    version: String,
    mode: String,
    session_id: String,
    transcript: Transcript,
    totals: Totals,
    observed: Observed,
    pending: VecDeque<Value>,
    init_sent: bool,
    interrupted: bool,
    exit_on_interrupt: bool,
    req_n: u64,
}

impl Runner {
    fn new(
        args: Args,
        script: Script,
        version: String,
        mut observed: Observed,
    ) -> Result<Runner, String> {
        let (session_id, transcript, totals) = match args.str("--resume") {
            Some(id) => match Transcript::find(id) {
                Some((t, restored)) => {
                    observed.set(
                        "resume",
                        json!({"found": true, "transcriptLines": restored.lines}),
                    );
                    (id.to_string(), t, restored.totals)
                }
                None => {
                    observed.set("resume", json!({"found": false}));
                    return Err(format!("No conversation found with session ID: {id}"));
                }
            },
            None => {
                let id = args
                    .str("--session-id")
                    .map_or_else(lines::fresh_uuid, str::to_string);
                (id.clone(), Transcript::create(&id), Totals::default())
            }
        };
        let mode = match args.str("--permission-mode") {
            None | Some("manual") => "default",
            Some(m) => m,
        }
        .to_string();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(l) => {
                        if tx.send(Some(l)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(None);
        });
        let sigint = Arc::new(AtomicBool::new(false));
        #[cfg(unix)]
        let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, sigint.clone());
        Ok(Runner {
            rx,
            sigint,
            args,
            script,
            version,
            mode,
            session_id,
            transcript,
            totals,
            observed,
            pending: VecDeque::new(),
            init_sent: false,
            interrupted: false,
            exit_on_interrupt: false,
            req_n: 0,
        })
    }

    fn emit(&self, v: &Value) {
        let mut out = std::io::stdout().lock();
        if writeln!(out, "{v}").and_then(|()| out.flush()).is_err() {
            std::process::exit(0);
        }
    }

    fn emit_script_line(&self, v: &Value) {
        match v {
            Value::String(raw) => {
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "{raw}").and_then(|()| out.flush());
            }
            other => self.emit(&self.substitute(other.clone())),
        }
    }

    fn substitute(&self, v: Value) -> Value {
        match v {
            Value::String(s) if s == "$session_id" => json!(self.session_id),
            Value::Array(a) => Value::Array(a.into_iter().map(|x| self.substitute(x)).collect()),
            Value::Object(o) => Value::Object(
                o.into_iter()
                    .map(|(k, x)| (k, self.substitute(x)))
                    .collect(),
            ),
            other => other,
        }
    }

    fn respond(&self, request_id: &Value, body: Value) {
        self.emit(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": body}}));
    }

    /// Serves one stdin line; returns what a step may care about.
    fn handle(&mut self, v: Value) -> Option<Ev> {
        match v["type"].as_str() {
            Some("control_request") => {
                let rid = v["request_id"].clone();
                let sub = v["request"]["subtype"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                match sub.as_str() {
                    "initialize" => self.respond(&rid, self.initialize_body()),
                    "set_permission_mode" => {
                        if let Some(m) = v["request"]["mode"].as_str() {
                            self.mode = if m == "manual" {
                                "default".into()
                            } else {
                                m.into()
                            };
                        }
                        self.respond(&rid, json!({}));
                    }
                    "interrupt" => {
                        self.respond(&rid, json!({}));
                        self.interrupt();
                        return Some(Ev::Interrupted);
                    }
                    _ => self.respond(&rid, json!({})),
                }
                Some(Ev::Control(sub))
            }
            Some("control_response") => Some(Ev::Answer(v)),
            Some("user") => {
                self.transcript.append(&v);
                if !self.init_sent {
                    self.init_sent = true;
                    self.emit(&self.init_line());
                }
                Some(Ev::User(v))
            }
            _ => None,
        }
    }

    fn interrupt(&mut self) {
        self.emit_result(Kind::Interrupted);
        self.interrupted = true;
        if self.exit_on_interrupt {
            std::process::exit(0);
        }
    }

    fn next(&mut self, timeout: Option<Duration>) -> Ev {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            if self.sigint.swap(false, std::sync::atomic::Ordering::SeqCst) {
                self.interrupt();
                return Ev::Interrupted;
            }
            let wait = match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        return Ev::Timeout;
                    }
                    (d - now).min(POLL)
                }
                None => POLL,
            };
            match self.rx.recv_timeout(wait) {
                Ok(Some(line)) => {
                    if let Some(ev) = serde_json::from_str(&line)
                        .ok()
                        .and_then(|v| self.handle(v))
                    {
                        return ev;
                    }
                }
                Ok(None) | Err(RecvTimeoutError::Disconnected) => return Ev::Eof,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn emit_result(&mut self, kind: Kind) {
        let (subtype, is_error, text, stop) = match &kind {
            Kind::Turn { text, delta } => {
                let t = &mut self.totals;
                t.input_tokens += delta.input_tokens;
                t.output_tokens += delta.output_tokens;
                t.cache_read_input_tokens += delta.cache_read_input_tokens;
                t.cache_creation_input_tokens += delta.cache_creation_input_tokens;
                t.cost_usd += delta.cost_usd;
                t.turns += 1;
                let over = self
                    .args
                    .str("--max-budget-usd")
                    .and_then(|b| b.parse::<f64>().ok())
                    .is_some_and(|b| t.cost_usd > b);
                if over {
                    ("error_max_budget_usd", true, text.clone(), Value::Null)
                } else {
                    ("success", false, text.clone(), json!("end_turn"))
                }
            }
            Kind::NotLoggedIn => (
                "success",
                true,
                "Not logged in · Please run /login".to_string(),
                Value::Null,
            ),
            Kind::Interrupted => ("error_during_execution", true, String::new(), Value::Null),
        };
        let t = self.totals;
        let model = self
            .script
            .model
            .clone()
            .unwrap_or_else(|| "claude-fake-1".into());
        self.emit(&json!({
            "type": "result", "subtype": subtype, "is_error": is_error,
            "duration_ms": 1, "duration_api_ms": 1, "num_turns": t.turns,
            "result": text, "stop_reason": stop,
            "session_id": self.session_id,
            "total_cost_usd": t.cost_usd,
            "usage": {
                "input_tokens": t.input_tokens, "output_tokens": t.output_tokens,
                "cache_read_input_tokens": t.cache_read_input_tokens,
                "cache_creation_input_tokens": t.cache_creation_input_tokens,
            },
            "modelUsage": {model: {
                "inputTokens": t.input_tokens, "outputTokens": t.output_tokens,
                "cacheReadInputTokens": t.cache_read_input_tokens,
                "cacheCreationInputTokens": t.cache_creation_input_tokens,
                "costUSD": t.cost_usd,
            }},
            "permission_denials": [],
            "queued_turn_count": self.pending.len(),
        }));
        self.transcript.append_totals(&self.totals);
    }

    fn default_turn(&mut self) {
        let delta = Usage {
            input_tokens: 1,
            output_tokens: 1,
            ..Usage::default()
        };
        self.emit_result(Kind::Turn {
            text: "ok".into(),
            delta,
        });
    }

    fn play(mut self) -> i32 {
        let steps = std::mem::take(&mut self.script.steps);
        for step in &steps {
            if step.expect == Some(Expect::User) {
                self.interrupted = false;
            } else if self.interrupted {
                continue;
            }
            if !self.step(step) {
                return 0;
            }
        }
        while let Some(_user) = self.pending.pop_front() {
            self.default_turn();
        }
        loop {
            match self.next(None) {
                Ev::User(_) => self.default_turn(),
                Ev::Eof => return 0,
                _ => {}
            }
        }
    }

    /// Runs one step; false when stdin ended.
    fn step(&mut self, s: &Step) -> bool {
        if s.exit_on.as_deref() == Some("interrupt") {
            self.exit_on_interrupt = true;
        }
        match s.expect {
            Some(Expect::User) => {
                if !self.wait_user() {
                    return false;
                }
                self.interrupted = false;
            }
            Some(Expect::Control) if !self.wait_control(s.subtype.as_deref()) => return false,
            Some(Expect::Control) => {}
            None => {}
        }
        if let Some(ms) = s.sleep
            && !self.pause(Duration::from_millis(ms))
        {
            return false;
        }
        if self.interrupted {
            return true;
        }
        self.side_effects(s);
        for line in &s.emit {
            self.emit_script_line(line);
        }
        if s.unknown_line {
            self.emit(&json!({"type": "fake_future_type", "session_id": self.session_id, "payload": {"note": "a line no CLI version has"}}));
        }
        if let Some(ask) = &s.ask {
            let answer = self.ask(&ask.tool, &ask.input);
            if self.interrupted {
                return answer.is_some();
            }
            match answer {
                None => return false,
                Some(true) => s.on_allow.iter().for_each(|l| self.emit_script_line(l)),
                Some(false) => s.on_deny.iter().for_each(|l| self.emit_script_line(l)),
            }
        }
        if s.not_logged_in {
            self.emit_result(Kind::NotLoggedIn);
        }
        if let Some(delta) = s.cumulative_usage {
            self.emit_result(Kind::Turn {
                text: s.text.clone().unwrap_or_else(|| "done".into()),
                delta,
            });
        }
        if let Some(c) = &s.crash {
            self.crash(c.signal.as_deref(), c.code);
        }
        true
    }

    fn side_effects(&mut self, s: &Step) {
        if let Some(w) = &s.write_file {
            let _ = std::fs::write(&w.path, &w.contents);
        }
        if let Some(sp) = &s.spawn {
            let code = sp.argv.split_first().and_then(|(prog, rest)| {
                std::process::Command::new(prog)
                    .args(rest)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .status()
                    .ok()
            });
            self.observed.push(
                "spawned",
                json!({"argv": sp.argv, "code": code.and_then(|c| c.code())}),
            );
        }
    }

    fn crash(&self, signal: Option<&str>, code: Option<i32>) {
        let _ = std::io::stdout().flush();
        #[cfg(unix)]
        if let Some(name) = signal {
            let sig = match name {
                "KILL" => libc::SIGKILL,
                "TERM" => libc::SIGTERM,
                "SEGV" => libc::SIGSEGV,
                "INT" => libc::SIGINT,
                "HUP" => libc::SIGHUP,
                _ => libc::SIGABRT,
            };
            // SAFETY: raise only delivers a signal to this thread's process.
            unsafe {
                libc::signal(sig, libc::SIG_DFL);
                libc::raise(sig);
            }
            std::process::exit(128 + sig);
        }
        std::process::exit(code.unwrap_or(1));
    }

    fn wait_user(&mut self) -> bool {
        if self.pending.pop_front().is_some() {
            return true;
        }
        loop {
            match self.next(None) {
                Ev::User(_) => return true,
                Ev::Eof => return false,
                _ => {}
            }
        }
    }

    fn wait_control(&mut self, subtype: Option<&str>) -> bool {
        loop {
            match self.next(None) {
                Ev::Control(s) if subtype.is_none_or(|w| w == s) => return true,
                Ev::Interrupted => return true,
                Ev::User(u) => self.pending.push_back(u),
                Ev::Eof => return false,
                _ => {}
            }
        }
    }

    /// Serves the host for `d`, queueing user lines; false on EOF.
    fn pause(&mut self, d: Duration) -> bool {
        let end = Instant::now() + d;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.next(Some(left)) {
                Ev::Timeout | Ev::Interrupted => return true,
                Ev::User(u) => self.pending.push_back(u),
                Ev::Eof => return false,
                _ => {}
            }
        }
    }

    /// Sends a `can_use_tool` request; `Some(allowed)`, or None on EOF.
    fn ask(&mut self, tool: &str, input: &Value) -> Option<bool> {
        self.req_n += 1;
        let rid = format!("fake-req-{}", self.req_n);
        self.emit(&json!({
            "type": "control_request", "request_id": rid,
            "request": {
                "subtype": "can_use_tool", "tool_name": tool, "input": input,
                "permission_suggestions": [], "blocked_path": null, "decision_reason": null,
                "tool_use_id": format!("toolu_fake_{}", self.req_n),
            },
        }));
        loop {
            match self.next(None) {
                Ev::Answer(v) if v["response"]["request_id"] == rid.as_str() => {
                    let resp = &v["response"];
                    let body = if resp["subtype"] == "success" {
                        resp["response"].clone()
                    } else {
                        json!({"error": resp["error"]})
                    };
                    let allow = body["behavior"] == "allow";
                    self.observed
                        .push("answers", json!({"tool": tool, "answer": body}));
                    return Some(allow);
                }
                Ev::Interrupted => return Some(false),
                Ev::User(u) => self.pending.push_back(u),
                Ev::Eof => return None,
                _ => {}
            }
        }
    }
}
