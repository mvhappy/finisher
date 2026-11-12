//! What a profile run reports: per command what happened in each phase, every host call, and
//! the findings (each a failed check). `ok` is "no findings".

use serde::Serialize;
use serde_json::Value;

/// One failed check. `code` is the code the real host would answer or record for it
/// (`PLUGIN_TIMEOUT`, `PLUGIN_CRASHED`, `PLUGIN_PERMISSION_DENIED`, …) or, for a check the
/// host has no code for, the harness's own (`LOG_LEAK`, `ARGS_DELIVERED`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// `answer`, `args`, `ops`, `cancel`, `permission`, `protocol` or `log`.
    pub check: &'static str,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `hello`, `warm`, `cold` or `cancel`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<&'static str>,
    pub message: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

/// How one run of a command ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    /// `tx`, `data`, `error`, `timeout`, `crashed`, `protocol` or `skipped`.
    pub outcome: &'static str,
    pub ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops: Option<usize>,
    /// The error code the job ended with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// A cancel run: milliseconds from the cancel to the job's end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_cancel_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandReport {
    pub name: String,
    pub timeout_s: u16,
    /// The sample arguments the runs used (made from the schema).
    pub args: Value,
    /// Arguments outside the schema were refused by the host and never sent.
    pub outside_args_refused: bool,
    /// Why the command was not run (no sample arguments pass its schema).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warm: Option<RunSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cold: Option<RunSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel: Option<RunSummary>,
}

/// A host call the plugin made, as the fake host answered it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRecord {
    pub command: String,
    pub phase: &'static str,
    pub method: String,
    /// The error code it was answered with, if refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Not answered: it came after a cancel (the host holds it).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub ok: bool,
    pub profile: &'static str,
    /// `<id>@<version>`.
    pub plugin: String,
    pub runtime: &'static str,
    /// The fixture's source (`builtin` or `file`) and rev.
    pub fixture: Value,
    pub commands: Vec<CommandReport>,
    pub calls: Vec<CallRecord>,
    pub findings: Vec<Finding>,
}
