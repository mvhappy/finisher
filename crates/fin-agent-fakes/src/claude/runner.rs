//! The stream-JSON loop: handshake, control requests, and the script's steps.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod ask;
mod lines;
mod mcp;
mod results;

use super::argv::{self, Args, Command};
use super::config;
use super::observed::Observed;
use super::script::{self, Branch, Expect, OnInterrupt, Script, Spawn, Step, Usage, WriteFile};
use super::settings::{self, Settings};
use super::transcript::{Totals, Transcript};
use ask::Outcome;
use results::Kind;

const POLL: Duration = Duration::from_millis(20);
const DEFAULT_VERSION: &str = "2.1.321";

enum Ev {
    User(Value),
    /// A `control_response` the host sent, and its raw line.
    Answer(Value, String),
    Control(String),
    /// An interrupt ended the open turn.
    Interrupted,
    Timeout,
    Eof,
}

pub fn main(argv: Vec<String>) -> i32 {
    let fail = |e: String| {
        eprintln!("{e}");
        1
    };
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let mut script = match cfg.script.as_deref().map(script::load) {
        Some(Ok(s)) => s,
        Some(Err(e)) => return fail(e),
        None => Script::default(),
    };
    if let Some(label) = cfg.account.as_deref() {
        with_account(&mut script, label);
    }
    let version = cfg
        .version
        .or_else(|| script.version.clone())
        .unwrap_or_else(|| DEFAULT_VERSION.to_string());
    let observed_path = cfg
        .observed
        .or_else(|| script.observed.clone())
        .and_then(|s| config::observed_path(&s));
    let mut observed = Observed::new(observed_path);
    let args = match argv::parse(&argv, &script.refuse_flag_values) {
        Ok(a) => a,
        Err(e) => {
            observed.start(&argv, json!({}));
            observed.set("refused", json!(e));
            return fail(e);
        }
    };
    observed.start(&argv, args.flags_json());
    match args.command {
        Command::Version => {
            println!("{version} (Claude Code)");
            0
        }
        Command::AuthStatus { text } => results::auth_status(&script, text),
        Command::Print => match Runner::new(args, script, version, observed) {
            Ok(r) => r.play(),
            Err(code) => code,
        },
    }
}

/// The configured account label, where a signed-in CLI reports one: `auth status` and
/// `initialize.account` (a script's own value wins; a signed-out script gets none).
// TODO(7D Task 0 Step 4): the field that holds the account label when signed in (and whether
// `initialize.account` carries it); modelled by fake-claude as `email` in both.
fn with_account(script: &mut Script, label: &str) {
    let auth = script.auth.get_or_insert_with(Default::default);
    if auth.get("loggedIn") != Some(&Value::Bool(false)) {
        auth.entry("email").or_insert_with(|| json!(label));
    }
    let mut account = lines::defaults()["initialize"]["account"].clone();
    match script.initialize.get("account") {
        Some(Value::Object(o)) => lines::merge(&mut account, o),
        // The script removed or replaced it: left as the script has it.
        Some(_) => return,
        None => {}
    }
    if let Value::Object(o) = &mut account
        && o.get("tokenSource").and_then(Value::as_str) != Some("none")
    {
        o.entry("email").or_insert_with(|| json!(label));
    }
    script.initialize.insert("account".into(), account);
}

struct Runner {
    rx: Receiver<Option<String>>,
    sigint: Arc<AtomicBool>,
    args: Args,
    script: Script,
    settings: Settings,
    mcp: mcp::Mcp,
    version: String,
    mode: String,
    session_id: String,
    transcript: Transcript,
    totals: Totals,
    observed: Observed,
    /// User lines received while a turn runs (steers), played in order.
    pending: VecDeque<Value>,
    started: bool,
    init_sent: bool,
    turn_open: bool,
    /// An interrupt ended the turn: its remaining steps are skipped.
    interrupted: bool,
    eof: bool,
    last_is_error: bool,
    req_n: u64,
}

impl Runner {
    fn new(
        args: Args,
        script: Script,
        version: String,
        mut observed: Observed,
    ) -> Result<Runner, i32> {
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
                    println!("{}", results::resume_missing(id));
                    eprintln!("No conversation found with session ID: {id}");
                    return Err(1);
                }
            },
            None => {
                let id = args
                    .str("--session-id")
                    .map_or_else(lines::fresh_uuid, str::to_string);
                (id.clone(), Transcript::create(&id), Totals::default())
            }
        };
        let settings = Settings::load(args.str("--settings"));
        observed.set("settings", json!(settings.state));
        let mcp = mcp::Mcp::new(settings::mcp_servers(args.str("--mcp-config")));
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
            settings,
            mcp,
            version,
            mode,
            session_id,
            transcript,
            totals,
            observed,
            pending: VecDeque::new(),
            started: false,
            init_sent: false,
            turn_open: false,
            interrupted: false,
            eof: false,
            last_is_error: false,
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
    fn handle(&mut self, v: Value, raw: String) -> Option<Ev> {
        match v["type"].as_str() {
            Some("control_request") => {
                let rid = v["request_id"].clone();
                let sub = v["request"]["subtype"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                self.observed.push(
                    "controlRequests",
                    json!({"requestId": rid, "subtype": sub, "request": v["request"]}),
                );
                match sub.as_str() {
                    "initialize" => {
                        // The SDK servers' handshake starts before the answer (Task 0).
                        self.mcp_start();
                        self.on_start();
                        let body = self.initialize_body();
                        self.emit(&json!({"type": "control_response", "response": {
                            "subtype": "success", "request_id": rid, "response": body,
                            "pending_permission_requests": [], "pending_user_dialog_requests": []}}));
                    }
                    "set_permission_mode" => {
                        if let Some(m) = v["request"]["mode"].as_str() {
                            self.mode = if m == "manual" { "default" } else { m }.into();
                        }
                        self.respond(&rid, json!({}));
                    }
                    "interrupt" => {
                        self.respond(&rid, json!({}));
                        return Some(self.interrupt(false).unwrap_or(Ev::Control(sub)));
                    }
                    _ => self.respond(&rid, json!({})),
                }
                Some(Ev::Control(sub))
            }
            Some("control_response") => {
                self.observed.push(
                    "controlResponses",
                    json!({"requestId": v["response"]["request_id"], "subtype": v["response"]["subtype"], "raw": raw}),
                );
                if self.mcp_answer(&v) {
                    None
                } else {
                    Some(Ev::Answer(v, raw))
                }
            }
            Some("user") => {
                // Every user line the host sent (a test proves none was sent).
                self.observed
                    .push("userLines", json!({"sessionId": v["session_id"]}));
                self.transcript.append(&v);
                Some(Ev::User(v))
            }
            _ => None,
        }
    }

    /// The first output of a session: `system/ui_invalidate` (Task 0: on every start).
    fn on_start(&mut self) {
        if !self.started {
            self.started = true;
            self.emit(&self.ui_invalidate());
        }
    }

    /// Applies the script's interrupt knob; `Some(Interrupted)` when it ended the open turn.
    fn interrupt(&mut self, sigint: bool) -> Option<Ev> {
        let knob = if sigint {
            self.script.on_sigint
        } else {
            self.script.on_interrupt
        };
        if knob == OnInterrupt::Ignore {
            return None;
        }
        let ended = self.turn_open;
        if ended {
            self.emit_result(Kind::Interrupted);
            self.interrupted = true;
        }
        if knob == OnInterrupt::Exit {
            let _ = std::io::stdout().flush();
            std::process::exit(if sigint { 130 } else { 0 });
        }
        ended.then_some(Ev::Interrupted)
    }

    /// The next event; with `timeout`, `Timeout` once it passes. After EOF a timed wait
    /// still sleeps out its time (the turn plays on); an untimed one returns `Eof`.
    fn next(&mut self, timeout: Option<Duration>) -> Ev {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            if self.sigint.swap(false, Ordering::SeqCst)
                && let Some(ev) = self.interrupt(true)
            {
                return ev;
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
            if self.eof {
                if deadline.is_none() {
                    return Ev::Eof;
                }
                std::thread::sleep(wait);
                continue;
            }
            match self.rx.recv_timeout(wait) {
                Ok(Some(line)) => {
                    if let Ok(v) = serde_json::from_str(&line)
                        && let Some(ev) = self.handle(v, line)
                    {
                        return ev;
                    }
                }
                Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                    self.eof = true;
                    return Ev::Eof;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// A turn begins: `system/init` before the first (after the SDK servers' handshake),
    /// then `system/status {status: "requesting"}`.
    fn start_turn(&mut self) {
        if !self.init_sent {
            self.on_start();
            self.mcp_wait();
            self.init_sent = true;
            self.emit(&self.init_line());
        }
        self.emit(&self.status_requesting());
        self.turn_open = true;
        self.interrupted = false;
    }

    /// A turn whose steps wrote no `result` gets a plain one.
    fn close_turn(&mut self) {
        if self.turn_open {
            self.emit_result(Kind::Turn {
                text: "done".into(),
                delta: Usage::default(),
            });
        }
    }

    fn default_turn(&mut self) {
        self.start_turn();
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

    /// The exit code once stdin is closed and the open turn has ended (Task 0): 1 after an
    /// `is_error` result, else 0.
    fn exit_code(&self) -> i32 {
        i32::from(self.last_is_error)
    }

    fn play(mut self) -> i32 {
        let steps = std::mem::take(&mut self.script.steps);
        for step in &steps {
            if step.expect == Some(Expect::User) {
                self.close_turn();
                if !self.wait_user() {
                    return self.exit_code();
                }
            } else if self.interrupted {
                continue;
            }
            self.step(step);
        }
        self.close_turn();
        loop {
            if self.pending.pop_front().is_some() {
                self.default_turn();
                continue;
            }
            match self.next(None) {
                Ev::User(u) => self.pending.push_back(u),
                Ev::Eof => return self.exit_code(),
                _ => {}
            }
        }
    }

    fn step(&mut self, s: &Step) {
        if s.expect == Some(Expect::Control) {
            self.wait_control(s.subtype.as_deref());
        }
        if let Some(ms) = s.sleep {
            self.pause(Duration::from_millis(ms));
        }
        if self.interrupted {
            return;
        }
        self.side_effects(s.write_file.as_ref(), s.spawn.as_ref());
        for line in &s.emit {
            self.emit_script_line(line);
        }
        if s.unknown_line {
            self.emit(&json!({"type": "fake_future_type", "session_id": self.session_id, "payload": {"note": "a line no CLI version has"}}));
        }
        if let Some(ask) = &s.ask {
            match self.ask(ask) {
                Outcome::Interrupted => return,
                Outcome::Allowed => self.branch(&s.on_allow),
                Outcome::Denied => self.branch(&s.on_deny),
                Outcome::Cancelled => {}
            }
            if self.interrupted {
                return;
            }
        }
        if s.not_logged_in {
            self.not_logged_in();
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
    }

    fn branch(&mut self, b: &Branch) {
        match b {
            Branch::Lines(lines) => lines.iter().for_each(|l| self.emit_script_line(l)),
            Branch::Then(t) => {
                self.side_effects(t.write_file.as_ref(), t.spawn.as_ref());
                t.emit.iter().for_each(|l| self.emit_script_line(l));
            }
        }
    }

    fn side_effects(&mut self, write: Option<&WriteFile>, spawn: Option<&Spawn>) {
        if let Some(w) = write {
            let _ = std::fs::write(&w.path, &w.contents);
        }
        if let Some(sp) = spawn {
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
        #[cfg(not(unix))]
        let _ = signal;
        std::process::exit(code.unwrap_or(1));
    }

    /// Takes the next user line (queued first) and starts its turn; false on EOF.
    fn wait_user(&mut self) -> bool {
        loop {
            if self.pending.pop_front().is_some() {
                self.start_turn();
                return true;
            }
            match self.next(None) {
                Ev::User(u) => self.pending.push_back(u),
                Ev::Eof => return false,
                _ => {}
            }
        }
    }

    /// Waits for a control request (of `subtype`); stdin's end ends the wait.
    fn wait_control(&mut self, subtype: Option<&str>) {
        loop {
            match self.next(None) {
                Ev::Control(s) if subtype.is_none_or(|w| w == s) => return,
                Ev::Interrupted | Ev::Eof => return,
                Ev::User(u) => self.pending.push_back(u),
                _ => {}
            }
        }
    }

    /// Serves the host for `d`, queueing user lines (steers); an interrupt ends it early.
    fn pause(&mut self, d: Duration) {
        let end = Instant::now() + d;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.next(Some(left)) {
                Ev::Timeout | Ev::Interrupted => return,
                Ev::User(u) => self.pending.push_back(u),
                _ => {}
            }
        }
    }
}
