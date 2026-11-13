//! The script file: what the fake plays after its handshake.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Script {
    pub version: Option<String>,
    pub model: Option<String>,
    pub auth: Option<Map<String, Value>>,
    pub initialize: Map<String, Value>,
    pub init: Map<String, Value>,
    pub observed: Option<String>,
    pub on_interrupt: OnInterrupt,
    pub on_sigint: OnInterrupt,
    /// How long the first turn waits for an SDK MCP server's handshake (ms).
    pub mcp_timeout_ms: Option<u64>,
    /// Flag values this "version" refuses, e.g. `{"--permission-prompt-tool": ["stdio"]}`.
    pub refuse_flag_values: BTreeMap<String, Vec<String>>,
    /// What `--resume` does with the transcript's cumulative totals: by default as 2.1.321 does
    /// (Task 0): restored when the last process exited cleanly, restarted after a kill.
    pub resume_totals: ResumeTotals,
    /// What a resumed process does with the tool calls its transcript holds without a result
    /// (Task 10: unverified on the real CLI, so both are modelled).
    pub on_resume_dangling: OnDangling,
    pub steps: Vec<Step>,
}

/// A resumed transcript's `tool_use` blocks that never got a `tool_result`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnDangling {
    /// Left as they are (the next prompt drives the session).
    #[default]
    Leave,
    /// Asked again at the first turn: a `can_use_tool` with the same `tool_use_id`; the call
    /// runs again (`reran` in the observations) only when the host allows it.
    Reask,
}

/// Whether a resumed process's `result` totals count the earlier runs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResumeTotals {
    /// Restored iff the transcript ends with the clean exit's `cost-state` entry (Task 0, s1d/s1e).
    #[default]
    Auto,
    /// The transcript's totals carry on (the CLI reference: "totals restored from earlier runs").
    Restored,
    /// The resumed process counts from zero.
    Reset,
}

/// What an interrupt (the control request, or SIGINT) does.
#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnInterrupt {
    /// End the open turn with an `error_during_execution` result; keep running.
    #[default]
    Result,
    /// Nothing (the control request is still answered).
    Ignore,
    /// End the open turn's result, then exit (0; 130 for SIGINT).
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Expect {
    User,
    Control,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Step {
    pub expect: Option<Expect>,
    pub subtype: Option<String>,
    pub sleep: Option<u64>,
    pub write_file: Option<WriteFile>,
    pub spawn: Option<Spawn>,
    pub emit: Vec<Value>,
    pub unknown_line: bool,
    pub ask: Option<Ask>,
    pub on_allow: Branch,
    pub on_deny: Branch,
    pub not_logged_in: bool,
    pub cumulative_usage: Option<Usage>,
    /// One model call's usage as `stream_event` lines (`message_start` with the input side,
    /// `message_delta` with the output), added to the totals the next `result` reports.
    pub live_usage: Option<Usage>,
    /// Usage added to the totals with no line (spend the stream did not show, e.g. a call an
    /// interrupt cut short): only the next `result` reports it.
    pub hidden_usage: Option<Usage>,
    /// The model `cumulative_usage`/`live_usage` is charged to in `modelUsage` (default: the
    /// session's model).
    pub usage_model: Option<String>,
    pub text: Option<String>,
    pub crash: Option<Crash>,
}

/// What follows an `ask`: lines, or side effects then lines.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Branch {
    Lines(Vec<Value>),
    Then(Then),
}

impl Default for Branch {
    fn default() -> Self {
        Branch::Lines(Vec::new())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Then {
    pub write_file: Option<WriteFile>,
    pub spawn: Option<Spawn>,
    pub emit: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteFile {
    pub path: String,
    #[serde(default)]
    pub contents: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    pub argv: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Ask {
    pub tool: String,
    #[serde(default)]
    pub input: Value,
    /// Withdraw the request (`control_cancel_request`) when unanswered after this long.
    pub cancel_after_ms: Option<u64>,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crash {
    pub signal: Option<String>,
    pub code: Option<i32>,
}

pub fn load(path: &str) -> Result<Script, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("fake-claude: cannot read script {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("fake-claude: bad script {path}: {e}"))
}
