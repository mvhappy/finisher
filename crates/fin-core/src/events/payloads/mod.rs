//! The payload of every kind (plan D, Decisions 22–28), one module per domain, and the shared
//! shapes: [`Counts`], [`TxIntent`] (from `intent.rs`) and [`Body`]. Every payload field is classed by its type
//! (`class.rs`); content fields are optional, so a redacted payload still decodes.
//!
//! Serde conventions, as in the rest of `fin-core`: camelCase; optional fields are omitted
//! when absent (never `null`) and carry `#[schemars(with = ...)]` so the schema names their
//! type; empty lists and maps are omitted too.

use std::borrow::Cow;
use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::approx::{ApproxBytes, approx_fixed};
use super::class::BlobRef;
pub use super::intent::{IntentTx, TxIntent};

pub mod agent;
pub mod auth;
pub mod cli;
pub mod consent;
pub mod errors;
pub mod fonts;
pub mod jobs;
pub mod journal;
pub mod library;
pub mod log;
pub mod media;
pub mod plugin;
pub mod reserved;
pub mod sandbox;
pub mod session;
pub mod stats;
pub mod storage;

pub use agent::{
    AgentApproval, AgentMode, AgentSession, AgentToolCall, AgentToolResult, AgentTurn,
    ApprovalDecision, ApprovalKind, Attribution, Capture, CostBasis, DecidedBy, Harness,
    MAX_TOOL_TX_IDS, Provenance, SessionPhase, SessionTotals, SessionTrigger, SignIn, StopReason,
    Weights,
};
pub use auth::{AuthPair, PairOutcome, PairPlatform, PairRoute};
pub use cli::CliCommand;
pub use consent::ConsentChanged;
pub use errors::{ErrorInternal, ErrorPanic, PanicThread, PanicWhere};
pub use fonts::{FetchOutcome, FontsFetch};
pub use jobs::{
    ExitReason, JobDone, JobOutcome, JobPriority, ProcessHelperKind, ProcessRole, WorkerExit,
};
pub use journal::JournalDropped;
pub use library::{LibraryScan, LibraryStorageName, ScanTrigger};
pub use log::{CommitOutcome, CommitRecord, IntentOmitted, LogCommit};
pub use media::{
    AdapterType, BackendEntry, CodecEntry, CopyPath, GpuEntry, JobMedia, MediaCapabilitiesEvent,
    RenderPhase, RenderStats, ResolutionBucket, Sandbox, SupportClass,
};
pub use plugin::{
    PluginKindName, PluginLifecycle, PluginOutcome, PluginRef, PluginSourceName, RegistryRefresh,
    RiskName, log2_bucket,
};
pub use reserved::{PreviewStats, RenderSample, ReviewVerdict, UiAction};
pub use sandbox::{SandboxOs, SandboxProfileName, SandboxUnconfined, UnconfinedReason};
pub use session::{Arch, BuildProfile, OsFamily, SessionEnd, SessionStart, SessionUnclean};
pub use stats::{EvalStats, Histogram, RpcStats};
pub use storage::{StorageMode, StorageState};

/// Counts by a classed key (op kinds, error codes): a JSON object whose keys are `K`. The
/// schema says so with `propertyNames`, so the class check sees the keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Counts<K: Ord>(pub BTreeMap<K, u64>);

impl<K: Ord> Default for Counts<K> {
    fn default() -> Self {
        Counts(BTreeMap::new())
    }
}

impl<K: Ord> Counts<K> {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn add(&mut self, key: K, n: u64) {
        *self.0.entry(key).or_default() += n;
    }
}

impl<K: Ord + JsonSchema> JsonSchema for Counts<K> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("Counts_{}", K::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let key = generator.subschema_for::<K>();
        json_schema!({
            "type": "object",
            "propertyNames": key,
            "additionalProperties": { "type": "integer", "format": "uint64", "minimum": 0 }
        })
    }
}

impl<K: Ord + ApproxBytes> ApproxBytes for Counts<K> {
    fn approx_bytes(&self) -> usize {
        self.0.approx_bytes()
    }
}

/// Large or structured content (the agent kinds): inline JSON, or a [`BlobRef`] when
/// it would not fit an entry (Decision 20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Body {
    Inline(Value),
    Blob(BlobRef),
}

impl ApproxBytes for Body {
    fn approx_bytes(&self) -> usize {
        12 + match self {
            Body::Inline(v) => v.approx_bytes(),
            Body::Blob(b) => b.approx_bytes(),
        }
    }
}

impl Body {
    pub fn blob(&self) -> Option<&BlobRef> {
        match self {
            Body::Blob(b) => Some(b),
            Body::Inline(_) => None,
        }
    }
}

approx_fixed!(10 =>
    super::Basis, super::Source, super::Purposes,
    AgentMode, Harness, SessionPhase, SessionTrigger, Weights, SignIn, StopReason, CostBasis,
    Capture, Attribution, ApprovalKind, ApprovalDecision, DecidedBy,
    PanicThread, PanicWhere, FetchOutcome, CommitOutcome, CommitRecord, IntentOmitted,
    Arch, BuildProfile, OsFamily, StorageMode,
    JobOutcome, JobPriority, ProcessRole, ProcessHelperKind, ExitReason,
    LibraryStorageName, ScanTrigger,
    PluginOutcome, PluginSourceName, PluginKindName, RiskName,
    SandboxProfileName, SandboxOs, UnconfinedReason,
    PairOutcome, PairPlatform, PairRoute,
);

/// The [`BlobRef`]s of each content [`Body`] given, for `AnyPayload::blobs`.
pub(crate) fn body_blobs<'a>(bodies: &[Option<&'a super::Content<Body>>]) -> Vec<&'a BlobRef> {
    bodies.iter().flatten().filter_map(|b| b.0.blob()).collect()
}
