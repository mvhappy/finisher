//! `result` lines (and the unsigned turn's synthetic `assistant`), `auth status`, and the
//! line an unknown `--resume` writes.

use serde_json::{Value, json};

use super::Runner;
use super::lines::{defaults, fresh_uuid, merge};
use crate::claude::script::{Script, Usage};

pub(super) enum Kind {
    Turn { text: String, delta: Usage },
    NotLoggedIn,
    Interrupted,
}

const NOT_LOGGED_IN: &str = "Not logged in · Please run /login";

fn usage(input: u64, output: u64, cache_read: u64, cache_create: u64) -> Value {
    let mut u = defaults()["usage"].clone();
    merge(
        &mut u,
        json!({"input_tokens": input, "output_tokens": output,
            "cache_read_input_tokens": cache_read, "cache_creation_input_tokens": cache_create})
        .as_object()
        .expect("an object"),
    );
    u
}

impl Runner {
    pub(super) fn emit_result(&mut self, kind: Kind) {
        let model = self.model();
        let mut over = serde_json::Map::new();
        let (subtype, is_error, text, stop) = match &kind {
            Kind::Turn { text, delta } => {
                let t = &mut self.totals;
                t.input_tokens += delta.input_tokens;
                t.output_tokens += delta.output_tokens;
                t.cache_read_input_tokens += delta.cache_read_input_tokens;
                t.cache_creation_input_tokens += delta.cache_creation_input_tokens;
                t.cost_usd += delta.cost_usd;
                t.turns += 1;
                let over_budget = self
                    .args
                    .str("--max-budget-usd")
                    .and_then(|b| b.parse::<f64>().ok())
                    .is_some_and(|b| t.cost_usd > b);
                if over_budget {
                    ("error_max_budget_usd", true, text.clone(), Value::Null)
                } else {
                    ("success", false, text.clone(), json!("end_turn"))
                }
            }
            Kind::NotLoggedIn => {
                self.totals.turns += 1;
                over.insert("terminal_reason".into(), json!("api_error"));
                over.insert("api_error_status".into(), Value::Null);
                (
                    "success",
                    true,
                    NOT_LOGGED_IN.to_string(),
                    json!("stop_sequence"),
                )
            }
            Kind::Interrupted => ("error_during_execution", true, String::new(), Value::Null),
        };
        let t = self.totals;
        let model_usage = if matches!(kind, Kind::NotLoggedIn) {
            json!({})
        } else {
            json!({model: {
                "inputTokens": t.input_tokens, "outputTokens": t.output_tokens,
                "cacheReadInputTokens": t.cache_read_input_tokens,
                "cacheCreationInputTokens": t.cache_creation_input_tokens,
                "costUSD": t.cost_usd,
            }})
        };
        let mut line = json!({
            "type": "result", "subtype": subtype, "is_error": is_error,
            "duration_ms": 1, "duration_api_ms": if is_error { 0 } else { 1 },
            "num_turns": t.turns, "result": text, "stop_reason": stop,
            "session_id": self.session_id,
            "total_cost_usd": t.cost_usd,
            "usage": usage(t.input_tokens, t.output_tokens, t.cache_read_input_tokens, t.cache_creation_input_tokens),
            "modelUsage": model_usage,
            "permission_denials": [],
            "uuid": fresh_uuid(),
            "queued_turn_count": self.pending.len(),
        });
        merge(&mut line, &over);
        self.emit(&line);
        self.transcript.append_totals(&self.totals);
        self.turn_open = false;
        self.last_is_error = is_error;
    }

    /// The unsigned turn (Task 0): a synthetic `assistant`, then a `success` result with
    /// `is_error: true`.
    pub(super) fn not_logged_in(&mut self) {
        self.emit(&json!({
            "type": "assistant",
            "message": {
                "id": fresh_uuid(), "container": null, "model": "<synthetic>", "role": "assistant",
                "stop_reason": "stop_sequence", "stop_sequence": "", "type": "message",
                "usage": usage(0, 0, 0, 0),
                "content": [{"type": "text", "text": NOT_LOGGED_IN}],
                "context_management": null,
            },
            "parent_tool_use_id": null, "session_id": self.session_id, "uuid": fresh_uuid(),
            "error": "authentication_failed", "is_api_error_message": true,
        }));
        self.emit_result(Kind::NotLoggedIn);
    }
}

/// `--resume` of a session the config dir lacks: this line on stdout, the message on stderr.
pub(super) fn resume_missing(id: &str) -> Value {
    json!({
        "type": "result", "subtype": "error_during_execution", "duration_ms": 0,
        "duration_api_ms": 0, "is_error": true, "num_turns": 0, "stop_reason": null,
        "session_id": id, "total_cost_usd": 0, "usage": usage(0, 0, 0, 0), "modelUsage": {},
        "permission_denials": [], "uuid": fresh_uuid(),
        "errors": [format!("No conversation found with session ID: {id}")],
    })
}

/// `auth status`: JSON by default, `--text` the alternative; exit 1 when signed out.
pub(super) fn auth_status(script: &Script, text: bool) -> i32 {
    let over = script.auth.clone().unwrap_or_default();
    let signed_out = over.get("loggedIn") == Some(&Value::Bool(false));
    let mut status = if signed_out {
        let mut s = defaults()["authSignedOut"].clone();
        let config = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".claude"))
            });
        if let (Value::Object(o), Some(c)) = (&mut s, config) {
            o.insert(
                "projectsDirectory".into(),
                json!(c.join("projects").to_string_lossy()),
            );
            o.insert("configDirectory".into(), json!(c.to_string_lossy()));
        }
        s
    } else {
        defaults()["authSignedIn"].clone()
    };
    merge(&mut status, &over);
    if !text {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_default()
        );
    } else if signed_out {
        println!("Not logged in. Run claude auth login to authenticate.");
    } else {
        println!(
            "Logged in (login method: {})",
            status["authMethod"].as_str().unwrap_or("unknown")
        );
    }
    i32::from(signed_out)
}
