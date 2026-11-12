//! The `command` profile (spec §7.4, sp8b Task 14). For every command the manifest declares:
//! - **args:** arguments outside its schema are refused by the host (`PLUGIN_COMMAND_ARGS`)
//!   and never delivered, so no plugin relies on its own validation; the runs use sample
//!   arguments made from the schema, checked the same way;
//! - **warm:** in one warm process (as commands run), it answers within its timeout
//!   (`PLUGIN_TIMEOUT`), without crashing (`PLUGIN_CRASHED`, the process is started again for
//!   the next command) or breaking the envelope (`PLUGIN_PROTOCOL`, `ready` after each job),
//!   and what it returns is a command result whose transaction fin-core applies to the
//!   fixture (`PLUGIN_TX_INVALID`); an error answer on the fixture is a finding too;
//! - **cancel:** cancelled at its first call (or 200 ms in), its job ends within 2 s;
//! - **cold:** each command that passed warm also runs in a cold process, which exits 0;
//! - **calls:** every call outside the declared permissions, to an undeclared origin, of a
//!   method host API 1.1 lacks, or with params that do not decode is a finding;
//! - **logs:** no `log` message holds a path or the run's canary secret (the fake API's
//!   cookie), and no stderr line holds the secret.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant};

use fin_plugin_api::protocol::DEFAULT_COMMAND_TIMEOUT_S;
use fin_plugin_api::schema_subset::CompiledArgs;
use fin_plugin_api::{FsAccess, FsScope, Permission};
use serde_json::{Value, json};

use super::judge::{Judge, finding};
use crate::calls::Calls;
use crate::fake_host::{Ended, FakeHost, Job};
use crate::fake_https::FakeApi;
use crate::fixtures::{Snapshot, outside_args, sample_args};
use crate::plugin::{Plugin, absolute};
use crate::process::{PluginProcess, RunDirs};
use crate::report::{CommandReport, Finding, Report};
use crate::{HarnessError, scan};

/// A secret no plugin can know: 128 random bits as hex.
fn canary() -> String {
    let word = || RandomState::new().build_hasher().finish();
    format!("fin-test-secret-{:016x}{:016x}", word(), word())
}

struct Cmd {
    name: String,
    schema: CompiledArgs,
    writes: bool,
    timeout_s: u16,
    args: Option<Value>,
    report: CommandReport,
}

/// The run's processes and what they leave behind.
struct Runner<'a> {
    host: FakeHost<'a>,
    judge: Judge<'a>,
    findings: Vec<Finding>,
    stderr: Vec<String>,
    warm: Option<PluginProcess>,
}

impl Runner<'_> {
    /// Keeps a process's stderr and kills it.
    fn retire(&mut self, mut p: PluginProcess) {
        p.kill();
        self.stderr.extend(p.stderr_lines());
    }

    /// A new process that said a good `hello`; a finding (and `None`) otherwise.
    fn start(&mut self, warm: bool, command: Option<&str>) -> Option<PluginProcess> {
        let mut p = match self.host.spawn(warm) {
            Ok(p) => p,
            Err(e) => {
                self.findings.push(finding(
                    "answer",
                    &e.code,
                    command,
                    Some("hello"),
                    e.message,
                    Value::Object(e.details),
                ));
                return None;
            }
        };
        match FakeHost::hello(&mut p) {
            Ok(_) => Some(p),
            Err((code, message)) => {
                let check = if code == "PLUGIN_PROTOCOL" {
                    "protocol"
                } else {
                    "answer"
                };
                self.findings.push(finding(
                    check,
                    code,
                    command,
                    Some("hello"),
                    message,
                    Value::Null,
                ));
                self.retire(p);
                None
            }
        }
    }

    /// Runs `cmd` in the warm process (started when there is none); `false` when no process
    /// would start.
    fn warm_run(&mut self, cmd: &mut Cmd, phase: &'static str) -> bool {
        let Some(args) = cmd.args.clone() else {
            return true;
        };
        // A process started for this job counts in its timeout, as the host counts from submit.
        let mut submitted = None;
        let mut p = match self.warm.take() {
            Some(p) => p,
            None => {
                submitted = Some(Instant::now());
                match self.start(true, Some(&cmd.name)) {
                    Some(p) => p,
                    None => return false,
                }
            }
        };
        let job = Job {
            command: &cmd.name,
            schema: &cmd.schema,
            submitted,
            args,
            timeout: Duration::from_secs(cmd.timeout_s.into()),
            phase,
            cancel: phase == "cancel",
        };
        let run = self.host.run(&mut p, &job);
        let summary = self.judge.run(
            &cmd.name,
            cmd.writes,
            cmd.timeout_s,
            phase,
            &run,
            &mut self.findings,
        );
        // A job that ended by the envelope (a result or an error) leaves a usable process.
        let ended = matches!(run.ended, Ended::Result(_) | Ended::Error(_));
        if phase == "cancel" {
            cmd.report.cancel = Some(summary);
        } else {
            cmd.report.warm = Some(summary);
        }
        if ended {
            match FakeHost::settle(&mut p) {
                Ok(()) => self.warm = Some(p),
                Err(m) => {
                    self.findings.push(finding(
                        "protocol",
                        "PLUGIN_PROTOCOL",
                        Some(&cmd.name),
                        Some(phase),
                        m,
                        json!({ "reason": "ready" }),
                    ));
                    self.retire(p);
                }
            }
        } else {
            self.retire(p);
        }
        true
    }

    fn cold_run(&mut self, cmd: &mut Cmd) {
        let Some(args) = cmd.args.clone() else { return };
        let submitted = Some(Instant::now());
        let Some(mut p) = self.start(false, Some(&cmd.name)) else {
            return;
        };
        let job = Job {
            command: &cmd.name,
            schema: &cmd.schema,
            submitted,
            args,
            timeout: Duration::from_secs(cmd.timeout_s.into()),
            phase: "cold",
            cancel: false,
        };
        let run = self.host.run(&mut p, &job);
        let summary = self.judge.run(
            &cmd.name,
            cmd.writes,
            cmd.timeout_s,
            "cold",
            &run,
            &mut self.findings,
        );
        if matches!(run.ended, Ended::Result(_) | Ended::Error(_))
            && let Err(m) = FakeHost::settle(&mut p)
        {
            self.findings.push(finding(
                "protocol",
                "PLUGIN_PROTOCOL",
                Some(&cmd.name),
                Some("cold"),
                m,
                json!({ "reason": "exit" }),
            ));
        }
        cmd.report.cold = Some(summary);
        self.retire(p);
    }
}

fn io(op: &str, e: std::io::Error) -> HarnessError {
    HarnessError::new("IO_ERROR", format!("{op} the run's directories: {e}")).with("op", op)
}

pub fn run(plugin: &Plugin, snapshot: &Snapshot) -> Result<Report, HarnessError> {
    let m = &plugin.manifest;
    // The run's private directories, gone when it ends.
    let root = tempfile::Builder::new()
        .prefix("fin-plugin-test-")
        .tempdir()
        .map_err(|e| io("create", e))?;
    std::fs::create_dir(root.path().join("tmp")).map_err(|e| io("create", e))?;
    let tmp = absolute(&root.path().join("tmp"))?;
    let data = match m.permissions.iter().find_map(|p| match p {
        Permission::Fs {
            scope: FsScope::PluginData,
            access,
        } => Some(*access),
        _ => None,
    }) {
        Some(access) => {
            std::fs::create_dir(root.path().join("data")).map_err(|e| io("create", e))?;
            Some((
                absolute(&root.path().join("data"))?,
                access == FsAccess::ReadWrite,
            ))
        }
        None => None,
    };
    let dirs = RunDirs { tmp, data };
    let secret = canary();
    let hosts: Vec<String> = m
        .permissions
        .iter()
        .flat_map(|p| match p {
            Permission::Net { origins } => origins.iter().map(|o| o.0.clone()).collect(),
            _ => Vec::new(),
        })
        .filter_map(|o| crate::calls::parse_https(&o).ok().map(|t| t.host))
        .collect();
    let api = if hosts.is_empty() {
        None
    } else {
        Some(FakeApi::start(&hosts, &secret).map_err(|e| {
            HarnessError::new(
                "IO_ERROR",
                format!("the fake HTTPS API does not start: {e}"),
            )
            .with("op", "fakeApi")
        })?)
    };
    let calls = Calls::new(m.permissions.clone(), snapshot.clone(), api);
    let mut known_paths = vec![
        plugin.dir.display().to_string(),
        dirs.tmp.display().to_string(),
    ];
    if let Some((d, _)) = &dirs.data {
        known_paths.push(d.display().to_string());
    }
    let mut runner = Runner {
        host: FakeHost::new(plugin, dirs, calls),
        judge: Judge { plugin, snapshot },
        findings: Vec::new(),
        stderr: Vec::new(),
        warm: None,
    };

    let mut cmds = Vec::new();
    for c in &m.contributes.commands {
        let timeout_s = c.timeout_s.unwrap_or(DEFAULT_COMMAND_TIMEOUT_S);
        let mut report = CommandReport {
            name: c.name.clone(),
            timeout_s,
            args: Value::Null,
            outside_args_refused: false,
            skipped: None,
            warm: None,
            cold: None,
            cancel: None,
        };
        // The manifest validated, so its schema compiles.
        let schema = CompiledArgs::new(&c.args).map_err(|e| {
            HarnessError::new(
                "PLUGIN_MANIFEST_INVALID",
                format!("{}'s args schema: {}", c.name, e.message),
            )
        })?;
        // A host-side guarantee (the subset refuses unknown keys at every object); what the
        // fake host actually sent is checked after the runs.
        report.outside_args_refused = FakeHost::check_args(&schema, &outside_args()).is_err();
        let args = sample_args(&schema, c.args.as_value());
        match &args {
            Some(a) => report.args = a.clone(),
            None => {
                report.skipped = Some("no sample arguments pass the schema: not run".into());
                runner.findings.push(finding(
                    "args",
                    "COMMAND_NOT_RUN",
                    Some(&c.name),
                    None,
                    "no sample arguments made from the schema pass it, so the command was not tested",
                    Value::Null,
                ));
            }
        }
        cmds.push(Cmd {
            name: c.name.clone(),
            schema,
            writes: c.writes,
            timeout_s,
            args,
            report,
        });
    }

    'phases: for phase in ["warm", "cancel"] {
        for cmd in &mut cmds {
            if !runner.warm_run(cmd, phase) {
                break 'phases;
            }
        }
    }
    if let Some(mut p) = runner.warm.take() {
        if let Err(m) = FakeHost::shutdown(&mut p) {
            runner.findings.push(finding(
                "protocol",
                "PLUGIN_PROTOCOL",
                None,
                Some("warm"),
                m,
                json!({ "reason": "shutdown" }),
            ));
        }
        runner.retire(p);
    }
    for cmd in &mut cmds {
        let passed = cmd
            .report
            .warm
            .as_ref()
            .is_some_and(|w| matches!(w.outcome, "tx" | "data"))
            && !runner
                .findings
                .iter()
                .chain(&runner.host.violations)
                .any(|f| f.command.as_deref() == Some(&cmd.name) && f.phase == Some("warm"));
        if passed {
            runner.cold_run(cmd);
        }
    }

    // Every `start` the fake host sent carried arguments inside its command's schema.
    for (command, args) in &runner.host.sent {
        let schema = cmds.iter().find(|c| &c.name == command).map(|c| &c.schema);
        if schema.is_none_or(|s| FakeHost::check_args(s, args).is_err()) {
            runner.findings.push(finding(
                "args",
                "ARGS_DELIVERED",
                Some(command),
                None,
                "a job reached the plugin with arguments outside its command's schema",
                Value::Null,
            ));
        }
    }

    let secrets = [secret];
    for line in &runner.host.calls.logs {
        if let Some(reason) = scan::leak(&line.msg, &secrets, &known_paths) {
            runner.findings.push(finding(
                "log",
                "LOG_LEAK",
                Some(&line.command),
                Some(line.phase),
                format!("a log message holds a {reason}"),
                json!({ "reason": reason, "level": line.level }),
            ));
        }
    }
    if runner
        .stderr
        .iter()
        .any(|l| scan::leak(l, &secrets, &[]) == Some("secret"))
    {
        runner.findings.push(finding(
            "log",
            "LOG_LEAK",
            None,
            None,
            "a stderr line holds a secret",
            json!({ "reason": "secret", "stream": "stderr" }),
        ));
    }

    let mut findings = std::mem::take(&mut runner.findings);
    findings.append(&mut runner.host.violations);
    Ok(Report {
        ok: findings.is_empty(),
        profile: "command",
        plugin: plugin.name(),
        runtime: plugin.program.runtime(),
        fixture: json!({ "source": snapshot.source, "rev": snapshot.rev }),
        commands: cmds.into_iter().map(|c| c.report).collect(),
        calls: std::mem::take(&mut runner.host.records),
        findings,
    })
}
