//! Playing one turn: the user's message, then the script's steps, then `turn/completed`.
//!
//! The script turn played is the thread's own count of turns started (persisted with it), so a
//! script survives a crash: the turn after a resume in a new process is the next one.

use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::{Active, Fake};
use crate::codex::script::{Crash, Expect, Fail, Step, Turn, Usage};
use crate::codex::threads;

/// The capture's 401 texts (Task 0, `captured-0.167.0.jsonl`).
const UNAUTHORIZED: &str =
    "unexpected status 401 Unauthorized: Missing bearer or basic authentication in header";
const WSS: &str = "url: wss://api.openai.com/v1/responses, cf-ray: 0000000000000000-XXX";
const HTTPS: &str = "url: https://api.openai.com/v1/responses, cf-ray: 0000000000000000-XXX, request id: req_00000000000000000000000000000000";

/// How a turn ends early (`None` from a step: go on).
pub(super) enum End {
    Failed(Value),
    Interrupted,
}

fn subst(v: &Value, thread: &str, turn: &str) -> Value {
    match v {
        Value::String(s) if s == "$threadId" => json!(thread),
        Value::String(s) if s == "$turnId" => json!(turn),
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, thread, turn)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), subst(x, thread, turn)))
                .collect(),
        ),
        o => o.clone(),
    }
}

/// A `TokenUsageBreakdown` from a partial one: missing counts are 0, `totalTokens` is the sum
/// of input and output.
fn breakdown(m: &Map<String, Value>) -> Map<String, Value> {
    let n = |k: &str| m.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut out = Map::new();
    for k in [
        "inputTokens",
        "cachedInputTokens",
        "outputTokens",
        "reasoningOutputTokens",
    ] {
        out.insert(k.into(), json!(n(k)));
    }
    out.insert(
        "totalTokens".into(),
        json!(
            m.get("totalTokens")
                .and_then(Value::as_i64)
                .unwrap_or(n("inputTokens") + n("outputTokens"))
        ),
    );
    out
}

impl Fake {
    pub(super) fn interrupted(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.interrupted)
    }

    pub(super) fn ids(&self) -> (String, String) {
        let a = self.active.as_ref().expect("an active turn");
        (a.thread.clone(), a.turn.clone())
    }

    pub(super) fn emit_all(&mut self, list: &[Value]) {
        let (t, u) = self.ids();
        for e in list {
            let e = subst(e, &t, &u);
            let method = e["method"].as_str().unwrap_or("fake/unnamed").to_string();
            let mut line = json!({"method": method, "params": e["params"].clone()});
            line["emittedAtMs"] = e
                .get("emittedAtMs")
                .cloned()
                .unwrap_or(json!(threads::now_ms()));
            self.write(&line);
        }
    }

    /// An item's `item/started` and `item/completed`; the completed one is kept for the thread.
    pub(super) fn item_started(&mut self, item: &Value) {
        let (t, u) = self.ids();
        let now = threads::now_ms();
        self.notify(
            "item/started",
            json!({"item": item, "threadId": t, "turnId": u, "startedAtMs": now}),
        );
    }

    pub(super) fn item_completed(&mut self, item: Value) {
        let (t, u) = self.ids();
        self.notify(
            "item/completed",
            json!({"item": item, "threadId": t, "turnId": u, "completedAtMs": threads::now_ms()}),
        );
        self.active.as_mut().unwrap().items.push(item);
    }

    fn say(&mut self, text: &str) {
        let (t, u) = self.ids();
        let id = threads::new_id();
        let now = threads::now_ms();
        let mut item = json!({"type": "agentMessage", "id": id, "text": "", "phase": null, "memoryCitation": null,
            "startedAtMs": now, "completedAtMs": null});
        self.item_started(&item);
        self.notify(
            "item/agentMessage/delta",
            json!({"threadId": t, "turnId": u, "itemId": id, "delta": text}),
        );
        item["text"] = json!(text);
        item["completedAtMs"] = json!(threads::now_ms());
        self.item_completed(item);
    }

    /// The thread's record of `turn` as it stands, saved (a crash keeps what was said).
    fn save_turn(&mut self, thread: &str, view: Value) {
        if let Some(rec) = self.loaded.get_mut(thread) {
            let turns = rec["turns"].as_array_mut().expect("turns");
            match turns.iter_mut().find(|t| t["id"] == view["id"]) {
                Some(slot) => *slot = view,
                None => turns.push(view),
            }
            rec["updatedAt"] = json!(threads::now_ms() / 1000);
            let rec = rec.clone();
            threads::save(&self.home, &rec);
        }
    }

    pub(super) fn play_turn(&mut self, thread: &str, turn: &str, user: Value) {
        let idx = self.loaded.get_mut(thread).map_or(0, |rec| {
            let n = rec["scriptTurns"].as_u64().unwrap_or(0);
            rec["scriptTurns"] = json!(n + 1);
            n as usize
        });
        let scripted = self.script.turns.get(idx).cloned();
        let plain = scripted.is_none();
        let t: Turn = scripted.unwrap_or_default();
        let started_s = threads::now_ms() / 1000;
        self.failed.remove(thread);
        self.active = Some(Active {
            thread: thread.into(),
            turn: turn.into(),
            steer: t.steer.unwrap_or(self.script.steer),
            on_interrupt: t.on_interrupt.unwrap_or(self.script.on_interrupt),
            interrupted: false,
            interrupt_seen: false,
            steers_seen: 0,
            steers_used: 0,
            items: vec![],
        });
        self.notify(
            "thread/status/changed",
            json!({"threadId": thread, "status": {"type": "active", "activeFlags": []}}),
        );
        self.notify("turn/started", json!({"threadId": thread, "turn": threads::turn_view(turn, "inProgress", None, Value::Null, Some(started_s), 0)}));
        let mut started = user.clone();
        started["completedAtMs"] = Value::Null;
        self.item_started(&started);
        self.item_completed(user);
        let items = self.active.as_ref().expect("an active turn").items.clone();
        self.save_turn(
            thread,
            threads::turn_view(
                turn,
                "inProgress",
                Some(&items),
                Value::Null,
                Some(started_s),
                0,
            ),
        );
        let begun = Instant::now();
        let mut end = None;
        if plain {
            self.say("ok");
        }
        for step in &t.steps {
            end = self.run_step(step);
            if end.is_some() {
                break;
            }
        }
        let (status, error) = match end {
            None => ("completed", Value::Null),
            Some(End::Failed(e)) => ("failed", e),
            Some(End::Interrupted) => ("interrupted", Value::Null),
        };
        let a = self.active.take().expect("an active turn");
        let ms = begun.elapsed().as_millis() as i64;
        let view =
            |items| threads::turn_view(turn, status, items, error.clone(), Some(started_s), ms);
        self.save_turn(thread, view(Some(&a.items)));
        if status == "failed" {
            // The capture's order: the thread's error state, the final error, then the turn.
            self.failed.insert(thread.to_string());
            self.notify(
                "thread/status/changed",
                json!({"threadId": thread, "status": {"type": "systemError"}}),
            );
            self.notify(
                "error",
                json!({"error": error, "willRetry": false, "threadId": thread, "turnId": turn}),
            );
        } else {
            self.notify(
                "thread/status/changed",
                json!({"threadId": thread, "status": {"type": "idle"}}),
            );
        }
        self.notify(
            "turn/completed",
            json!({"threadId": thread, "turn": view(None)}),
        );
        if a.interrupted && a.on_interrupt == crate::codex::script::OnInterrupt::Exit {
            self.exit(0, "interrupted");
        }
        for (id, params) in std::mem::take(&mut self.deferred) {
            self.revert_now(&id, &params);
        }
    }

    fn run_step(&mut self, s: &Step) -> Option<End> {
        if let Some(e) = s.expect {
            while !self.expect_met(e) {
                if self.interrupted() {
                    return Some(End::Interrupted);
                }
                self.pump(None);
            }
        }
        if let Some(ms) = s.sleep {
            let end = Instant::now() + Duration::from_millis(ms);
            while let Some(left) = end.checked_duration_since(Instant::now()) {
                if self.interrupted() {
                    break;
                }
                self.pump(Some(left));
            }
        }
        if self.interrupted() {
            return Some(End::Interrupted);
        }
        self.emit_all(&s.emit);
        if let Some(text) = &s.say {
            self.say(text);
        }
        if let Some(u) = &s.usage {
            self.usage(u);
        }
        if let Some(n) = &s.unknown_notification {
            let (t, u) = self.ids();
            let method = n.as_str().unwrap_or("fake/unknownNotification");
            self.notify(method, json!({"threadId": t, "turnId": u}));
        }
        if let Some(a) = &s.account_change {
            self.account_change(a);
        }
        if let Some(d) = &s.dynamic_tool
            && let Some(end) = self.dynamic_tool(d)
        {
            return Some(end);
        }
        if let Some(p) = &s.patch
            && let Some(end) = self.patch(p)
        {
            return Some(end);
        }
        if s.not_logged_in {
            return Some(self.not_logged_in());
        }
        if let Some(f) = &s.fail {
            return Some(self.fail(f));
        }
        if let Some(c) = &s.crash {
            self.crash(c);
        }
        self.interrupted().then_some(End::Interrupted)
    }

    fn expect_met(&mut self, e: Expect) -> bool {
        let a = self.active.as_mut().unwrap();
        match e {
            Expect::Interrupt => a.interrupt_seen,
            Expect::Steer if a.steers_seen > a.steers_used => {
                a.steers_used += 1;
                true
            }
            Expect::Steer => false,
        }
    }

    fn usage(&mut self, u: &Usage) {
        let (t, turn) = self.ids();
        let last = breakdown(&u.last);
        let prev = self
            .loaded
            .get(&t)
            .map(|r| r["tokenTotal"].clone())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        let total = match &u.total {
            Some(m) => breakdown(m),
            None => {
                let mut sum = breakdown(&prev);
                for (k, v) in &last {
                    let s = sum[k].as_i64().unwrap_or(0) + v.as_i64().unwrap_or(0);
                    sum.insert(k.clone(), json!(s));
                }
                sum
            }
        };
        if let Some(rec) = self.loaded.get_mut(&t) {
            rec["tokenTotal"] = Value::Object(total.clone());
        }
        self.notify("thread/tokenUsage/updated", json!({"threadId": t, "turnId": turn, "tokenUsage": {
            "total": total, "last": last, "modelContextWindow": u.model_context_window.unwrap_or(272_000)}}));
    }

    fn account_change(&mut self, a: &Map<String, Value>) {
        let _ = std::fs::write(
            self.home.join("fake-account.json"),
            Value::Object(a.clone()).to_string(),
        );
        let api = a.get("type").and_then(Value::as_str) == Some("apiKey");
        self.notify(
            "account/updated",
            json!({"authMode": if api { "apikey" } else { "chatgpt" },
            "planType": a.get("planType").cloned().unwrap_or(Value::Null)}),
        );
    }

    /// `error {willRetry: true}` "Reconnecting... i/of" for each `i` of `range`.
    fn retry_errors(
        &mut self,
        range: std::ops::RangeInclusive<u32>,
        of: u32,
        info: &Value,
        details: &str,
    ) {
        let (t, u) = self.ids();
        for i in range {
            self.notify(
                "error",
                json!({"error": {"message": format!("Reconnecting... {i}/{of}"),
                "codexErrorInfo": info, "additionalDetails": details, "misalignment": null},
                "willRetry": true, "threadId": t, "turnId": u}),
            );
        }
    }

    /// The capture: retries 2..5 of the WebSocket, the fallback warning, retries 1..5 over
    /// HTTPS, then the 401 failure (the capture's ~14 s are not waited).
    fn not_logged_in(&mut self) -> End {
        let info = json!({"responseStreamDisconnected": {"httpStatusCode": 401}});
        let wss = format!("{UNAUTHORIZED}, {WSS}");
        let https = format!("{UNAUTHORIZED}, {HTTPS}");
        self.retry_errors(2..=5, 5, &info, &wss);
        let (t, _) = self.ids();
        self.notify(
            "warning",
            json!({"threadId": t, "message": format!("Falling back from WebSockets to HTTPS transport. {wss}")}),
        );
        self.retry_errors(1..=5, 5, &info, &https);
        End::Failed(
            json!({"message": https, "codexErrorInfo": {"httpConnectionFailed": {"httpStatusCode": 401}},
            "additionalDetails": null, "misalignment": null}),
        )
    }

    fn fail(&mut self, f: &Fail) -> End {
        let message = f
            .message
            .clone()
            .unwrap_or_else(|| "the turn failed".into());
        self.retry_errors(1..=f.retries, f.retries, &f.codex_error_info, &message);
        End::Failed(
            json!({"message": message, "codexErrorInfo": f.codex_error_info,
            "additionalDetails": f.additional_details, "misalignment": null}),
        )
    }

    fn crash(&mut self, c: &Crash) {
        if let Some(code) = c.code {
            self.exit(code, "crash");
        }
        self.obs
            .set("exit", json!({"signal": c.signal, "reason": "crash"}));
        #[cfg(unix)]
        {
            let sig = match c.signal.as_deref() {
                Some("TERM") => libc::SIGTERM,
                Some("SEGV") => libc::SIGSEGV,
                Some("INT") => libc::SIGINT,
                Some("HUP") => libc::SIGHUP,
                _ => libc::SIGKILL,
            };
            // SAFETY: restoring the default action and raising a signal on ourselves.
            unsafe {
                libc::signal(sig, libc::SIG_DFL);
                libc::raise(sig);
            }
        }
        std::process::exit(1);
    }
}
