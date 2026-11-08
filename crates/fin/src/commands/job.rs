//! `fin job ls|show|cancel|pause|resume` and `fin wait` (plan 5C-2 Task 2, spec §6.7): the
//! server's jobs, each command one `jobs.*` RPC, so offline every one is `SERVER_UNAVAILABLE`.
//! A principal sees and controls its own jobs (an agent session: its session's), every owner's
//! with `jobs.control`, and only jobs whose project is in its reach; any other id answers
//! `JOB_NOT_FOUND` (exit 3), exactly as an unknown one.
//!
//! - `fin job ls [--state <s>]… [--kind <k>] [--mine] [--limit <n>] [--cursor <c>]`: newest
//!   first, `{jobs, next?}`; `--mine` keeps a `jobs.control` principal to its own.
//! - `fin job show <id>`: the job's view (a failed job is a read: exit 0).
//! - `fin job cancel <id>`: the view once the run ended (`cancelled`, or what it ended as first).
//! - `fin job pause|resume (<id> | --all)`: one job (`{job}`), or dispatch and every job
//!   (`--all`, `jobs.control`).
//! - `fin wait <id>… [--any] [--timeout <s>]` (default 300 s, at most 600; on its own connection,
//!   outside `FIN_DEADLINE_S`): `{jobs}` and exit 0 when every job (with `--any`: every one that
//!   finished) succeeded; `JOB_FAILED` (exit 4) with `details.jobs` (each view) and
//!   `details.failed` (`{id, state, code?}`) otherwise; `WAIT_TIMEOUT` (exit 5) when they did not
//!   finish in time. Commands that start jobs (`fin look --no-wait`) print `{job}`: wait on its
//!   `id`.

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

/// The job states `--state` takes.
const STATES: &[&str] = &[
    "queued",
    "running",
    "paused",
    "succeeded",
    "failed",
    "cancelled",
    "interrupted",
];
/// `fin wait`'s default and longest timeout, in seconds.
pub const WAIT_DEFAULT_S: u64 = 300;
pub const WAIT_MAX_S: u64 = 600;
/// What `fin wait` gives the server beyond the wait itself to answer.
const WAIT_SLACK: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Subcommand)]
pub enum JobCmd {
    /// The jobs this principal sees, newest first.
    Ls(LsArgs),
    /// One job's view.
    Show(IdArgs),
    /// Cancels a job and answers its view once its run ended.
    Cancel(IdArgs),
    /// Pauses a job, or (--all) dispatch and every running resumable job.
    Pause(TargetArgs),
    /// Resumes a paused job, or (--all) every paused job and dispatch.
    Resume(TargetArgs),
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LsArgs {
    /// Only jobs in this state (repeatable): queued, running, paused, succeeded, failed,
    /// cancelled, interrupted.
    #[arg(long, value_name = "s")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state: Vec<String>,
    /// Only jobs of this kind.
    #[arg(long, value_name = "k")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Only this principal's own jobs (a `jobs.control` principal sees every owner's).
    #[arg(long)]
    #[serde(default)]
    pub mine: bool,
    /// At most this many (1 to 500, default 100).
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next` of the previous page.
    #[arg(long, value_name = "c")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    /// The job's id.
    pub id: String,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetArgs {
    /// The job's id (or --all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Every job and dispatch itself (needs jobs.control).
    #[arg(long)]
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitArgs {
    /// The jobs to wait for (1 to 100).
    #[arg(required = true, value_name = "id")]
    pub ids: Vec<String>,
    /// Return once any one of them finished.
    #[arg(long)]
    #[serde(default)]
    pub any: bool,
    /// Seconds to wait (default 300, at most 600).
    #[arg(long, value_name = "s")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

fn offline(operation: &str) -> CoreError {
    unavailable(json!({ "mode": "offline", "operation": operation }))
        .with_fix("jobs run on the server: start finisherd and run it again")
}

fn target(a: &TargetArgs, verb: &str) -> Result<Value, CoreError> {
    match (&a.id, a.all) {
        (Some(id), false) => Ok(json!({ "job": id })),
        (None, true) => Ok(json!({ "all": true })),
        _ => Err(usage(
            format!("fin job {verb} takes a job id or --all, not both"),
            json!({ "id": a.id, "all": a.all }),
            format!("run fin job {verb} <id>, or fin job {verb} --all"),
        )),
    }
}

fn ls_payload(a: &LsArgs) -> Result<Value, CoreError> {
    if let Some(bad) = a.state.iter().find(|s| !STATES.contains(&s.as_str())) {
        return Err(usage(
            format!("--state {bad:?} is not a job state"),
            json!({ "state": bad, "states": STATES }),
            format!("use one of {}", STATES.join(", ")),
        ));
    }
    let mut p = json!({});
    if !a.state.is_empty() {
        p["state"] = json!(a.state);
    }
    if let Some(k) = &a.kind {
        p["kind"] = json!(k);
    }
    if a.mine {
        p["mine"] = json!(true);
    }
    if let Some(n) = a.limit {
        p["limit"] = json!(n);
    }
    if let Some(c) = &a.cursor {
        p["cursor"] = json!(c);
    }
    Ok(p)
}

pub fn run(backend: &dyn Backend, cmd: &JobCmd) -> Result<Value, CoreError> {
    let (tag, payload) = match cmd {
        JobCmd::Ls(a) => ("jobs.list", ls_payload(a)?),
        JobCmd::Show(a) => ("jobs.get", json!({ "job": a.id })),
        JobCmd::Cancel(a) => ("jobs.cancel", json!({ "job": a.id })),
        JobCmd::Pause(a) => ("jobs.pause", target(a, "pause")?),
        JobCmd::Resume(a) => ("jobs.resume", target(a, "resume")?),
    };
    if backend.dry_run_only() {
        return Err(offline(tag));
    }
    backend.server_call(tag, payload)
}

/// The jobs of a `jobs.wait` answer that did not succeed (with `any`: of those that finished).
fn failed(jobs: &[Value], any: bool) -> Vec<Value> {
    jobs.iter()
        .filter(|j| !(any && j.get("finishedAt").is_none_or(Value::is_null)))
        .filter(|j| j["state"] != "succeeded")
        .map(|j| {
            let mut f = json!({ "id": j["id"], "state": j["state"] });
            if let Some(code) = j["error"].get("code") {
                f["code"] = code.clone();
            }
            f
        })
        .collect()
}

/// What a `jobs.wait` answer means: `{jobs}` when they succeeded, else `JOB_FAILED`.
pub fn judge(answer: Value, any: bool) -> Result<Value, CoreError> {
    let jobs = answer["jobs"].as_array().cloned().unwrap_or_default();
    let failed = failed(&jobs, any);
    if failed.is_empty() {
        return Ok(answer);
    }
    let (n, of) = (failed.len(), jobs.len());
    Err(
        CoreError::new("JOB_FAILED", format!("{n} of {of} job(s) did not succeed"))
            .with_details(json!({ "failed": failed, "jobs": jobs }))
            .with_fix("each job's error is in details.failed, and each whole job in details.jobs"),
    )
}

pub fn wait(backend: &dyn Backend, a: &WaitArgs) -> Result<Value, CoreError> {
    let secs = a.timeout.unwrap_or(WAIT_DEFAULT_S);
    if !(1..=WAIT_MAX_S).contains(&secs) {
        return Err(usage(
            format!("--timeout {secs} is outside 1 to {WAIT_MAX_S} seconds"),
            json!({ "timeout": secs, "max": WAIT_MAX_S }),
            format!("wait at most {WAIT_MAX_S} seconds, then run fin wait again"),
        ));
    }
    if backend.dry_run_only() {
        return Err(offline("jobs.wait"));
    }
    let wait = Duration::from_secs(secs);
    // A long call with its own timeout: outside the overall deadline, as streams are.
    let mut client = Client::connect(&endpoint::resolve()?, wait + WAIT_SLACK)?.without_deadline();
    let mut payload = json!({ "ids": a.ids, "timeoutMs": secs * 1000 });
    if a.any {
        payload["any"] = json!(true);
    }
    judge(client.call("jobs.wait", payload)?, a.any)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, state: &str, code: Option<&str>) -> Value {
        let mut j = json!({ "id": id, "state": state });
        if state != "queued" && state != "running" {
            j["finishedAt"] = json!(1);
        }
        if let Some(c) = code {
            j["error"] = json!({ "code": c, "message": "m", "details": {} });
        }
        j
    }

    #[test]
    fn wait_succeeds_only_when_every_job_did() {
        let ok = json!({ "jobs": [job("a", "succeeded", None)] });
        assert_eq!(judge(ok.clone(), false).unwrap(), ok);
        let bad = json!({ "jobs": [job("a", "succeeded", None), job("b", "failed", Some("WORKER_CRASHED")), job("c", "cancelled", Some("JOB_CANCELLED"))] });
        let e = judge(bad, false).unwrap_err();
        assert_eq!(e.code, "JOB_FAILED");
        assert_eq!(crate::output::exit_code(&e.code), 4);
        assert_eq!(
            e.details["failed"],
            json!([{ "id": "b", "state": "failed", "code": "WORKER_CRASHED" }, { "id": "c", "state": "cancelled", "code": "JOB_CANCELLED" }])
        );
        assert_eq!(e.details["jobs"].as_array().unwrap().len(), 3);
        // `--any`: only the finished ones count.
        let any = json!({ "jobs": [job("a", "succeeded", None), job("b", "running", None)] });
        assert!(judge(any, true).is_ok());
        let any_bad =
            json!({ "jobs": [job("a", "failed", Some("JOB_TIMEOUT")), job("b", "queued", None)] });
        assert_eq!(
            judge(any_bad, true).unwrap_err().details["failed"][0]["id"],
            "a"
        );
    }

    #[test]
    fn targets_and_states_are_checked_before_anything_is_sent() {
        let both = TargetArgs {
            id: Some("j".into()),
            all: true,
        };
        let neither = TargetArgs {
            id: None,
            all: false,
        };
        for t in [both, neither] {
            assert_eq!(target(&t, "pause").unwrap_err().code, "USAGE");
        }
        let one = TargetArgs {
            id: Some("j".into()),
            all: false,
        };
        assert_eq!(target(&one, "resume").unwrap(), json!({ "job": "j" }));
        let ls = LsArgs {
            state: vec!["done".into()],
            kind: None,
            mine: false,
            limit: None,
            cursor: None,
        };
        assert_eq!(ls_payload(&ls).unwrap_err().code, "USAGE");
        let ls = LsArgs {
            state: vec!["failed".into()],
            kind: Some("media.look".into()),
            mine: true,
            limit: Some(5),
            cursor: None,
        };
        assert_eq!(
            ls_payload(&ls).unwrap(),
            json!({ "state": ["failed"], "kind": "media.look", "mine": true, "limit": 5 })
        );
    }

    #[test]
    fn job_codes_keep_their_exit_classes() {
        // The codes these commands answer with (the parity test checks them against
        // `JOB_ERROR_CODES` from the other side).
        for (code, exit) in [
            ("JOB_NOT_FOUND", 3),
            ("JOB_FAILED", 4),
            ("JOB_CANCELLED", 4),
            ("WAIT_TIMEOUT", 5),
            ("WATCH_LAGGING", 5),
            ("TOO_MANY_WATCHES", 5),
            ("SCOPE_DENIED", 7),
        ] {
            assert_eq!(crate::output::exit_code(code), exit, "{code}");
        }
    }
}
