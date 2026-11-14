//! Playing one turn: the user's message, then the script's steps, then `turn/completed`.

use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::{Active, Fake};
use crate::codex::script::{Crash, DynamicTool, Expect, Fail, Patch, Step, Turn, Usage};
use crate::codex::threads;

/// How a turn ends early (`None` from a step: go on).
enum End {
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
    fn interrupted(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.interrupted)
    }

    fn ids(&self) -> (String, String) {
        let a = self.active.as_ref().expect("an active turn");
        (a.thread.clone(), a.turn.clone())
    }

    fn emit_all(&mut self, list: &[Value]) {
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
    fn item_started(&mut self, item: &Value) {
        let (t, u) = self.ids();
        let now = threads::now_ms();
        self.notify(
            "item/started",
            json!({"item": item, "threadId": t, "turnId": u, "startedAtMs": now}),
        );
    }

    fn item_completed(&mut self, item: Value) {
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

    pub(super) fn play_turn(&mut self, thread: &str, turn: &str, user: Value) {
        let idx = self.turn_cursor;
        self.turn_cursor += 1;
        let scripted = self.script.turns.get_mut(idx).map(std::mem::take);
        let plain = scripted.is_none();
        let t: Turn = scripted.unwrap_or_default();
        let started_s = threads::now_ms() / 1000;
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
        self.notify("turn/started", json!({"threadId": thread, "turn": threads::turn_view(turn, "inProgress", &[], Value::Null, started_s, 0)}));
        let mut started = user.clone();
        started["completedAtMs"] = Value::Null;
        self.item_started(&started);
        self.item_completed(user.clone());
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
        let mut items = a.items;
        items.insert(0, user);
        let done = threads::turn_view(
            turn,
            status,
            &items,
            error,
            started_s,
            begun.elapsed().as_millis() as i64,
        );
        if let Some(rec) = self.loaded.get_mut(thread) {
            rec["turns"]
                .as_array_mut()
                .expect("turns")
                .push(done.clone());
            rec["updatedAt"] = json!(threads::now_ms() / 1000);
            threads::save(&self.home, &rec.clone());
        }
        self.notify(
            "thread/status/changed",
            json!({"threadId": thread, "status": {"type": "idle"}}),
        );
        self.notify("turn/completed", json!({"threadId": thread, "turn": done}));
        if a.interrupted && a.on_interrupt == crate::codex::script::OnInterrupt::Exit {
            self.exit(0, "interrupted");
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
        if let Some(d) = &s.dynamic_tool {
            self.dynamic_tool(d)?;
        }
        if let Some(p) = &s.patch {
            self.patch(p)?;
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

    fn dynamic_tool(&mut self, d: &DynamicTool) -> Option<()> {
        let (t, u) = self.ids();
        // A tool the thread never declared (or lost on resume) is not one the model can call.
        if !self
            .tools
            .get(&t)
            .is_some_and(|names| names.contains(&d.tool))
        {
            self.obs.push("skippedTools", json!(d.tool));
            return Some(());
        }
        self.next_call += 1;
        let call = d
            .call_id
            .clone()
            .unwrap_or_else(|| format!("call_{}", self.next_call));
        let began = Instant::now();
        let mut item = json!({"type": "dynamicToolCall", "id": call, "tool": d.tool, "namespace": null,
            "arguments": d.arguments, "status": "inProgress", "contentItems": null, "success": null,
            "durationMs": null, "startedAtMs": threads::now_ms(), "completedAtMs": null});
        self.item_started(&item);
        let id = self.server_request(
            "item/tool/call",
            json!({"threadId": t, "turnId": u, "callId": call,
            "namespace": null, "tool": d.tool, "arguments": d.arguments}),
        );
        let answer = self.wait_response(&id, "item/tool/call")?;
        let result = answer.get("result").cloned().unwrap_or(Value::Null);
        let ok = result["success"] == true;
        item["status"] = json!(if ok { "completed" } else { "failed" });
        item["contentItems"] = result.get("contentItems").cloned().unwrap_or(json!([]));
        item["success"] = json!(ok);
        item["durationMs"] = json!(began.elapsed().as_millis() as i64);
        item["completedAtMs"] = json!(threads::now_ms());
        self.item_completed(item);
        self.emit_all(if ok { &d.on_success } else { &d.on_failure });
        Some(())
    }

    fn patch(&mut self, p: &Patch) -> Option<()> {
        let (t, u) = self.ids();
        self.next_call += 1;
        let item_id = p
            .item_id
            .clone()
            .unwrap_or_else(|| format!("fc_{}", self.next_call));
        let now = threads::now_ms();
        let mut item = json!({"type": "fileChange", "id": item_id, "status": "inProgress", "changes": p.changes,
            "startedAtMs": now, "completedAtMs": null});
        self.item_started(&item);
        let id = self.server_request(
            "item/fileChange/requestApproval",
            json!({"threadId": t, "turnId": u,
            "itemId": item_id, "startedAtMs": now, "reason": null, "grantRoot": null}),
        );
        let answer = self.wait_response(&id, "item/fileChange/requestApproval")?;
        self.notify(
            "serverRequest/resolved",
            json!({"threadId": t, "requestId": id}),
        );
        let decision = answer["result"]["decision"]
            .as_str()
            .unwrap_or("decline")
            .to_string();
        let accepted = decision == "accept" || decision == "acceptForSession";
        item["status"] = json!(if accepted { "completed" } else { "declined" });
        item["completedAtMs"] = json!(threads::now_ms());
        self.item_completed(item);
        self.emit_all(if accepted {
            &p.on_accept
        } else {
            &p.on_decline
        });
        // "cancel" also interrupts the turn.
        if decision == "cancel" {
            self.active.as_mut().unwrap().interrupted = true;
            return None;
        }
        Some(())
    }

    fn retry_errors(&mut self, n: u32, info: &Value, message: &str) {
        let (t, u) = self.ids();
        for i in 1..=n {
            self.notify(
                "error",
                json!({"error": {"message": format!("Reconnecting... {i}/{n}"),
                "codexErrorInfo": info, "additionalDetails": message, "misalignment": null},
                "willRetry": true, "threadId": t, "turnId": u}),
            );
        }
    }

    fn not_logged_in(&mut self) -> End {
        let msg =
            "unexpected status 401 Unauthorized: Missing bearer or basic authentication in header";
        self.retry_errors(
            4,
            &json!({"responseStreamDisconnected": {"httpStatusCode": 401}}),
            msg,
        );
        End::Failed(
            json!({"message": msg, "codexErrorInfo": {"httpConnectionFailed": {"httpStatusCode": 401}},
            "additionalDetails": null, "misalignment": null}),
        )
    }

    fn fail(&mut self, f: &Fail) -> End {
        let message = f
            .message
            .clone()
            .unwrap_or_else(|| "the turn failed".into());
        self.retry_errors(f.retries, &f.codex_error_info, &message);
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
