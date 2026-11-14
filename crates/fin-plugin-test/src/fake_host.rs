//! The fake host: starts plugin processes, checks their `hello`, runs `plugin.command` v1 jobs
//! as the real host does (the args checked against the schema before a job exists, a
//! [`PluginContext`] in `start.extra`, a [`CommandRun`] in `params` at the fixture's rev under
//! a held-snapshot handle) and `plugin.hook` v1 jobs (a [`HookRun`] in `params`), answers their
//! calls through [`Calls`], cancels, and records every call and every call that should not have
//! been made.

use std::time::{Duration, Instant};

use fin_jobs::{
    Dirs, Hello, HostMsg, JobError, KindVersion, Limits, Role, Start, StartJob, WorkerMsg,
};
use fin_plugin_api::protocol::{
    COMMAND_KIND, COMMAND_KIND_VERSION, CallLimits, CommandRun, HOOK_KIND, HOOK_KIND_VERSION,
    HookRequest, HookRun, MAX_MESSAGE_BYTES, PluginContext, PluginDirs, PluginRefFull,
};
use fin_plugin_api::schema_subset::CompiledArgs;
use serde_json::{Value, json};

use crate::calls::{Calls, PAGE_BYTES};
use crate::plugin::Plugin;
use crate::process::{Event, PluginProcess, RunDirs, Sent};
use crate::report::{CallRecord, Finding};
use crate::{HarnessError, report};

/// How long a process may take to say `hello` (a cold Node start on a slow runner).
pub const HELLO_WAIT: Duration = Duration::from_secs(15);
/// A cancel is honoured when the job ends within this (spec §7.4).
pub const CANCEL_LIMIT: Duration = Duration::from_secs(2);
/// A cancel run cancels at the first call, or this long after the start.
pub const CANCEL_AFTER: Duration = Duration::from_millis(200);
/// How long after a job's end a warm process has to send `ready`, a cold one to exit.
pub const SETTLE_WAIT: Duration = Duration::from_secs(2);

/// How a job ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Ended {
    Result(Value),
    Error(JobError),
    /// No end within the command's timeout (it was then cancelled and given [`CANCEL_LIMIT`]).
    Timeout,
    /// The process's output closed before the job ended: its exit code, if it exited.
    Crashed(Option<i32>),
    /// A message that breaks the envelope or the job's protocol.
    Protocol(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRun {
    pub ended: Ended,
    pub ms: u64,
    /// A cancel run: from the cancel to the job's end, when it ended.
    pub after_cancel_ms: Option<u64>,
    /// A cancel run whose job did not end within [`CANCEL_LIMIT`] of the cancel.
    pub cancel_ignored: bool,
}

/// What a job runs.
pub enum JobBody<'a> {
    /// A `plugin.command` v1 job. The command's schema: the host checks `args` with it before
    /// anything is sent.
    Command {
        schema: &'a CompiledArgs,
        args: Value,
    },
    /// A `plugin.hook` v1 job calling the handler [`Job::command`] names.
    Hook { request: HookRequest },
}

/// One command or hook job to run.
pub struct Job<'a> {
    /// The command's name, or the hook's handler (in the records and findings).
    pub command: &'a str,
    pub body: JobBody<'a>,
    /// When the job was submitted, if before now: the process it runs on was started for it
    /// (the real host counts a command's timeout from the submit, the process start included).
    pub submitted: Option<Instant>,
    pub timeout: Duration,
    /// `warm`, `cold` or `cancel` (in the records).
    pub phase: &'static str,
    /// Cancel it (at its first call or after [`CANCEL_AFTER`]).
    pub cancel: bool,
}

pub struct FakeHost<'a> {
    plugin: &'a Plugin,
    dirs: RunDirs,
    pub calls: Calls,
    pub records: Vec<CallRecord>,
    /// Calls that should not have been made, as findings.
    pub violations: Vec<Finding>,
    /// Every command `start` sent: (command, args).
    pub sent: Vec<(String, Value)>,
    next_job: u64,
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl<'a> FakeHost<'a> {
    pub fn new(plugin: &'a Plugin, dirs: RunDirs, calls: Calls) -> FakeHost<'a> {
        FakeHost {
            plugin,
            dirs,
            calls,
            records: Vec::new(),
            violations: Vec::new(),
            sent: Vec::new(),
            next_job: 0,
        }
    }

    pub fn dirs(&self) -> &RunDirs {
        &self.dirs
    }

    /// The job's context (`start.extra`): the manifest's permissions as the grants.
    pub fn context(&self, timeout_s: u16) -> PluginContext {
        let m = &self.plugin.manifest;
        PluginContext {
            host_api: fin_plugin_api::host_api(),
            plugin: PluginRefFull {
                id: m.id.clone(),
                version: m.version.clone(),
            },
            grants: m.permissions.clone(),
            dirs: PluginDirs {
                plugin: self.plugin.dir.display().to_string(),
                data: self
                    .dirs
                    .data
                    .as_ref()
                    .map(|(d, _)| d.display().to_string()),
                tmp: self.dirs.tmp.display().to_string(),
            },
            locale: "en".into(),
            limits: CallLimits {
                timeout_s,
                message_bytes: MAX_MESSAGE_BYTES,
                page_bytes: PAGE_BYTES as u32,
            },
        }
    }

    pub fn spawn(&self, warm: bool) -> Result<PluginProcess, HarnessError> {
        PluginProcess::spawn(self.plugin, &self.dirs, warm)
    }

    /// The process's `hello`, checked: protocol 1, a worker of the `plugin` family serving
    /// `plugin.command` v1, warm exactly when started warm. `Err((code, message))` otherwise.
    pub fn hello(p: &mut PluginProcess) -> Result<Hello, (&'static str, String)> {
        let h = match p.recv(Instant::now() + HELLO_WAIT) {
            Some(Event::Msg(WorkerMsg::Hello(h))) => h,
            Some(Event::Eof) => {
                let code = p.wait_exit(Duration::from_secs(1)).and_then(|s| s.code());
                return Err((
                    "PLUGIN_CRASHED",
                    format!("the process exited before hello (exit {code:?})"),
                ));
            }
            None => {
                return Err((
                    "PLUGIN_TIMEOUT",
                    format!("no hello within {} s", HELLO_WAIT.as_secs()),
                ));
            }
            Some(other) => return Err(("PLUGIN_PROTOCOL", format!("hello first, got {other:?}"))),
        };
        let kind = KindVersion::new(COMMAND_KIND, COMMAND_KIND_VERSION);
        let problem = if h.v != fin_jobs::PROTOCOL {
            Some(format!("hello names protocol {}", h.v))
        } else if h.role != Role::Worker || h.family != "plugin" {
            Some(format!(
                "hello names role {:?}, family {:?}",
                h.role, h.family
            ))
        } else if !h.kinds.contains(&kind) {
            Some(format!(
                "hello does not name {COMMAND_KIND} v{COMMAND_KIND_VERSION}"
            ))
        } else if p.warm && !h.warm {
            // A cold process's `warm` is not read (`jobs/attempt.ts`).
            Some(format!(
                "hello says warm: {} for a process started warm: {}",
                h.warm, p.warm
            ))
        } else {
            None
        };
        match problem {
            Some(m) => Err(("PLUGIN_PROTOCOL", m)),
            None => {
                p.hello_kinds = h.kinds.clone();
                Ok(h)
            }
        }
    }

    /// Checks `args` against `schema` as the host does before a job exists:
    /// `PLUGIN_COMMAND_ARGS {problems}` and nothing sent when they do not pass.
    pub fn check_args(schema: &CompiledArgs, args: &Value) -> Result<(), JobError> {
        schema.validate(args).map_err(|problems| {
            JobError::new(
                "PLUGIN_COMMAND_ARGS",
                "the arguments do not match the command's schema",
            )
            .with_details(json!({ "problems": problems }))
        })
    }

    fn start_msg(&self, id: &str, job: &Job<'_>) -> HostMsg {
        let snapshot = self.calls.snapshot();
        let (kind, kind_version, params) = match &job.body {
            JobBody::Command { args, .. } => {
                let run = CommandRun {
                    command: job.command.into(),
                    args: args.clone(),
                    project: snapshot.project.id.clone(),
                    branch: "main".into(),
                    rev: snapshot.rev,
                    snapshot: Some(format!("held-{id}")),
                };
                let params = serde_json::to_value(run).expect("a command run serialises");
                (COMMAND_KIND, COMMAND_KIND_VERSION, params)
            }
            JobBody::Hook { request } => {
                let run = HookRun {
                    handler: job.command.into(),
                    request: request.clone(),
                };
                let params = serde_json::to_value(run).expect("a hook run serialises");
                (HOOK_KIND, HOOK_KIND_VERSION, params)
            }
        };
        let timeout_s = u16::try_from(job.timeout.as_secs()).unwrap_or(u16::MAX);
        HostMsg::Start(Start {
            job: StartJob {
                id: id.into(),
                kind: kind.into(),
                kind_version,
                params,
                checkpoint: None,
                attempt: 1,
                extra: Some(
                    serde_json::to_value(self.context(timeout_s)).expect("a context serialises"),
                ),
            },
            limits: Limits::default(),
            files: vec![],
            helpers: vec![],
            dirs: Dirs {
                job: self.dirs.tmp.clone(),
                tmp: self.dirs.tmp.clone(),
            },
            diagnostics: false,
        })
    }

    fn cancel(p: &mut PluginProcess, id: &str) {
        p.send(&HostMsg::Cancel {
            job: id.into(),
            reason: fin_jobs::CancelReason::Cancel,
        });
    }

    fn record(&mut self, job: &Job<'_>, method: &str, code: Option<String>, after_cancel: bool) {
        self.records.push(CallRecord {
            command: job.command.into(),
            phase: job.phase,
            method: method.into(),
            code,
            after_cancel,
        });
    }

    /// Answers a call; a reply over one envelope line is answered `INTERNAL` instead (the
    /// host's `CallRouter#reply`).
    fn answer(
        &mut self,
        p: &mut PluginProcess,
        job: &Job<'_>,
        call: u64,
        method: &str,
        params: &Value,
        after_cancel: bool,
    ) {
        let answer = self.calls.answer(method, params);
        let code = answer.reply.as_ref().err().map(|e| e.code.clone());
        self.record(job, method, code, after_cancel);
        if let (Some((check, message)), Err(e)) = (&answer.violation, &answer.reply) {
            self.violations.push(report::Finding {
                check,
                code: e.code.clone(),
                command: Some(job.command.into()),
                phase: Some(job.phase),
                message: message.clone(),
                details: e.details.clone(),
            });
        }
        let (value, error) = match answer.reply {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        };
        let reply = HostMsg::Reply {
            id: call,
            value,
            error,
        };
        if p.send(&reply) == Sent::TooLarge {
            let e = JobError::new("INTERNAL", "the reply does not fit one envelope line")
                .with_details(json!({ "reason": "replyTooLarge" }));
            p.send(&HostMsg::Reply {
                id: call,
                value: None,
                error: Some(e),
            });
        }
    }

    /// Runs one job on `p` to its end.
    pub fn run(&mut self, p: &mut PluginProcess, job: &Job<'_>) -> JobRun {
        self.next_job += 1;
        let id = format!("j{}", self.next_job);
        let start = job.submitted.unwrap_or_else(Instant::now);
        let done = |ended: Ended, cancelled: Option<Instant>| {
            let now = Instant::now();
            JobRun {
                ended,
                ms: ms(now - start),
                after_cancel_ms: cancelled.map(|c| ms(now - c)),
                cancel_ignored: false,
            }
        };
        // Arguments outside the schema never reach the plugin.
        if let JobBody::Command { schema, args } = &job.body
            && let Err(e) = Self::check_args(schema, args)
        {
            return done(Ended::Error(e), None);
        }
        self.calls.start_job(job.command, job.phase);
        let msg = self.start_msg(&id, job);
        match p.send(&msg) {
            Sent::Ok => {
                if let JobBody::Command { args, .. } = &job.body {
                    self.sent.push((job.command.into(), args.clone()));
                }
            }
            Sent::Gone => {
                let code = p.wait_exit(Duration::from_secs(1)).and_then(|s| s.code());
                return done(Ended::Crashed(code), None);
            }
            Sent::TooLarge => {
                let e = JobError::new(
                    "PLUGIN_COMMAND_ARGS",
                    "the job does not fit one envelope line",
                );
                return done(Ended::Error(e), None);
            }
        }
        let deadline = start + job.timeout;
        let mut cancel_due = job.cancel.then(|| Instant::now() + CANCEL_AFTER);
        let mut cancelled: Option<Instant> = None;
        let mut timed_out = false;
        let result = loop {
            let until = match (cancelled, cancel_due) {
                (Some(c), _) => c + CANCEL_LIMIT,
                (None, Some(c)) => c.min(deadline),
                (None, None) => deadline,
            };
            let event = p.recv(until);
            let now = Instant::now();
            match event {
                None => {
                    if let Some(c) = cancelled {
                        if now >= c + CANCEL_LIMIT {
                            let mut run = done(Ended::Timeout, None);
                            run.cancel_ignored = !timed_out;
                            return run;
                        }
                    } else if cancel_due.is_some_and(|c| now >= c) || now >= deadline {
                        // A due cancel, or the command's timeout: the host cancels the job.
                        timed_out = now >= deadline && cancel_due.is_none();
                        Self::cancel(p, &id);
                        cancelled = Some(now);
                        cancel_due = None;
                    }
                }
                Some(Event::Msg(WorkerMsg::Call {
                    id: call,
                    method,
                    params,
                })) => {
                    if cancelled.is_none() && cancel_due.is_some() {
                        // A cancel run: cancelled at its first call, which is still answered
                        // (the host answers calls whatever the job's state; an SDK drops it).
                        Self::cancel(p, &id);
                        cancelled = Some(now);
                        cancel_due = None;
                    }
                    let after = cancelled.is_some();
                    self.answer(p, job, call, &method, &params, after);
                }
                Some(Event::Msg(WorkerMsg::Result { job: j, value })) if j == id => {
                    break Ended::Result(value);
                }
                Some(Event::Msg(WorkerMsg::Error { job: j, error, .. })) if j == id => {
                    break Ended::Error(error);
                }
                Some(Event::Msg(
                    WorkerMsg::Progress(_) | WorkerMsg::Diag { .. } | WorkerMsg::Alive {},
                ))
                | Some(Event::Unknown(_)) => {}
                Some(Event::Msg(m)) => {
                    break Ended::Protocol(format!("a {} message during job {id}", m.type_name()));
                }
                Some(Event::Bad(e)) => break Ended::Protocol(e),
                Some(Event::Eof) => {
                    let code = p.wait_exit(Duration::from_secs(1)).and_then(|s| s.code());
                    break Ended::Crashed(code);
                }
            }
        };
        self.calls.end_job();
        if timed_out {
            return done(Ended::Timeout, None);
        }
        done(result, cancelled)
    }

    /// After a job's end: a warm process sends `ready`, a cold one exits 0.
    pub fn settle(p: &mut PluginProcess) -> Result<(), String> {
        if p.warm {
            return match p.recv(Instant::now() + SETTLE_WAIT) {
                Some(Event::Msg(WorkerMsg::Ready {})) => Ok(()),
                Some(Event::Unknown(t)) => Err(format!("a {t} message in place of ready")),
                Some(e) => Err(format!("{e:?} in place of ready")),
                None => Err(format!(
                    "no ready within {} s of the job's end",
                    SETTLE_WAIT.as_secs()
                )),
            };
        }
        match p.wait_exit(SETTLE_WAIT) {
            Some(s) if s.success() => Ok(()),
            Some(s) => Err(format!(
                "the cold process exited {:?} after its job",
                s.code()
            )),
            None => Err(format!(
                "the cold process still runs {} s after its job",
                SETTLE_WAIT.as_secs()
            )),
        }
    }

    /// Ends a warm process as the host does (`shutdown`); whether it exited 0 in time.
    pub fn shutdown(p: &mut PluginProcess) -> Result<(), String> {
        p.send(&HostMsg::Shutdown {});
        match p.wait_exit(SETTLE_WAIT) {
            Some(s) if s.success() => Ok(()),
            Some(s) => Err(format!("the process exited {:?} on shutdown", s.code())),
            None => {
                p.kill();
                Err(format!(
                    "the process still ran {} s after shutdown",
                    SETTLE_WAIT.as_secs()
                ))
            }
        }
    }
}
