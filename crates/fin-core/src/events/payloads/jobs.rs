//! `job.done` and `worker.exit` (5C-1 plan, Decision 42): one per job that reaches a terminal
//! state, and one per process the job engine started (the watchdog excepted). Names come from
//! `names_jobs.rs`; a plugin id is keyed-hashed by the sink.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;
use crate::events::class::{Code, Hashed, Name};
use crate::events::names_jobs;

/// `job.done` (`kv` 1). Sub-project 6 adds its media fields as `kv` 2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobDone {
    // The job kind (`JOB_KINDS`). Plain comments on `$ref` fields: a doc comment would inline
    // the type in the generated TypeScript.
    #[serde(deserialize_with = "names_jobs::job_kind")]
    pub kind: Name,
    // The worker family (`WORKER_FAMILIES`).
    #[serde(deserialize_with = "names_jobs::worker_family")]
    pub family: Name,
    pub outcome: JobOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
    pub attempts: u32,
    pub priority: JobPriority,
    pub warm: bool,
    /// From submit to the first start.
    pub queued_ms: u64,
    /// The sum of every attempt's run time.
    pub run_micros: u64,
    /// Batch kinds: the job's item count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub items: Option<u32>,
    /// Batch kinds: the items that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub items_failed: Option<u32>,
    /// Whether isolation (bisection) ran.
    pub isolated: bool,
    /// The helper processes started over all attempts.
    pub helpers: u32,
}
approx_struct!(JobDone {
    kind,
    family,
    outcome,
    code,
    attempts,
    priority,
    warm,
    queued_ms,
    run_micros,
    items,
    items_failed,
    isolated,
    helpers
});

/// A job's terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobOutcome {
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

/// A job's priority as submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobPriority {
    Interactive,
    Normal,
    Background,
}

/// `worker.exit` (`kv` 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerExit {
    pub role: ProcessRole,
    // The worker family (`WORKER_FAMILIES`); a helper's is its job's.
    #[serde(deserialize_with = "names_jobs::worker_family")]
    pub family: Name,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ProcessHelperKind")]
    pub helper_kind: Option<ProcessHelperKind>,
    // The keyed hash of a plugin process's plugin id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Hashed")]
    pub plugin: Option<Hashed>,
    pub reason: ExitReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "i64")]
    pub exit_code: Option<i64>,
    // The terminating signal (`SIGNALS`; Unix only).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "names_jobs::signal_opt"
    )]
    #[schemars(with = "Name")]
    pub signal: Option<Name>,
    pub jobs_served: u32,
    pub lifetime_s: u64,
    pub sandboxed: bool,
}
approx_struct!(WorkerExit {
    role,
    family,
    helper_kind,
    plugin,
    reason,
    exit_code,
    signal,
    jobs_served,
    lifetime_s,
    sandboxed
});

/// What the process was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProcessRole {
    Worker,
    Helper,
}

/// A helper's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProcessHelperKind {
    Codec,
    Plugin,
}

/// Why a process ended. A closed list (`worker_exit_reasons_closed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ExitReason {
    Done,
    IdleExpired,
    Recycled,
    Cancelled,
    Killed,
    Crashed,
    Hung,
    Stalled,
    Abandoned,
    SpawnFailed,
    Oom,
    ProtocolError,
}

impl ExitReason {
    /// Every reason, in declaration order.
    pub const ALL: [ExitReason; 12] = [
        ExitReason::Done,
        ExitReason::IdleExpired,
        ExitReason::Recycled,
        ExitReason::Cancelled,
        ExitReason::Killed,
        ExitReason::Crashed,
        ExitReason::Hung,
        ExitReason::Stalled,
        ExitReason::Abandoned,
        ExitReason::SpawnFailed,
        ExitReason::Oom,
        ExitReason::ProtocolError,
    ];
}
