//! `fin agent spawn|wait|ls|send|stop` (plan 7B Task 6, Decisions 16–21; spec §6.3, §11.2): a
//! running session's children. Each child is its own session on a child branch of the parent's
//! current branch, with its own credential, a mode at most the parent's and a role the parent's
//! role may spawn; the parent composes by `fin merge <child branch>` into its own branch.
//!
//! - `fin agent spawn --role <r> --brief <text|@file> [--mode <m>] [--budget <tokens|usd>]
//!   [--model <m>]` → `{session, branch}` at once (the brief runs as the child's first run). A
//!   `@file` brief is read here, confined to the session's workspace and project folder under
//!   an agent credential. `--budget 50000` is tokens, `--budget 2usd` (or `$2`) money.
//! - `fin agent wait <id>… [--any] [--timeout <s>]` (default and most: 90 s, a longer one is
//!   capped) → `{children: [{child, status, reason?, branch?, summary?}]}` once every child (or,
//!   with `--any`, one) is done; `WAIT_TIMEOUT` (exit 5) when none is, so wait again.
//! - `fin agent ls` → the caller's children (a person's `fin`: every session of the project).
//! - `fin agent send <id> <text>`: a prompt when the child is idle, a steer while it runs.
//! - `fin agent stop <id>`: ends the child (its branch stays).
//!
//! All are `agent.session.*` RPCs: offline they are `SERVER_UNAVAILABLE`.
use std::path::Path;
use std::time::Duration;

use clap::{Args, Subcommand};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::backend::Backend;
use crate::client::Client;
use crate::endpoint;
use crate::output::{unavailable, usage};

/// `fin agent wait`'s default and longest wait, in seconds.
pub const AGENT_WAIT_MAX_S: u64 = 90;
/// What `fin agent wait` gives the server beyond the wait itself to answer.
const WAIT_SLACK: Duration = Duration::from_secs(15);
const MODES: &[&str] = &["plan", "draft", "auto"];
/// The longest brief read from a file, in bytes (the server takes 200,000 characters).
const BRIEF_FILE_MAX: u64 = 800_000;

#[derive(Debug, Clone, Subcommand)]
pub enum AgentCmd {
    /// Starts a child session on a child branch of this session's branch.
    Spawn(SpawnArgs),
    /// Waits at most 90 s for children to be done.
    Wait(WaitArgs),
    /// This session's children.
    Ls(LsArgs),
    /// Prompts an idle child, or steers a running one.
    Send(SendArgs),
    /// Ends a child; its branch stays.
    Stop(StopArgs),
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpawnArgs {
    /// The child's role (one the parent's role may spawn).
    #[arg(long, value_name = "r")]
    pub role: String,
    /// The brief: the text, or `@` and a file holding it.
    #[arg(long, value_name = "brief")]
    pub brief: String,
    /// plan, draft or auto (default: the role's, at most the parent's).
    #[arg(long, value_name = "m")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// The child's budget: tokens (`50000`) or money (`2usd`, `$2`).
    #[arg(long, value_name = "tokens|usd")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<String>,
    /// The child's model (default: the role's).
    #[arg(long, value_name = "m")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitArgs {
    /// The children to wait for (1 to 16).
    #[arg(required = true, value_name = "id")]
    pub ids: Vec<String>,
    /// Return once any one of them is done.
    #[arg(long)]
    #[serde(default)]
    pub any: bool,
    /// Seconds to wait (default and most: 90).
    #[arg(long, value_name = "s")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LsArgs {}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendArgs {
    /// The child's session id.
    pub id: String,
    /// What to tell it.
    pub text: String,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopArgs {
    /// The child's session id.
    pub id: String,
}

fn offline(operation: &str) -> CoreError {
    unavailable(json!({ "mode": "offline", "operation": operation }))
        .with_fix("agent sessions run on the server: start finisherd and run it again")
}

/// `--budget`: whole tokens, or money as `<x>usd` / `$<x>` (in millionths of a USD).
pub fn parse_budget(s: &str) -> Result<Value, CoreError> {
    let t = s.trim();
    let bad = || {
        usage(
            format!("--budget {s:?} is neither tokens nor money"),
            json!({ "budget": s }),
            "give tokens (`--budget 50000`) or money (`--budget 2usd`)",
        )
    };
    let money = t
        .strip_suffix("usd")
        .or_else(|| t.strip_suffix("USD"))
        .or_else(|| t.strip_prefix('$'));
    if let Some(m) = money {
        let v: f64 = m.trim().parse().map_err(|_| bad())?;
        if !v.is_finite() || v <= 0.0 || v > 1_000_000.0 {
            return Err(bad());
        }
        return Ok(json!({ "moneyMicros": (v * 1_000_000.0).round() as u64 }));
    }
    let n: u64 = t.parse().map_err(|_| bad())?;
    if n == 0 {
        return Err(bad());
    }
    Ok(json!({ "tokens": n }))
}

/// The brief's text: `@<file>` is read (confined under an agent credential), else as given.
fn brief_text(project: Option<&Path>, brief: &str) -> Result<String, CoreError> {
    let Some(file) = brief.strip_prefix('@') else {
        return Ok(brief.to_string());
    };
    crate::agent_paths::check(file, project)?;
    let path = crate::paths::expand_home(Path::new(file));
    let unreadable = |e: std::io::Error| {
        usage(
            format!("cannot read the brief file {file}: {e}"),
            json!({ "brief": file }),
            "give the brief as text, or @ and a readable file",
        )
    };
    let len = std::fs::metadata(&path).map_err(unreadable)?.len();
    if len > BRIEF_FILE_MAX {
        return Err(usage(
            format!("the brief file {file} is larger than {BRIEF_FILE_MAX} bytes"),
            json!({ "brief": file, "size": len, "max": BRIEF_FILE_MAX }),
            "write a shorter brief",
        ));
    }
    std::fs::read_to_string(&path).map_err(unreadable)
}

/// The `agent.session.start` payload of a spawn (without `projectId`, which the backend adds).
pub fn spawn_payload(a: &SpawnArgs, brief: String) -> Result<Value, CoreError> {
    let mut p = json!({ "role": a.role, "prompt": brief });
    if let Some(m) = &a.mode {
        if !MODES.contains(&m.as_str()) {
            return Err(usage(
                format!("--mode {m:?} is not a mode"),
                json!({ "mode": m, "modes": MODES }),
                format!("use one of {}", MODES.join(", ")),
            ));
        }
        p["mode"] = json!(m);
    }
    if let Some(b) = &a.budget {
        p["budget"] = parse_budget(b)?;
    }
    if let Some(m) = &a.model {
        p["model"] = json!(m);
    }
    Ok(p)
}

/// The `agent.session.wait` payload: the wait in ms, capped at 90 s.
pub fn wait_payload(a: &WaitArgs) -> Value {
    let secs = a.timeout.unwrap_or(AGENT_WAIT_MAX_S).min(AGENT_WAIT_MAX_S);
    let mut p = json!({ "ids": a.ids, "timeoutMs": secs * 1000 });
    if a.any {
        p["any"] = json!(true);
    }
    p
}

/// A session as `fin agent` shows it: what a parent needs, no budgets or usage.
fn brief_view(s: &Value) -> Value {
    let mut v =
        json!({ "session": s["id"], "role": s["role"], "mode": s["mode"], "status": s["status"] });
    for (from, to) in [
        ("statusReason", "reason"),
        ("rootBranch", "branch"),
        ("parent", "parent"),
        ("title", "title"),
    ] {
        if let Some(x) = s.get(from).filter(|x| !x.is_null()) {
            v[to] = x.clone();
        }
    }
    v
}

/// `project`: the global `--project` (an agent credential confines a `@file` brief by it).
pub fn run(
    backend: &dyn Backend,
    c: &AgentCmd,
    project: Option<&Path>,
) -> Result<Value, CoreError> {
    if backend.dry_run_only() {
        let op = match c {
            AgentCmd::Spawn(_) => "agent.session.start",
            AgentCmd::Wait(_) => "agent.session.wait",
            AgentCmd::Ls(_) => "agent.session.list",
            AgentCmd::Send(_) => "agent.session.steer",
            AgentCmd::Stop(_) => "agent.session.end",
        };
        return Err(offline(op));
    }
    match c {
        AgentCmd::Spawn(a) => {
            let payload = spawn_payload(a, brief_text(project, &a.brief)?)?;
            let started = backend.project_call("agent.session.start", payload)?;
            let s = &started["session"];
            let mut out = json!({ "session": s["id"] });
            if let Some(b) = s.get("rootBranch").filter(|b| !b.is_null()) {
                out["branch"] = b.clone();
            }
            Ok(out)
        }
        AgentCmd::Wait(a) => {
            let payload = wait_payload(a);
            let wait = Duration::from_millis(payload["timeoutMs"].as_u64().unwrap_or(0));
            // A long call with its own timeout: outside the overall deadline, as `fin wait` is.
            let mut client =
                Client::connect(&endpoint::resolve()?, wait + WAIT_SLACK)?.without_deadline();
            client.call("agent.session.wait", payload)
        }
        AgentCmd::Ls(_) => {
            let listed = backend.project_call("agent.session.list", json!({}))?;
            let children: Vec<Value> = listed["sessions"]
                .as_array()
                .map(|a| a.iter().map(brief_view).collect())
                .unwrap_or_default();
            Ok(json!({ "children": children }))
        }
        AgentCmd::Send(a) => backend.server_call(
            "agent.session.steer",
            json!({ "sessionId": a.id, "text": a.text }),
        ),
        AgentCmd::Stop(a) => {
            let ended = backend.server_call("agent.session.end", json!({ "sessionId": a.id }))?;
            Ok(brief_view(&ended["session"]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_tokens_or_money() {
        assert_eq!(parse_budget("50000").unwrap(), json!({ "tokens": 50000 }));
        assert_eq!(
            parse_budget("2usd").unwrap(),
            json!({ "moneyMicros": 2_000_000 })
        );
        assert_eq!(
            parse_budget("$0.25").unwrap(),
            json!({ "moneyMicros": 250_000 })
        );
        for bad in ["", "0", "-5", "ten", "1.5", "0usd", "$x"] {
            assert_eq!(parse_budget(bad).unwrap_err().code, "USAGE", "{bad}");
        }
    }

    #[test]
    fn wait_caps_at_90s() {
        let at = |timeout: Option<u64>| {
            wait_payload(&WaitArgs {
                ids: vec!["s1".into()],
                any: false,
                timeout,
            })["timeoutMs"]
                .as_u64()
                .unwrap()
        };
        assert_eq!(at(None), 90_000);
        assert_eq!(at(Some(5)), 5_000);
        assert_eq!(at(Some(600)), 90_000);
    }

    #[test]
    fn spawn_checks_its_mode_first() {
        let mut a = SpawnArgs {
            role: "fixer".into(),
            brief: "fix it".into(),
            mode: Some("wild".into()),
            budget: None,
            model: None,
        };
        assert_eq!(spawn_payload(&a, "x".into()).unwrap_err().code, "USAGE");
        a.mode = Some("draft".into());
        a.budget = Some("1000".into());
        assert_eq!(
            spawn_payload(&a, "fix it".into()).unwrap(),
            json!({ "role": "fixer", "prompt": "fix it", "mode": "draft", "budget": { "tokens": 1000 } })
        );
    }

    #[test]
    fn subagent_codes_keep_their_exit_classes() {
        for (code, exit) in [
            ("WAIT_TIMEOUT", 5),
            ("DEPTH_EXCEEDED", 7),
            ("ROLE_UNKNOWN", 3),
            ("MODE_INVALID", 3),
            ("SESSION_NOT_FOUND", 3),
            ("SCOPE_DENIED", 7),
        ] {
            assert_eq!(crate::output::exit_code(code), exit, "{code}");
        }
    }
}
