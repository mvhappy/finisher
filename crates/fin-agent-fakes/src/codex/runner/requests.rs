//! The fake's own server requests (`item/tool/call`, `item/fileChange/requestApproval`): the
//! item they belong to, the wait for the client's answer, and the end of that wait.
//!
//! An answer is checked against the schema's response type; a malformed one is recorded under
//! `badAnswers` and counts as an error answer. An interrupt while one waits withdraws it
//! (`serverRequest/resolved`), completes its item (`failed` tool call, `declined` patch) and
//! ends the turn `interrupted`; an answer that comes later is recorded with `late: true`.
//! UNOBSERVED(7E Task 0 Step 3): all of it, as no unsigned turn reaches a server request: that
//! `serverRequest/resolved` follows an answered request and a withdrawn one, the status a
//! withdrawn item ends with, and how 0.167.0 reacts to a malformed answer.

use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value, json};

use super::Fake;
use super::turn::End;
use crate::codex::patch_fs;
use crate::codex::script::{DynamicTool, Patch};
use crate::codex::threads;

/// How the wait for an answer ended.
enum Answer {
    Result(Value),
    /// A JSON-RPC error, or a malformed result.
    Error,
    /// The turn was interrupted first.
    Withdrawn,
}

const DECISIONS: &[&str] = &["accept", "acceptForSession", "decline", "cancel"];

/// What is wrong with an answer to `item/tool/call` (`DynamicToolCallResponse`).
fn tool_answer_problem(r: &Value) -> Option<String> {
    let Some(items) = r.get("contentItems").and_then(Value::as_array) else {
        return Some("missing or non-array `contentItems`".into());
    };
    if !r.get("success").is_some_and(Value::is_boolean) {
        return Some("missing or non-boolean `success`".into());
    }
    for it in items {
        let ok = match it["type"].as_str() {
            Some("inputText") => it["text"].is_string(),
            Some("inputImage") => it["imageUrl"].is_string(),
            _ => false,
        };
        if !ok {
            return Some(format!("bad content item {it}"));
        }
    }
    None
}

fn approval_problem(r: &Value) -> Option<String> {
    match r.get("decision") {
        Some(Value::String(d)) if DECISIONS.contains(&d.as_str()) => None,
        d => Some(format!(
            "unknown decision {}",
            d.cloned().unwrap_or(Value::Null)
        )),
    }
}

impl Fake {
    /// Waits for the answer to server request `id`, serving requests meanwhile.
    fn await_answer(
        &mut self,
        id: &Value,
        method: &str,
        problem: fn(&Value) -> Option<String>,
    ) -> Answer {
        let key = id.to_string();
        loop {
            if let Some(v) = self.responses.remove(&key) {
                let mut rec = json!({"id": id, "method": method});
                for k in ["result", "error"] {
                    if let Some(x) = v.get(k) {
                        rec[k] = x.clone();
                    }
                }
                self.obs.push("answers", rec);
                self.resolved(id);
                let Some(result) = v.get("result") else {
                    return Answer::Error;
                };
                if let Some(why) = problem(result) {
                    self.obs.push(
                        "badAnswers",
                        json!({"id": id, "method": method, "reason": why, "raw": v}),
                    );
                    return Answer::Error;
                }
                return Answer::Result(result.clone());
            }
            if self.interrupted() {
                self.withdrawn.insert(key, method.to_string());
                self.resolved(id);
                return Answer::Withdrawn;
            }
            self.pump(None);
        }
    }

    fn resolved(&mut self, id: &Value) {
        let (t, _) = self.ids();
        self.notify(
            "serverRequest/resolved",
            json!({"threadId": t, "requestId": id}),
        );
    }

    pub(super) fn dynamic_tool(&mut self, d: &DynamicTool) -> Option<End> {
        let (t, u) = self.ids();
        // A tool the thread never declared (or lost on resume) is not one the model can call.
        if !self
            .tools
            .get(&t)
            .is_some_and(|names| names.contains(&d.tool))
        {
            self.obs.push("skippedTools", json!(d.tool));
            return None;
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
        let answer = self.await_answer(&id, "item/tool/call", tool_answer_problem);
        let (ok, content) = match &answer {
            Answer::Result(r) => (r["success"] == true, r["contentItems"].clone()),
            Answer::Error => (false, json!([])),
            Answer::Withdrawn => (false, Value::Null),
        };
        item["status"] = json!(if ok { "completed" } else { "failed" });
        item["contentItems"] = content;
        item["success"] = json!(ok);
        item["durationMs"] = json!(began.elapsed().as_millis() as i64);
        item["completedAtMs"] = json!(threads::now_ms());
        self.item_completed(item);
        if matches!(answer, Answer::Withdrawn) {
            return Some(End::Interrupted);
        }
        self.emit_all(if ok { &d.on_success } else { &d.on_failure });
        None
    }

    fn cwd_of(&self, thread: &str) -> PathBuf {
        self.loaded
            .get(thread)
            .and_then(|r| r["cwd"].as_str())
            .map(PathBuf::from)
            .unwrap_or_default()
    }

    pub(super) fn patch(&mut self, p: &Patch) -> Option<End> {
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
        let answer = self.await_answer(&id, "item/fileChange/requestApproval", approval_problem);
        let decision = match &answer {
            Answer::Result(r) => r["decision"].as_str().unwrap_or("decline").to_string(),
            _ => "decline".into(),
        };
        let accepted = decision == "accept" || decision == "acceptForSession";
        let status = if !accepted {
            "declined"
        } else {
            match patch_fs::apply(&self.cwd_of(&t), &p.changes) {
                Ok(()) => "completed",
                Err(e) => {
                    self.obs
                        .push("patchErrors", json!({"itemId": item_id, "error": e}));
                    "failed"
                }
            }
        };
        item["status"] = json!(status);
        item["completedAtMs"] = json!(threads::now_ms());
        self.item_completed(item);
        // A withdrawn request, or `cancel` ("the turn will also be immediately interrupted").
        if matches!(answer, Answer::Withdrawn) || decision == "cancel" {
            self.active.as_mut().expect("an active turn").interrupted = true;
            return Some(End::Interrupted);
        }
        self.emit_all(if accepted {
            &p.on_accept
        } else {
            &p.on_decline
        });
        None
    }
}
