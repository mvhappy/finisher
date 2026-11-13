//! `result` lines (and the unsigned turn's synthetic `assistant`), `auth status`, and the
//! line an unknown `--resume` writes.

use serde_json::{Value, json};

use super::Runner;
use super::lines::{defaults, fresh_uuid, merge};
use crate::claude::script::{Script, Usage};
use crate::claude::transcript::ModelTotals;
use std::collections::BTreeMap;

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
    /// to the totals. Verified 2.1.321, Task 0 (s1a): `message_start.message.usage` holds the input
    /// side (output 1 at most); `message_delta.usage` repeats the input and cache fields and gives
    /// `output_tokens` as the message's running total.
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
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null},
            "usage": {"input_tokens": u.input_tokens,
                "cache_creation_input_tokens": u.cache_creation_input_tokens,
                "cache_read_input_tokens": u.cache_read_input_tokens,
                "output_tokens": u.output_tokens, "output_tokens_details": {"thinking_tokens": 0}}}),
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
                    // Verified 2.1.321, Task 0 (s1f): no `result` key, the cap in `errors`.
                    let cap = self.args.str("--max-budget-usd").unwrap_or_default();
                    over.insert("terminal_reason".into(), json!("budget_exhausted"));
                    over.insert(
                        "errors".into(),
                        json!([format!("Reached maximum budget (${cap})")]),
                    );
                    ("error_max_budget_usd", true, None, json!("end_turn"))
                } else {
                    over.insert("terminal_reason".into(), json!("completed"));
                    ("success", false, Some(text.clone()), json!("end_turn"))
                }
            }
            Kind::NotLoggedIn => {
                self.totals.turns += 1;
                over.insert("terminal_reason".into(), json!("api_error"));
                over.insert("api_error_status".into(), Value::Null);
                (
                    "success",
                    true,
                    Some(NOT_LOGGED_IN.to_string()),
                    json!("stop_sequence"),
                )
            }
            Kind::Interrupted { aborted_tools } => {
                // Verified on 2.1.321 (7D Task 0, s1b, s2b): an interrupt mid-tool ends the turn
                // so, with no `result` key and a diagnostic in `errors`. TODO(7D Task 0 Step 1):
                // an interrupt with no tool running; modelled the same with no `terminal_reason`.
                let stop = if *aborted_tools {
                    json!("tool_use")
                } else {
                    Value::Null
                };
                if *aborted_tools {
                    over.insert("terminal_reason".into(), json!("aborted_tools"));
                }
                over.insert(
                    "errors".into(),
                    json!([format!(
                        "[ede_diagnostic] result_type=user last_content_type=n/a stop_reason={}",
                        stop.as_str().unwrap_or("null")
                    )]),
                );
                ("error_during_execution", true, None, stop)
            }
        };
        let t = self.totals.clone();
        // Verified 2.1.321, Task 0 (s1a, s1b, s1d, s1e, s1f): `modelUsage` and `total_cost_usd`
        // cumulative per process (restored after a clean exit), `model_usage_since_last_result`
        // the turn's per model (`costBasis: "list"`) on every result, `usage` the turn's only
        // (all zeros on `error_max_budget_usd`), `result_index` from 0 per process.
        let plain = !matches!(kind, Kind::NotLoggedIn);
        let model_usage = per_model(&t.models, None, plain);
        let since = per_model(&t.models, Some(&t.at_last_result), plain);
        let mut turn = ModelTotals::default();
        if subtype != "error_max_budget_usd" {
            for (name, m) in &t.models {
                let l = t.at_last_result.get(name).copied().unwrap_or_default();
                turn.input_tokens += m.input_tokens - l.input_tokens;
                turn.output_tokens += m.output_tokens - l.output_tokens;
                turn.cache_read_input_tokens +=
                    m.cache_read_input_tokens - l.cache_read_input_tokens;
                turn.cache_creation_input_tokens +=
                    m.cache_creation_input_tokens - l.cache_creation_input_tokens;
            }
        }
        self.totals.at_last_result = t.models.clone();
        self.totals.results += 1;
        // The keys every real result has (Task 0), then a turn's that produced text.
        let mut line = defaults()["result"].clone();
        if let Some(text) = &text {
            merge(
                &mut line,
                defaults()["resultText"].as_object().expect("an object"),
            );
            over.insert("result".into(), json!(text));
        }
        merge(&mut line, json!({
            "type": "result", "subtype": subtype, "is_error": is_error,
            "num_turns": t.turns, "stop_reason": stop,
            "session_id": self.session_id,
            "total_cost_usd": t.cost_usd,
            "usage": usage(turn.input_tokens, turn.output_tokens, turn.cache_read_input_tokens, turn.cache_creation_input_tokens),
            "modelUsage": model_usage,
            "model_usage_since_last_result": since,
            "result_index": t.results,
            "permission_denials": std::mem::take(&mut self.denials),
            "uuid": fresh_uuid(),
            // Verified on 2.1.321 (7D Task 0, s1b/s1d): 0 even with a steer queued.
            "queued_turn_count": 0,
        }).as_object().expect("an object"));
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

/// `modelUsage` (cumulative) or, with `since`, `model_usage_since_last_result` (the models with
/// usage since then); empty when `plain` is false (the unsigned turn).
fn per_model(
    models: &BTreeMap<String, ModelTotals>,
    since: Option<&BTreeMap<String, ModelTotals>>,
    plain: bool,
) -> serde_json::Map<String, Value> {
    if !plain {
        return serde_json::Map::new();
    }
    let mut out = serde_json::Map::new();
    for (name, m) in models {
        let l = since.and_then(|s| s.get(name)).copied().unwrap_or_default();
        let d = ModelTotals {
            input_tokens: m.input_tokens - l.input_tokens,
            output_tokens: m.output_tokens - l.output_tokens,
            cache_read_input_tokens: m.cache_read_input_tokens - l.cache_read_input_tokens,
            cache_creation_input_tokens: m.cache_creation_input_tokens
                - l.cache_creation_input_tokens,
            cost_usd: m.cost_usd - l.cost_usd,
        };
        let moved = d.input_tokens
            + d.output_tokens
            + d.cache_read_input_tokens
            + d.cache_creation_input_tokens
            > 0
            || d.cost_usd > 0.0;
        if since.is_some() && !moved {
            continue;
        }
        // Verified 2.1.321, Task 0: both maps carry the model's metadata and `costBasis`.
        let mut v = defaults()["modelUsage"].clone();
        merge(
            &mut v,
            json!({
                "inputTokens": d.input_tokens, "outputTokens": d.output_tokens,
                "cacheReadInputTokens": d.cache_read_input_tokens,
                "cacheCreationInputTokens": d.cache_creation_input_tokens,
                "cacheCreationEphemeral1hInputTokens": d.cache_creation_input_tokens,
                "costUSD": d.cost_usd, "canonicalModel": name,
            })
            .as_object()
            .expect("an object"),
        );
        out.insert(name.clone(), v);
    }
    out
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
        // Verified on 2.1.321 (7D Task 0, 04-auth-status B/D): the label fields come from the
        // config dir's `.claude.json` `oauthAccount` (the sign-in home's, which `auth login`
        // writes); a fresh dir has none, so they stay `null`.
        if !signed_out {
            let profile = std::fs::read_to_string(c.join(".claude.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .map(|v| v["oauthAccount"].clone())
                .unwrap_or(Value::Null);
            for (field, key) in [
                ("email", "emailAddress"),
                ("orgId", "organizationUuid"),
                ("orgName", "organizationName"),
            ] {
                if let Some(v) = profile[key].as_str() {
                    o.insert(field.into(), json!(v));
                }
            }
        }
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
