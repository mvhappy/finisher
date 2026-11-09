//! `fin approval wait|ls` (plan 7B Task 5, Decision 11; spec §7.3, §11.2): an agent session's
//! approval cards. A command a rule asks about answers `APPROVAL_PENDING` (exit 8, "wait, then
//! repeat") with `details.approval`; the agent waits on that card and repeats the request once
//! it is allowed (the grant is one-time and exact: the same command and arguments).
//!
//! - `fin approval wait <id> [--timeout <s>]` (default and most: 90 s, below every CLI tool
//!   timeout; a longer `--timeout` waits 90 s): `{approval}` and exit 0 when a person allowed
//!   it; `APPROVAL_DENIED` (exit 7: do not retry) when denied or expired; `APPROVAL_PENDING`
//!   (exit 8) when no one answered in time, so the agent runs it again.
//! - `fin approval ls [--status open|answered|expired] [--session <id>]`: newest first; under a
//!   session's credential only its own cards.
//!
//! Both are `agent.approval.*` RPCs: offline they are `SERVER_UNAVAILABLE`.
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

/// `fin approval wait`'s default and longest wait, in seconds.
pub const APPROVAL_WAIT_MAX_S: u64 = 90;
/// What `fin approval wait` gives the server beyond the wait itself to answer.
const WAIT_SLACK: Duration = Duration::from_secs(15);
const STATUSES: &[&str] = &["open", "answered", "expired"];

#[derive(Debug, Clone, Subcommand)]
pub enum ApprovalCmd {
    /// Waits at most 90 s for a person's answer to an approval.
    Wait(WaitArgs),
    /// This session's approvals, newest first.
    Ls(LsArgs),
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitArgs {
    /// The approval's id (`details.approval` of an `APPROVAL_PENDING` answer).
    pub id: String,
    /// Seconds to wait (default and most: 90).
    #[arg(long, value_name = "s")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LsArgs {
    /// Only approvals in this state: open, answered or expired.
    #[arg(long, value_name = "s")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Only this session's (a session's own credential lists only its own anyway).
    #[arg(long, value_name = "id")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

fn offline(operation: &str) -> CoreError {
    unavailable(json!({ "mode": "offline", "operation": operation }))
        .with_fix("approvals are kept by the server: start finisherd and run it again")
}

/// The `agent.approval.wait` payload: the wait in ms, capped at 90 s.
pub fn wait_payload(a: &WaitArgs) -> Value {
    let secs = a
        .timeout
        .unwrap_or(APPROVAL_WAIT_MAX_S)
        .min(APPROVAL_WAIT_MAX_S);
    json!({ "id": a.id, "timeoutMs": secs * 1000 })
}

fn ls_payload(a: &LsArgs) -> Result<Value, CoreError> {
    let mut p = json!({});
    if let Some(s) = &a.status {
        if !STATUSES.contains(&s.as_str()) {
            return Err(usage(
                format!("--status {s:?} is not an approval state"),
                json!({ "status": s, "states": STATUSES }),
                format!("use one of {}", STATUSES.join(", ")),
            ));
        }
        p["status"] = json!(s);
    }
    if let Some(s) = &a.session {
        p["sessionId"] = json!(s);
    }
    Ok(p)
}

pub fn run(backend: &dyn Backend, c: &ApprovalCmd) -> Result<Value, CoreError> {
    match c {
        ApprovalCmd::Ls(a) => {
            let payload = ls_payload(a)?;
            if backend.dry_run_only() {
                return Err(offline("agent.approval.list"));
            }
            backend.server_call("agent.approval.list", payload)
        }
        ApprovalCmd::Wait(a) => {
            if backend.dry_run_only() {
                return Err(offline("agent.approval.wait"));
            }
            let payload = wait_payload(a);
            let wait = Duration::from_millis(payload["timeoutMs"].as_u64().unwrap_or(0));
            // A long call with its own timeout: outside the overall deadline, as `fin wait` is.
            let mut client =
                Client::connect(&endpoint::resolve()?, wait + WAIT_SLACK)?.without_deadline();
            client.call("agent.approval.wait", payload)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_caps_at_90s() {
        let at = |timeout: Option<u64>| {
            wait_payload(&WaitArgs {
                id: "a1".into(),
                timeout,
            })["timeoutMs"]
                .as_u64()
                .unwrap()
        };
        assert_eq!(at(None), 90_000);
        assert_eq!(at(Some(5)), 5_000);
        assert_eq!(at(Some(90)), 90_000);
        assert_eq!(at(Some(600)), 90_000);
        assert_eq!(at(Some(0)), 0);
    }

    #[test]
    fn approval_codes_keep_their_exit_classes() {
        for (code, exit) in [
            ("APPROVAL_PENDING", 8),
            ("APPROVAL_DENIED", 7),
            ("APPROVAL_NOT_FOUND", 3),
            ("SCOPE_DENIED", 7),
        ] {
            assert_eq!(crate::output::exit_code(code), exit, "{code}");
        }
    }

    #[test]
    fn ls_checks_its_status_first() {
        let bad = LsArgs {
            status: Some("done".into()),
            session: None,
        };
        assert_eq!(ls_payload(&bad).unwrap_err().code, "USAGE");
        let good = LsArgs {
            status: Some("open".into()),
            session: Some("s1".into()),
        };
        assert_eq!(
            ls_payload(&good).unwrap(),
            json!({ "status": "open", "sessionId": "s1" })
        );
    }
}
