//! The `hook` profile (7F Task 9). The plugin's `hello` names `plugin.hook` v1 beside
//! `plugin.command` v1; then, for every hook the manifest declares (`contributes.hooks`):
//! - **warm:** in one warm process, a `plugin.hook` v1 job calling its handler with a sample
//!   request of its event answers within the hook's timeout (`PLUGIN_TIMEOUT`), without
//!   crashing (`PLUGIN_CRASHED`) or breaking the envelope (`PLUGIN_PROTOCOL`, `ready` after
//!   each job), and its result is one valid reply: `allow`, `block {reason}` or `context
//!   {text}`, nothing else, within Decision 2's limits (`HOOK_REPLY_INVALID`: a `modify` reply,
//!   an unknown field, a reason over 1 KiB or a text over 16 KiB after control characters are
//!   stripped, `{reason: tooLarge, field, bytes, max}`). The real
//!   host takes any of these as a failed hook resolved by the event's `onTimeout`;
//! - **cold:** each handler that passed warm also runs in a cold process, which exits 0;
//! - **calls** and **logs** are checked as in the `command` profile.
//!
//! The manifest's commands are not run here (the `command` profile runs them).

use std::time::{Duration, Instant};

use fin_jobs::KindVersion;
use fin_plugin_api::protocol::{
    DEFAULT_HOOK_TIMEOUT_S, HOOK_KIND, HOOK_KIND_VERSION, HookEvent, HookProject, HookReply,
    HookRequest, HookSession,
};
use serde_json::{Value, json};

use super::command::Runner;
use super::judge::{answer_error, finding};
use crate::HarnessError;
use crate::fake_host::{Ended, FakeHost, Job, JobBody, JobRun};
use crate::fixtures::Snapshot;
use crate::plugin::Plugin;
use crate::process::PluginProcess;
use crate::report::{Finding, HookReport, Report, RunSummary};

/// A request of `event` as the hook engine sends it, with a plausible payload.
pub fn sample_request(event: HookEvent, project: &str, dir: &str) -> HookRequest {
    let session = HookSession {
        id: "s-fin-plugin-test".into(),
        role: "editor".into(),
        mode: fin_core::events::payloads::AgentMode::Draft,
        harness: fin_core::events::payloads::Harness::Builtin,
        root_branch: Some("agent/s-fin-plugin-test".into()),
    };
    let command = json!({ "rpc": "history.list", "command": "history list", "commands": ["history list"], "payload": {} });
    let merge = json!({ "via": "rpc", "rpc": "history.merge", "from": "agent/s-fin-plugin-test", "into": "main" });
    let payload = match event {
        HookEvent::SessionStart => json!({}),
        HookEvent::SessionEnd => json!({ "reason": "ended" }),
        HookEvent::UserPrompt => json!({ "text": "tighten the intro", "origin": "prompt" }),
        HookEvent::PreCommand | HookEvent::PostCommand => command,
        HookEvent::BranchReady => json!({
            "sessionId": "s-fin-plugin-test", "projectId": project,
            "branch": "agent/s-fin-plugin-test", "headSeq": 1
        }),
        HookEvent::PreMerge | HookEvent::PostMerge => merge,
        HookEvent::PreRender | HookEvent::PostRender => json!({}),
        HookEvent::JobDone => json!({
            "jobId": "j-fin-plugin-test", "kind": "media.probe", "kindVersion": 1,
            "state": "done", "project": project, "owner": "local"
        }),
        HookEvent::Approval => json!({ "approval": { "id": "a1", "kind": "command" } }),
        HookEvent::Notification => json!({ "kind": "status", "status": "idle" }),
    };
    let session = (event != HookEvent::JobDone).then_some(session);
    let project = HookProject {
        id: project.into(),
        dir: dir.into(),
    };
    HookRequest::new(event, session, Some(project), payload)
}

struct Hook {
    handler: String,
    timeout_s: u16,
    request: HookRequest,
    report: HookReport,
}

/// A hook run's summary; its findings go to `out`.
fn judge(hook: &str, phase: &'static str, run: &JobRun, out: &mut Vec<Finding>) -> RunSummary {
    let mut summary = RunSummary {
        outcome: "error",
        ms: run.ms,
        ops: None,
        code: None,
        after_cancel_ms: None,
    };
    let mut find = |check, code: &str, message: String, details: Value| {
        out.push(finding(
            check,
            code,
            Some(hook),
            Some(phase),
            message,
            details,
        ));
    };
    match &run.ended {
        Ended::Result(v) => match serde_json::from_value::<HookReply>(v.clone()) {
            Ok(r) => match r.checked() {
                Ok(_) => summary.outcome = "reply",
                Err(t) => {
                    summary.outcome = "protocol";
                    find(
                        "reply",
                        "HOOK_REPLY_INVALID",
                        format!(
                            "{hook} answered a {} of {} bytes, over {} (after control characters are stripped)",
                            t.field, t.bytes, t.max
                        ),
                        json!({ "reason": "tooLarge", "field": t.field, "bytes": t.bytes, "max": t.max }),
                    );
                }
            },
            Err(e) => {
                summary.outcome = "protocol";
                find(
                    "reply",
                    "HOOK_REPLY_INVALID",
                    format!("{hook} answered something that is not allow, block or context: {e}"),
                    json!({ "error": e.to_string() }),
                );
            }
        },
        Ended::Error(e) => {
            let e = answer_error(e);
            summary.code = Some(e.code.clone());
            find(
                "answer",
                &e.code,
                format!("{hook} failed: {}", e.message),
                e.details,
            );
        }
        Ended::Timeout => {
            summary.outcome = "timeout";
            find(
                "answer",
                "PLUGIN_TIMEOUT",
                format!("{hook} did not answer within its timeout"),
                Value::Null,
            );
        }
        Ended::Crashed(code) => {
            summary.outcome = "crashed";
            find(
                "answer",
                "PLUGIN_CRASHED",
                format!("the process ended during {hook} (exit {code:?})"),
                json!({ "exitCode": code }),
            );
        }
        Ended::Protocol(m) => {
            summary.outcome = "protocol";
            find("protocol", "PLUGIN_PROTOCOL", m.clone(), Value::Null);
        }
    }
    summary
}

/// The reply's kind, when the run ended with a valid one (Decision 2's limits included).
fn reply_kind(run: &JobRun) -> Option<&'static str> {
    match &run.ended {
        Ended::Result(v) => match serde_json::from_value::<HookReply>(v.clone())
            .ok()?
            .checked()
            .ok()?
        {
            HookReply::Allow {} => Some("allow"),
            HookReply::Block { .. } => Some("block"),
            HookReply::Context { .. } => Some("context"),
        },
        _ => None,
    }
}

impl Runner<'_> {
    /// A process whose `hello` names `plugin.hook` v1 too; a finding (and `None`) otherwise.
    fn start_hooks(&mut self, warm: bool, hook: &str) -> Option<PluginProcess> {
        let p = self.start(warm, Some(hook))?;
        if p.hello_kinds
            .contains(&KindVersion::new(HOOK_KIND, HOOK_KIND_VERSION))
        {
            return Some(p);
        }
        self.findings.push(finding(
            "protocol",
            "PLUGIN_PROTOCOL",
            Some(hook),
            Some("hello"),
            format!(
                "hello does not name {HOOK_KIND} v{HOOK_KIND_VERSION}, and the manifest has hooks"
            ),
            json!({ "reason": "hello" }),
        ));
        self.retire(p);
        None
    }

    /// Runs `hook` in a process (the warm one, started when there is none, or a new cold one)
    /// and judges it; `false` when no process would start.
    fn run_hook(&mut self, hook: &mut Hook, phase: &'static str) -> bool {
        let warm = phase == "warm";
        let mut submitted = None;
        let mut p = match self.warm.take().filter(|_| warm) {
            Some(p) => p,
            None => {
                submitted = Some(Instant::now());
                match self.start_hooks(warm, &hook.handler) {
                    Some(p) => p,
                    None => return false,
                }
            }
        };
        let job = Job {
            command: &hook.handler,
            body: JobBody::Hook {
                request: hook.request.clone(),
            },
            submitted,
            timeout: Duration::from_secs(hook.timeout_s.into()),
            phase,
            cancel: false,
        };
        let run = self.host.run(&mut p, &job);
        let summary = judge(&hook.handler, phase, &run, &mut self.findings);
        if warm {
            hook.report.reply = reply_kind(&run);
            hook.report.warm = Some(summary);
        } else {
            hook.report.cold = Some(summary);
        }
        let ended = matches!(run.ended, Ended::Result(_) | Ended::Error(_));
        if ended {
            match FakeHost::settle(&mut p) {
                Ok(()) if warm => {
                    self.warm = Some(p);
                    return true;
                }
                Ok(()) => {}
                Err(m) => self.findings.push(finding(
                    "protocol",
                    "PLUGIN_PROTOCOL",
                    Some(&hook.handler),
                    Some(phase),
                    m,
                    json!({ "reason": if warm { "ready" } else { "exit" } }),
                )),
            }
        }
        self.retire(p);
        true
    }
}

pub fn run(plugin: &Plugin, snapshot: &Snapshot) -> Result<Report, HarnessError> {
    let m = &plugin.manifest;
    let mut runner = Runner::new(plugin, snapshot)?;
    let dir = runner.host.dirs().tmp.display().to_string();
    let mut hooks: Vec<Hook> = m
        .contributes
        .hooks
        .iter()
        .flatten()
        .map(|h| {
            let timeout_s = h.timeout_s.unwrap_or(DEFAULT_HOOK_TIMEOUT_S);
            let request = sample_request(h.event, &snapshot.project.id, &dir);
            Hook {
                handler: h.handler.clone(),
                timeout_s,
                report: HookReport {
                    event: h.event.name().into(),
                    handler: h.handler.clone(),
                    timeout_s,
                    request: serde_json::to_value(&request).unwrap_or(Value::Null),
                    reply: None,
                    warm: None,
                    cold: None,
                },
                request,
            }
        })
        .collect();
    for hook in &mut hooks {
        if !runner.run_hook(hook, "warm") {
            break;
        }
    }
    runner.shut_warm();
    for hook in &mut hooks {
        let passed = hook.report.reply.is_some()
            && !runner
                .findings
                .iter()
                .chain(&runner.host.violations)
                .any(|f| f.command.as_deref() == Some(&hook.handler) && f.phase == Some("warm"));
        if passed {
            runner.run_hook(hook, "cold");
        }
    }
    let findings = runner.scan_logs();
    Ok(Report {
        ok: findings.is_empty(),
        profile: "hook",
        plugin: plugin.name(),
        runtime: plugin.program.runtime(),
        fixture: json!({ "source": snapshot.source, "rev": snapshot.rev }),
        commands: Vec::new(),
        hooks: hooks.into_iter().map(|h| h.report).collect(),
        calls: std::mem::take(&mut runner.host.records),
        findings,
    })
}
