//! A command run's end, judged as the real host would take it (`command-run.ts`): a result
//! decodes as a `CommandOut`; a transaction comes only from a command whose manifest says
//! `writes`, with `project:write`, and applies under fin-core to the snapshot the plugin read
//! (`baseRev` = the fixture's rev, `via` the plugin); an error, a timeout, a crash and a broken
//! envelope are findings.

use fin_core::{MemIds, SCHEMA_VERSION, Tx, Via, apply_tx};
use fin_plugin_api::Permission;
use fin_plugin_api::protocol::CommandOut;
use serde_json::{Value, json};

use crate::fake_host::{Ended, JobRun};
use crate::fixtures::Snapshot;
use crate::plugin::Plugin;
use crate::report::{Finding, RunSummary};

/// Codes a plugin's `error` keeps (`plugins/host/plugin-errors.ts`'s `PLUGIN_ANSWER_CODES`).
const PLUGIN_ANSWER_CODES: &[&str] = &[
    "PLUGIN_COMMAND_ARGS",
    "PLUGIN_PROTOCOL",
    "PLUGIN_OUTPUT_INVALID",
    "PLUGIN_COMMAND_FAILED",
    "PLUGIN_PERMISSION_DENIED",
    "EGRESS_DENIED",
    "EGRESS_NOT_HTTPS",
    "EGRESS_PRIVATE_ADDRESS",
    "EGRESS_TOO_LARGE",
    "EGRESS_TIMEOUT",
    "EGRESS_RATE_LIMITED",
    "EGRESS_UNREACHABLE",
    "SECRET_MISSING",
];

/// A plugin's `error` as the host takes it (`pluginAnswerError`): `JOB_CANCELLED` and the
/// codes above kept, any other code `PLUGIN_COMMAND_FAILED {cause: plugin}`.
pub fn answer_error(e: &fin_jobs::JobError) -> fin_jobs::JobError {
    if e.code == "JOB_CANCELLED" || PLUGIN_ANSWER_CODES.contains(&e.code.as_str()) {
        return e.clone();
    }
    fin_jobs::JobError::new("PLUGIN_COMMAND_FAILED", "the plugin's job failed")
        .with_details(json!({ "cause": "plugin", "pluginCode": crate::calls::clip(&e.code) }))
}

pub struct Judge<'a> {
    pub plugin: &'a Plugin,
    pub snapshot: &'a Snapshot,
}

pub fn finding(
    check: &'static str,
    code: &str,
    command: Option<&str>,
    phase: Option<&'static str>,
    message: impl Into<String>,
    details: Value,
) -> Finding {
    Finding {
        check,
        code: code.into(),
        command: command.map(String::from),
        phase,
        message: message.into(),
        details,
    }
}

impl Judge<'_> {
    /// The run's summary; its findings go to `out`. In the `cancel` phase an error (the job's
    /// `JOB_CANCELLED`, or any other) is how a cancelled job ends, not a finding.
    pub fn run(
        &self,
        command: &str,
        writes: bool,
        timeout_s: u16,
        phase: &'static str,
        run: &JobRun,
        out: &mut Vec<Finding>,
    ) -> RunSummary {
        let mut summary = RunSummary {
            outcome: "error",
            ms: run.ms,
            ops: None,
            code: None,
            after_cancel_ms: run.after_cancel_ms,
        };
        let mut find = |check, code: &str, message: String, details: Value| {
            out.push(finding(
                check,
                code,
                Some(command),
                Some(phase),
                message,
                details,
            ));
        };
        match &run.ended {
            Ended::Result(v) => match serde_json::from_value::<CommandOut>(v.clone()) {
                Err(e) => {
                    summary.outcome = "protocol";
                    find(
                        "protocol",
                        "PLUGIN_PROTOCOL",
                        format!("{command} returned something that is not a command result: {e}"),
                        json!({ "error": e.to_string() }),
                    );
                }
                Ok(CommandOut::Data { .. }) => summary.outcome = "data",
                Ok(CommandOut::Tx { ops }) => {
                    summary.outcome = "tx";
                    summary.ops = Some(ops.len());
                    if let Some((code, message, details)) = self.tx_problem(command, writes, ops) {
                        let check = if code == "PLUGIN_PERMISSION_DENIED" {
                            "permission"
                        } else {
                            "ops"
                        };
                        find(check, code, message, details);
                    }
                }
            },
            Ended::Error(e) => {
                let e = &answer_error(e);
                summary.code = Some(e.code.clone());
                if phase != "cancel" {
                    find(
                        "answer",
                        &e.code,
                        format!("{command} failed on the fixture: {}", e.message),
                        e.details.clone(),
                    );
                }
            }
            Ended::Timeout => {
                summary.outcome = "timeout";
                if run.cancel_ignored {
                    find(
                        "cancel",
                        "PLUGIN_TIMEOUT",
                        format!("{command} did not end within 2 s of its cancel"),
                        json!({ "reason": "cancel", "limitMs": 2000 }),
                    );
                } else {
                    find(
                        "answer",
                        "PLUGIN_TIMEOUT",
                        format!("{command} did not answer within its {timeout_s} s timeout"),
                        json!({ "timeoutS": timeout_s }),
                    );
                }
            }
            Ended::Crashed(code) => {
                summary.outcome = "crashed";
                summary.code = Some("PLUGIN_CRASHED".into());
                find(
                    "answer",
                    "PLUGIN_CRASHED",
                    match code {
                        Some(c) => format!("the process exited {c} during {command}"),
                        None => format!("the process ended during {command}"),
                    },
                    json!({ "exitCode": code }),
                );
            }
            Ended::Protocol(m) => {
                summary.outcome = "protocol";
                summary.code = Some("PLUGIN_PROTOCOL".into());
                find(
                    "protocol",
                    "PLUGIN_PROTOCOL",
                    format!("{command} broke the envelope: {m}"),
                    json!({ "error": m }),
                );
            }
        }
        summary
    }

    /// Why the host would refuse a returned transaction, if it would.
    fn tx_problem(
        &self,
        command: &str,
        writes: bool,
        ops: Vec<fin_core::Op>,
    ) -> Option<(&'static str, String, Value)> {
        if !writes {
            return Some((
                "PLUGIN_TX_INVALID",
                format!(
                    "{command} returned a transaction, but its manifest does not say it writes"
                ),
                json!({ "reason": "readOnlyCommand" }),
            ));
        }
        let m = &self.plugin.manifest;
        if !m
            .permissions
            .iter()
            .any(|p| matches!(p, Permission::ProjectWrite))
        {
            return Some((
                "PLUGIN_PERMISSION_DENIED",
                format!("{command} returned a transaction without project:write"),
                json!({ "perm": "project:write", "missing": "grant" }),
            ));
        }
        let tx = Tx {
            tx_id: "fin-plugin-test".into(),
            author: "local".into(),
            branch: "main".into(),
            base_rev: self.snapshot.rev,
            schema_version: SCHEMA_VERSION,
            ops,
            args_hash: None,
            via: Some(Box::new(Via {
                plugin: self.plugin.name(),
                command: command.into(),
            })),
        };
        let project = &self.snapshot.project;
        match apply_tx(project, &tx, &mut MemIds::after(project)) {
            Ok(_) => None,
            Err(e) => Some((
                "PLUGIN_TX_INVALID",
                format!("fin-core refuses {command}'s transaction: {}", e.message),
                json!({ "problems": [e] }),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::Program;

    #[test]
    fn a_transaction_needs_project_write_and_unknown_codes_are_command_failed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/broken");
        let mut manifest: fin_plugin_api::Manifest =
            serde_json::from_slice(&std::fs::read(dir.join("finisher-plugin.json")).unwrap())
                .unwrap();
        manifest
            .permissions
            .retain(|p| !matches!(p, Permission::ProjectWrite));
        let plugin = Plugin {
            manifest,
            dir: dir.clone(),
            program: Program::Native { exe: dir },
        };
        let snapshot = Snapshot::builtin();
        let judge = Judge {
            plugin: &plugin,
            snapshot: &snapshot,
        };
        let tx = json!({ "kind": "tx", "ops": [{ "op": "item.delete", "item": "adj1", "mode": "lift" }] });
        let run = |ended| JobRun {
            ended,
            ms: 1,
            after_cancel_ms: None,
            cancel_ignored: false,
        };
        let mut out = Vec::new();
        let s = judge.run(
            "bad-tx",
            true,
            10,
            "warm",
            &run(Ended::Result(tx)),
            &mut out,
        );
        assert_eq!(s.outcome, "tx");
        assert_eq!(
            (out[0].check, out[0].code.as_str()),
            ("permission", "PLUGIN_PERMISSION_DENIED")
        );
        assert_eq!(
            out[0].details,
            json!({ "perm": "project:write", "missing": "grant" })
        );

        let mut out = Vec::new();
        let own = fin_jobs::JobError::new("MY_OWN_CODE", "no");
        judge.run("x", false, 10, "warm", &run(Ended::Error(own)), &mut out);
        assert_eq!(out[0].code, "PLUGIN_COMMAND_FAILED");
        assert_eq!(
            answer_error(&fin_jobs::JobError::cancelled()).code,
            "JOB_CANCELLED"
        );
    }
}
