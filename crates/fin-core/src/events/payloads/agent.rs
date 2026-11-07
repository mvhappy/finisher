//! The reserved agent kinds (Decision 28; version 0, no emitter until sub-project 7, which
//! may change them without a version bump and then sets version 1). `agent.turn`,
//! `agent.toolCall` and `agent.toolResult` are training-only: under diagnostics they keep
//! their metadata and drop their content.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Body;
use crate::events::approx::approx_struct;
use crate::events::class::{Code, Content, Hashed, IdRef, Name};
use crate::events::names;

/// `agent.session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    #[serde(deserialize_with = "names::agent_provider")]
    pub provider: Name,
    #[serde(deserialize_with = "names::agent_model")]
    pub model: Name,
    pub role: AgentRole,
    pub mode: AgentMode,
}
approx_struct!(AgentSession {
    provider,
    model,
    role,
    mode
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AgentRole {
    Main,
    Subagent,
    Critic,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AgentMode {
    Interactive,
    Headless,
    Other,
}

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

/// `agent.turn`: one model call. Content: the turn's new messages and the output; the system
/// prompt and the tool definitions by hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurn {
    pub turn_id: IdRef,
    #[serde(deserialize_with = "names::agent_provider")]
    pub provider: Name,
    #[serde(deserialize_with = "names::agent_model")]
    pub model: Name,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub latency_ms: u64,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub cost_micros: Option<u64>,
    pub system_hash: Hashed,
    pub tools_hash: Hashed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "modelInput"))]
    pub messages: Option<Content<Body>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "modelOutput"))]
    pub output: Option<Content<Body>>,
}
approx_struct!(AgentTurn {
    turn_id,
    provider,
    model,
    input_tokens,
    output_tokens,
    cache_read_tokens,
    cache_write_tokens,
    latency_ms,
    stop_reason,
    cost_micros,
    system_hash,
    tools_hash,
    messages,
    output,
});

/// `agent.toolCall`: the agent ran a command (`callId` is the `FIN_CALL` its `fin` run
/// carries).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCall {
    pub call_id: IdRef,
    pub turn_id: IdRef,
    #[serde(deserialize_with = "names::cli_command")]
    pub command: Name,
    pub args_hash: Hashed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "toolArgs"))]
    pub args: Option<Content<Body>>,
}
approx_struct!(AgentToolCall {
    call_id,
    turn_id,
    command,
    args_hash,
    args
});

/// `agent.toolResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
    pub call_id: IdRef,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
    pub micros: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tx_ids: Vec<IdRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "toolResult"))]
    pub output: Option<Content<Body>>,
}
approx_struct!(AgentToolResult {
    call_id,
    ok,
    code,
    micros,
    tx_ids,
    output
});

/// `agent.editVerdict`: what became of an agent's edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentEditVerdict {
    pub tx_ids: Vec<IdRef>,
    pub verdict: Verdict,
    pub by: VerdictBy,
}
approx_struct!(AgentEditVerdict {
    tx_ids,
    verdict,
    by
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Accepted,
    Rejected,
    Undone,
    Merged,
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum VerdictBy {
    User,
    Critic,
    Agent,
}
