//! The agent kinds' wire forms (sub-project 7 §12.2; review H1, H2 and M3): what `finisherd`
//! sends to the journal, and the pure conversion to the stored payload.
//!
//! - **Model ids** arrive raw. One `AGENT_MODELS` names is stored as that name; any other is
//!   stored as `other` with `modelHash`, the keyed hash of the id. OpenRouter takes any id the
//!   user types, so most are hashed; none is ever stored raw.
//! - **Upstreams** arrive as OpenRouter reports them (`Moonshot AI`, `DeepInfra`): a known one
//!   becomes its `AGENT_UPSTREAMS` slug ([`names::agent_upstream`]), any other `upstreamHash`.
//! - **Hashes** arrive unkeyed (`blake3:<hex>`, as `fin`'s `argsHash`): `systemHash`,
//!   `toolsHash`, `bodyHash` and `argsHash` are stored keyed, so a hash of a prompt cannot be
//!   checked against a guess without the install key.
//! - **txIds** arrive raw (an idempotency key need not be a ULID) and become `IdRef`s.
//! - **Provenance** on a tool call, a tool result and an approval (the agent-trace store's
//!   copies) arrives with its model id raw, as on a session.
//! - **`bodyBlake3`** (the agent-trace store's turns) is the one hash stored unkeyed: it is
//!   content, beside the body it hashes, so it tells a reader nothing the body does not.

use serde::{Deserialize, Deserializer, Serialize};

use super::class::{Blake3Hex, Code, Content, Hashed, IdRef, Name, RawId, ScrubCtx};
use super::names::{self, AGENT_MODELS};
use super::payloads::Body;
use super::payloads::agent::{
    AgentApproval, AgentMode, AgentSession, AgentToolCall, AgentToolResult, AgentTurn,
    ApprovalDecision, ApprovalKind, Attribution, Capture, CostBasis, DecidedBy, Harness,
    MAX_TOOL_TX_IDS, Provenance, SessionPhase, SessionTotals, SessionTrigger, SignIn, StopReason,
    TurnPurpose, Weights,
};
use super::wire::WireForm;

type Key<'a> = &'a dyn Fn(&[u8]) -> [u8; 16];

/// `model` and `modelHash` of a raw model id.
fn model_of(raw: &RawId, key: Key<'_>) -> (Name, Option<Hashed>) {
    match AGENT_MODELS.get(raw.as_str()).filter(|n| *n != Name::OTHER) {
        Some(n) => (n, None),
        None => (
            Name::OTHER,
            Some(Hashed::keyed(raw.as_str().as_bytes(), key)),
        ),
    }
}

/// `upstream` and `upstreamHash` of an upstream as reported.
fn upstream_of(raw: Option<&RawId>, key: Key<'_>) -> (Option<Name>, Option<Hashed>) {
    match raw {
        None => (None, None),
        Some(r) => match names::agent_upstream(r.as_str()) {
            Some(n) => (Some(n), None),
            None => (None, Some(Hashed::keyed(r.as_str().as_bytes(), key))),
        },
    }
}

fn keyed(h: &Blake3Hex, key: Key<'_>) -> Hashed {
    Hashed::keyed(h.as_str().as_bytes(), key)
}

/// [`Provenance`] as `finisherd` sends it: the model id raw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceIn {
    pub harness: Harness,
    #[serde(deserialize_with = "names::agent_provider")]
    pub provider: Name,
    pub model: RawId,
    pub weights: Weights,
}

impl ProvenanceIn {
    fn into_stored(self, key: Key<'_>) -> Provenance {
        let (model, model_hash) = model_of(&self.model, key);
        Provenance {
            harness: self.harness,
            provider: self.provider,
            model,
            model_hash,
            weights: self.weights,
        }
    }
}

/// `agent.session` as `finisherd` sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionIn {
    pub session_id: IdRef,
    pub phase: SessionPhase,
    pub harness: Harness,
    #[serde(deserialize_with = "names::agent_provider")]
    pub provider: Name,
    /// The model id as used (raw).
    pub model: RawId,
    pub weights: Weights,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<SignIn>,
    #[serde(deserialize_with = "names::agent_role")]
    pub role: Name,
    pub mode: AgentMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<IdRef>,
    pub trigger: SessionTrigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Code>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totals: Option<SessionTotals>,
}

impl WireForm for AgentSessionIn {
    type Payload = AgentSession;

    fn into_payload(self, key: Key<'_>, _ctx: &ScrubCtx) -> AgentSession {
        let (model, model_hash) = model_of(&self.model, key);
        AgentSession {
            session_id: self.session_id,
            phase: self.phase,
            harness: self.harness,
            provider: self.provider,
            model,
            model_hash,
            weights: self.weights,
            sign_in: self.sign_in,
            role: self.role,
            mode: self.mode,
            parent: self.parent,
            trigger: self.trigger,
            reason: self.reason,
            totals: self.totals,
        }
    }
}

/// `agent.turn` as `finisherd` sends it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnIn {
    pub turn_id: IdRef,
    pub run_id: IdRef,
    pub seq: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_turn_id: Option<IdRef>,
    pub harness: Harness,
    #[serde(deserialize_with = "names::agent_provider")]
    pub provider: Name,
    pub model: RawId,
    pub weights: Weights,
    /// As OpenRouter reported it (a display name or a slug).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<RawId>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micros: Option<u64>,
    pub cost_basis: CostBasis,
    pub capture: Capture,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<TurnPurpose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_hash: Option<Blake3Hex>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_hash: Option<Blake3Hex>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_hash: Option<Blake3Hex>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_blake3: Option<Content<Blake3Hex>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_delta: Option<Content<Body>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Content<Body>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<Content<Body>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Content<Body>>,
}

impl WireForm for AgentTurnIn {
    type Payload = AgentTurn;

    fn into_payload(self, key: Key<'_>, _ctx: &ScrubCtx) -> AgentTurn {
        let (model, model_hash) = model_of(&self.model, key);
        let (upstream, upstream_hash) = upstream_of(self.upstream.as_ref(), key);
        AgentTurn {
            turn_id: self.turn_id,
            run_id: self.run_id,
            seq: self.seq,
            parent_turn_id: self.parent_turn_id,
            harness: self.harness,
            provider: self.provider,
            model,
            model_hash,
            weights: self.weights,
            upstream,
            upstream_hash,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            reasoning_tokens: self.reasoning_tokens,
            latency_ms: self.latency_ms,
            ttft_ms: self.ttft_ms,
            stop_reason: self.stop_reason,
            cost_micros: self.cost_micros,
            cost_basis: self.cost_basis,
            capture: self.capture,
            purpose: self.purpose,
            system_hash: self.system_hash.as_ref().map(|h| keyed(h, key)),
            tools_hash: self.tools_hash.as_ref().map(|h| keyed(h, key)),
            body_hash: self.body_hash.as_ref().map(|h| keyed(h, key)),
            body_blake3: self.body_blake3,
            messages_delta: self.messages_delta,
            output: self.output,
            system: self.system,
            tools: self.tools,
        }
    }
}

/// `agent.toolCall` as `finisherd` sends it: `argsHash` unkeyed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCallIn {
    pub call_id: IdRef,
    pub turn_id: IdRef,
    #[serde(deserialize_with = "names::agent_tool")]
    pub tool: Name,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "names::cli_command_opt"
    )]
    pub command: Option<Name>,
    pub args_hash: Blake3Hex,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ProvenanceIn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Content<Body>>,
}

impl WireForm for AgentToolCallIn {
    type Payload = AgentToolCall;

    fn into_payload(self, key: Key<'_>, _ctx: &ScrubCtx) -> AgentToolCall {
        AgentToolCall {
            call_id: self.call_id,
            turn_id: self.turn_id,
            tool: self.tool,
            command: self.command,
            args_hash: keyed(&self.args_hash, key),
            provenance: self.provenance.map(|p| p.into_stored(key)),
            args: self.args,
        }
    }
}

/// `agent.toolResult` as `finisherd` sends it: txIds raw (at most 64).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResultIn {
    pub call_id: IdRef,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Code>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    pub micros: u64,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "capped_raw_ids"
    )]
    pub tx_ids: Vec<RawId>,
    pub attribution: Attribution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ProvenanceIn>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Content<Body>>,
}

fn capped_raw_ids<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<RawId>, D::Error> {
    let ids = Vec::<RawId>::deserialize(d)?;
    if ids.len() > MAX_TOOL_TX_IDS {
        return Err(serde::de::Error::custom(format!(
            "txIds holds {} ids, at most {MAX_TOOL_TX_IDS}",
            ids.len()
        )));
    }
    Ok(ids)
}

impl WireForm for AgentToolResultIn {
    type Payload = AgentToolResult;

    fn into_payload(self, key: Key<'_>, _ctx: &ScrubCtx) -> AgentToolResult {
        AgentToolResult {
            call_id: self.call_id,
            ok: self.ok,
            code: self.code,
            exit: self.exit,
            micros: self.micros,
            tx_ids: self
                .tx_ids
                .into_iter()
                .map(|id| id.into_id_ref(key))
                .collect(),
            attribution: self.attribution,
            provenance: self.provenance.map(|p| p.into_stored(key)),
            truncated: self.truncated,
            output: self.output,
        }
    }
}

/// `agent.approval` as `finisherd` sends it: the provenance's model id raw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentApprovalIn {
    pub approval_id: IdRef,
    pub session_id: IdRef,
    pub kind: ApprovalKind,
    pub decision: ApprovalDecision,
    pub by: DecidedBy,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ProvenanceIn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Content<Body>>,
}

impl WireForm for AgentApprovalIn {
    type Payload = AgentApproval;

    fn into_payload(self, key: Key<'_>, _ctx: &ScrubCtx) -> AgentApproval {
        AgentApproval {
            approval_id: self.approval_id,
            session_id: self.session_id,
            kind: self.kind,
            decision: self.decision,
            by: self.by,
            latency_ms: self.latency_ms,
            provenance: self.provenance.map(|p| p.into_stored(key)),
            subject: self.subject,
        }
    }
}
