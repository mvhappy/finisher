//! The script file: what the fake plays for each `turn/start`, and its few behaviour knobs.

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Script {
    pub version: Option<String>,
    pub observed: Option<String>,
    /// The account `account/read` reports when `auth.json` exists and no sidecar says otherwise.
    pub account: Option<Value>,
    /// Replaces the bundled `model/list` catalogue.
    pub models: Option<Vec<Value>>,
    /// The `requirements` of `configRequirements/read` (default `null`).
    pub requirements: Option<Value>,
    pub steer: Steer,
    pub on_interrupt: OnInterrupt,
    /// Whether `thread/resume` brings back the thread's dynamic tools (Task 0 Step 3 decides it
    /// per pin; default `true`).
    pub resume_restores_dynamic_tools: Option<bool>,
    /// `thread/revert` answers `threadRollbackFailed` and changes nothing.
    pub revert_fails: bool,
    pub turns: Vec<Turn>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Steer {
    /// `turn/steer` joins the active turn (the real answer for a normal turn).
    #[default]
    Join,
    /// `activeTurnNotSteerable`.
    /// UNOBSERVED(7E Task 0 Step 3): the refusal's shape (`-32600` with
    /// `data.codexErrorInfo.activeTurnNotSteerable {turnKind}`).
    Refuse,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnInterrupt {
    /// `turn/interrupt` ends the open turn with `turn/completed {status: "interrupted"}`.
    #[default]
    Completed,
    /// The request is answered and nothing else happens.
    Ignore,
    /// The turn ends interrupted, then the process exits 0.
    Exit,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Turn {
    pub steer: Option<Steer>,
    pub on_interrupt: Option<OnInterrupt>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Expect {
    Steer,
    Interrupt,
}

/// One step; its fields run in the order they are declared here (all optional).
///
/// UNOBSERVED(7E Task 0 Step 3): everything a signed-in turn does, as only the unsigned half of
/// Task 0 ran: the item shapes and order of `say` (agentMessage deltas), `usage`
/// (`thread/tokenUsage/updated` cadence), `dynamicTool`, `patch`, `fail` (a failure other than
/// the unsigned 401) and the closing order of a `completed` turn (seen for `failed` and
/// `interrupted` ones).
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Step {
    pub expect: Option<Expect>,
    pub sleep: Option<u64>,
    /// `{method, params}` notifications, written as they are (`"$threadId"` and `"$turnId"`
    /// are replaced; `emittedAtMs` is added when missing).
    pub emit: Vec<Value>,
    /// An agent message: one `item/agentMessage/delta`, then `item/completed`.
    pub say: Option<String>,
    pub usage: Option<Usage>,
    /// A notification no client knows: `true` (named `fake/unknownNotification`) or a method.
    pub unknown_notification: Option<Value>,
    /// UNOBSERVED(7E Task 0 Step 3): the `account/updated` shape (`{authMode, planType}`).
    pub account_change: Option<Map<String, Value>>,
    pub dynamic_tool: Option<DynamicTool>,
    pub patch: Option<Patch>,
    /// The unsigned turn of the real binary (the Task 0 capture): four WebSocket and five HTTPS
    /// 401 retries, then a 401 failure.
    pub not_logged_in: bool,
    pub fail: Option<Fail>,
    pub crash: Option<Crash>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Usage {
    pub last: Map<String, Value>,
    /// Default: the running total plus `last`.
    pub total: Option<Map<String, Value>>,
    pub model_context_window: Option<i64>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct DynamicTool {
    pub tool: String,
    pub arguments: Value,
    pub call_id: Option<String>,
    /// Notifications written after a `success: true` / any other answer.
    pub on_success: Vec<Value>,
    pub on_failure: Vec<Value>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Patch {
    pub changes: Vec<Value>,
    pub item_id: Option<String>,
    pub on_accept: Vec<Value>,
    pub on_decline: Vec<Value>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Fail {
    pub codex_error_info: Value,
    pub message: Option<String>,
    pub additional_details: Option<String>,
    /// `error {willRetry: true}` notifications before the failure.
    pub retries: u32,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Crash {
    /// `KILL`, `TERM`, `SEGV`, `INT` or `HUP`.
    pub signal: Option<String>,
    pub code: Option<i32>,
}
