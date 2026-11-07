//! The agent kinds, version 1 (sub-project 7 §12.2; D-plan Decisions 3, 4, 20, 28):
//! `agent.session`, `agent.turn`, `agent.toolCall`, `agent.toolResult` and `agent.approval`.
//! The reserved v0 `agent.editVerdict` is retired: 5E's `review.verdict` is the single source
//! of verdicts (§12.5).
//!
//! `agent.turn`, `agent.toolCall` and `agent.toolResult` are training-only: under diagnostics
//! they keep their metadata and drop their content. Every session and turn carries its
//! **provenance** (`harness`, `provider`, `model`, `weights`; `upstream` on turns) as required
//! meta fields, so a later filter on it needs no join and no training content (§12.7).
//!
//! These are the stored forms. `finisherd` sends the wire forms of `wire_agent.rs`: raw model
//! ids and upstream names, which become a table name or a keyed hash, and unkeyed BLAKE3
//! hashes, which are keyed.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::Body;
use crate::events::approx::approx_struct;
use crate::events::class::{Code, Content, Hashed, IdRef, Name};
use crate::events::names;

/// The most txIds one `agent.toolResult` lists.
pub const MAX_TOOL_TX_IDS: usize = 64;

/// What runs the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Harness {
    Builtin,
    ClaudeCode,
    Codex,
    Fake,
    Plugin,
}

/// The session's mode (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AgentMode {
    Plan,
    Draft,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SessionPhase {
    Started,
    Resumed,
    Interrupted,
    Ended,
}

/// What started the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SessionTrigger {
    User,
    SavedPrompt,
    Subagent,
    Hook,
    Eval,
}

/// The model's weights class, from the catalogue at the time (§12.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Weights {
    Open,
    Closed,
    Unknown,
}

/// The kind of sign-in a CLI harness reports (CLI harnesses only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SignIn {
    Plan,
    Key,
    Unknown,
}

/// `agent.session`: a session started, resumed, was interrupted or ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub session_id: IdRef,
    pub phase: SessionPhase,
    pub harness: Harness,
    #[serde(deserialize_with = "names::agent_provider")]
    pub provider: Name,
    #[serde(deserialize_with = "names::agent_model")]
    pub model: Name,
    // The keyed hash of the model id when `model` is `other` (a slug the owner added).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Hashed")]
    pub model_hash: Option<Hashed>,
    pub weights: Weights,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "SignIn")]
    pub sign_in: Option<SignIn>,
    #[serde(deserialize_with = "names::agent_role")]
    pub role: Name,
    pub mode: AgentMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IdRef")]
    pub parent: Option<IdRef>,
    pub trigger: SessionTrigger,
    // Why the session was interrupted or ended (an error code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub reason: Option<Code>,
    // At `ended` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "SessionTotals")]
    pub totals: Option<SessionTotals>,
}
approx_struct!(AgentSession {
    session_id,
    phase,
    harness,
    provider,
    model,
    model_hash,
    weights,
    sign_in,
    role,
    mode,
    parent,
    trigger,
    reason,
    totals,
});

/// A session's totals at `ended`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionTotals {
    pub turns: u32,
    pub tool_calls: u32,
    pub txs: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub cost_micros: Option<u64>,
    pub wall_s: u64,
}
approx_struct!(SessionTotals {
    turns,
    tool_calls,
    txs,
    input_tokens,
    output_tokens,
    cost_micros,
    wall_s,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    Refusal,
    Other,
}

/// Where a turn's cost comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CostBasis {
    /// The provider reported what it charged (OpenRouter's usage accounting).
    Reported,
    /// Priced from the catalogue.
    Priced,
    /// A plan sign-in: notional, not charged.
    Subscription,
    Unknown,
}

/// How faithful a turn's content is (§12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Capture {
    /// The request and response bodies as sent and received (the built-in agent).
    Exact,
    /// What a CLI's stream reports.
    Stream,
}

/// `agent.turn`: one model call. Content (training only, in the agent-trace store): the
/// turn's new input messages, the output, and the system prompt and tool definitions, which
/// are always blobs so identical prompts are stored once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurn {
    pub turn_id: IdRef,
    pub run_id: IdRef,
    pub seq: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IdRef")]
    pub parent_turn_id: Option<IdRef>,
    pub harness: Harness,
    #[serde(deserialize_with = "names::agent_provider")]
    pub provider: Name,
    #[serde(deserialize_with = "names::agent_model")]
    pub model: Name,
    // As on `agent.session`: the keyed hash of the model id when `model` is `other`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Hashed")]
    pub model_hash: Option<Hashed>,
    pub weights: Weights,
    // The provider OpenRouter routed the call to (`AGENT_UPSTREAMS`); one the table lacks is
    // `upstreamHash` instead.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "names::agent_upstream_opt"
    )]
    #[schemars(with = "Name")]
    pub upstream: Option<Name>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Hashed")]
    pub upstream_hash: Option<Hashed>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub ttft_ms: Option<u64>,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub cost_micros: Option<u64>,
    pub cost_basis: CostBasis,
    pub capture: Capture,
    pub system_hash: Hashed,
    pub tools_hash: Hashed,
    // The keyed hash of the BLAKE3 of the full request body as sent (exact capture only): the
    // emitter sends the unkeyed `blake3:<hex>` and the wire form keys it, like `systemHash`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Hashed")]
    pub body_hash: Option<Hashed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "modelInput"))]
    pub messages_delta: Option<Content<Body>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "modelOutput"))]
    pub output: Option<Content<Body>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "modelInput"))]
    pub system: Option<Content<Body>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "modelInput"))]
    pub tools: Option<Content<Body>>,
}
approx_struct!(AgentTurn {
    turn_id,
    run_id,
    seq,
    parent_turn_id,
    harness,
    provider,
    model,
    model_hash,
    weights,
    upstream,
    upstream_hash,
    input_tokens,
    output_tokens,
    cache_read_tokens,
    cache_write_tokens,
    reasoning_tokens,
    latency_ms,
    ttft_ms,
    stop_reason,
    cost_micros,
    cost_basis,
    capture,
    system_hash,
    tools_hash,
    body_hash,
    messages_delta,
    output,
    system,
    tools,
});

/// `agent.toolCall`: the agent called a tool. `callId` is the `FIN_CALL` its `fin` run
/// carries; `command` names the `fin` command when the tool ran `fin`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCall {
    pub call_id: IdRef,
    pub turn_id: IdRef,
    #[serde(deserialize_with = "names::agent_tool")]
    pub tool: Name,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "names::cli_command_opt"
    )]
    #[schemars(with = "Name")]
    pub command: Option<Name>,
    pub args_hash: Hashed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "toolArgs"))]
    pub args: Option<Content<Body>>,
}
approx_struct!(AgentToolCall {
    call_id,
    turn_id,
    tool,
    command,
    args_hash,
    args
});

/// How a tool result's txIds were joined to its call (§12.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Attribution {
    Exact,
    Inferred,
    Ambiguous,
    None,
}

/// `agent.toolResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
    pub call_id: IdRef,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "i32")]
    pub exit: Option<i32>,
    pub micros: u64,
    // The commits the call made (at most `MAX_TOOL_TX_IDS`).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "capped_tx_ids"
    )]
    #[schemars(length(max = 64))]
    pub tx_ids: Vec<IdRef>,
    pub attribution: Attribution,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "toolResult"))]
    pub output: Option<Content<Body>>,
}
approx_struct!(AgentToolResult {
    call_id,
    ok,
    code,
    exit,
    micros,
    tx_ids,
    attribution,
    truncated,
    output
});

fn capped_tx_ids<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<IdRef>, D::Error> {
    let ids = Vec::<IdRef>::deserialize(d)?;
    if ids.len() > MAX_TOOL_TX_IDS {
        return Err(serde::de::Error::custom(format!(
            "txIds holds {} ids, at most {MAX_TOOL_TX_IDS}",
            ids.len()
        )));
    }
    Ok(ids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalKind {
    Rule,
    Budget,
    Provider,
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    Allow,
    Deny,
    Expired,
}

/// Who decided an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DecidedBy {
    User,
    Policy,
}

/// `agent.approval`: an approval was answered or expired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentApproval {
    pub approval_id: IdRef,
    pub session_id: IdRef,
    pub kind: ApprovalKind,
    pub decision: ApprovalDecision,
    pub by: DecidedBy,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "userText"))]
    pub subject: Option<Content<Body>>,
}
approx_struct!(AgentApproval {
    approval_id,
    session_id,
    kind,
    decision,
    by,
    latency_ms,
    subject
});
