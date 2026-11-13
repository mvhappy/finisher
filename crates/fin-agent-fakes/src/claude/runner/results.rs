//! `result` lines (and the unsigned turn's synthetic `assistant`), `auth status`, and the
//! line an unknown `--resume` writes.

use serde_json::{Value, json};

use super::Runner;
use super::lines::{defaults, fresh_uuid, merge};
use crate::claude::script::{Script, Usage};

pub(super) enum Kind {
    Turn {
        text: String,
        delta: Usage,
        model: Option<String>,
    },
    NotLoggedIn,
    /// `aborted_tools`: a tool call was stopped (its error result already written).
    Interrupted {
        aborted_tools: bool,
    },
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
    /// Adds one call's usage to the process totals and to `model`'s (default: the session's).
    pub(super) fn add_usage(&mut self, delta: &Usage, model: Option<&str>) {
        let model = model.map_or_else(|| self.model(), str::to_string);
        let t = &mut self.totals;
        t.input_tokens += delta.input_tokens;
        t.output_tokens += delta.output_tokens;
        t.cache_read_input_tokens += delta.cache_read_input_tokens;
        t.cache_creation_input_tokens += delta.cache_creation_input_tokens;
        t.cost_usd += delta.cost_usd;
        let m = t.models.entry(model).or_default();
        m.input_tokens += delta.input_tokens;
        m.output_tokens += delta.output_tokens;
        m.cache_read_input_tokens += delta.cache_read_input_tokens;
        m.cache_creation_input_tokens += delta.cache_creation_input_tokens;
        m.cost_usd += delta.cost_usd;
    }

    /// One model call's usage as the stream reports it (`--include-partial-messages`), then added
    /// to the totals. TODO(7D Task 0 Step 1): the `stream_event` usage shape; modelled on the
    /// Messages API stream: `message_start.message.usage` holds the input side (output 1 at most),
    /// `message_delta.usage.output_tokens` the message's output so far.
    pub(super) fn live_usage(&mut self, u: &Usage, model: Option<&str>) {
        let name = model.map_or_else(|| self.model(), str::to_string);
        let id = fresh_uuid();
        let event = |e: Value| json!({"type": "stream_event", "uuid": fresh_uuid(), "session_id": self.session_id, "parent_tool_use_id": null, "event": e});
        self.emit(&event(json!({"type": "message_start", "message": {
            "id": id, "model": name, "type": "message", "role": "assistant", "content": [],
            "usage": {"input_tokens": u.input_tokens, "output_tokens": u.output_tokens.min(1),
                "cache_read_input_tokens": u.cache_read_input_tokens,
                "cache_creation_input_tokens": u.cache_creation_input_tokens},
        }})));
        self.emit(&event(
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": u.output_tokens}}),
        ));
        self.emit(&event(json!({"type": "message_stop"})));
        self.add_usage(u, Some(&name));
    }

    pub(super) fn emit_result(&mut self, kind: Kind) {
        let mut over = serde_json::Map::new();
        let (subtype, is_error, text, stop) = match &kind {
            Kind::Turn { text, delta, model } => {
                self.add_usage(delta, model.as_deref());
                let t = &mut self.totals;
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
            Kind::Interrupted { aborted_tools } => {
                // Verified 2.1.321, Task 0 (s1b): an interrupt mid-tool ends the turn so.
                if *aborted_tools {
                    over.insert("terminal_reason".into(), json!("aborted_tools"));
                    over.insert("result".into(), Value::Null);
                }
                let stop = if *aborted_tools {
                    json!("tool_use")
                } else {
                    Value::Null
                };
                ("error_during_execution", true, String::new(), stop)
            }
        };
        let t = self.totals.clone();
        // TODO(7D Task 0 Step 1): `modelUsage`'s shape (cumulative per model, like `usage`).
        let model_usage: serde_json::Map<String, Value> = if matches!(kind, Kind::NotLoggedIn) {
            serde_json::Map::new()
        } else {
            t.models
                .iter()
                .map(|(name, m)| {
                    (
                        name.clone(),
                        json!({
                            "inputTokens": m.input_tokens, "outputTokens": m.output_tokens,
                            "cacheReadInputTokens": m.cache_read_input_tokens,
                            "cacheCreationInputTokens": m.cache_creation_input_tokens,
                            "costUSD": m.cost_usd,
                        }),
                    )
                })
                .collect()
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
            // Verified on 2.1.321 (7D Task 0, s1b/s1d): 0 even with a steer queued.
            "queued_turn_count": 0,
        });
        merge(&mut line, &over);
        self.emit(&line);
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
    // Verified on 2.1.321 (7D Task 0): both answers name the config dir; signed in under a
    // fresh one, the label fields are `null` (`defaults.json`).
    let shape = if signed_out {
        "authSignedOut"
    } else {
        "authSignedIn"
    };
    let mut status = defaults()[shape].clone();
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".claude")));
    if let (Value::Object(o), Some(c)) = (&mut status, config) {
        o.insert(
            "projectsDirectory".into(),
            json!(c.join("projects").to_string_lossy()),
        );
        o.insert("configDirectory".into(), json!(c.to_string_lossy()));
    }
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
