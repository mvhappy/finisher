//! Process → host messages (Decisions 3, 5, 5a, 7, 8). On the wire each is
//! `{"v":1,"t":"<type>","seq":n, …}` ([`crate::codec`]); `hello` carries the major `v` itself.

use std::borrow::Cow;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ItemError, JobError};
use crate::msg_host::HelperKind;

/// A message from a worker or a helper to its host. Any message counts as alive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "t", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WorkerMsg {
    /// The first message of every process.
    Hello(Hello),
    /// An item starts (batch kinds).
    Begin {
        job: String,
        item: String,
    },
    /// An item ended: with `error`, it failed (recorded in `itemErrors`) and the job goes on.
    Item {
        job: String,
        item: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Value")]
        value: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "JobError")]
        error: Option<JobError>,
    },
    Progress(Progress),
    /// A resumable job's state, ≤ 256 KiB of JSON.
    Checkpoint {
        job: String,
        data: Value,
    },
    /// Sent every `limits.aliveMs` from the runtime's own thread.
    Alive {},
    Result {
        job: String,
        value: Value,
    },
    Error {
        job: String,
        error: JobError,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Vec<ItemError>")]
        item_errors: Option<Vec<ItemError>>,
    },
    /// A warm worker is free again.
    Ready {},
    /// A diagnostics payload of one of the family's `diagKinds` (Decision 7).
    Diag {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        job: Option<String>,
        kind: String,
        payload: Value,
    },
    /// A host call (Decision 5a); answered by `reply {id}`.
    Call {
        id: u64,
        method: String,
        params: Value,
    },
}

impl WorkerMsg {
    /// The wire type (`t`).
    pub fn type_name(&self) -> &'static str {
        match self {
            WorkerMsg::Hello(_) => "hello",
            WorkerMsg::Begin { .. } => "begin",
            WorkerMsg::Item { .. } => "item",
            WorkerMsg::Progress(_) => "progress",
            WorkerMsg::Checkpoint { .. } => "checkpoint",
            WorkerMsg::Alive {} => "alive",
            WorkerMsg::Result { .. } => "result",
            WorkerMsg::Error { .. } => "error",
            WorkerMsg::Ready {} => "ready",
            WorkerMsg::Diag { .. } => "diag",
            WorkerMsg::Call { .. } => "call",
        }
    }

    /// The job a message names, if any (the host refuses another job's, Decision 4).
    pub fn job(&self) -> Option<&str> {
        match self {
            WorkerMsg::Begin { job, .. }
            | WorkerMsg::Item { job, .. }
            | WorkerMsg::Checkpoint { job, .. }
            | WorkerMsg::Result { job, .. }
            | WorkerMsg::Error { job, .. } => Some(job),
            WorkerMsg::Progress(p) => Some(&p.job),
            WorkerMsg::Diag { job, .. } => job.as_deref(),
            WorkerMsg::Hello(_)
            | WorkerMsg::Alive {}
            | WorkerMsg::Ready {}
            | WorkerMsg::Call { .. } => None,
        }
    }

    /// Every wire type, for the codec.
    pub const TYPES: &'static [&'static str] = &[
        "hello",
        "begin",
        "item",
        "progress",
        "checkpoint",
        "alive",
        "result",
        "error",
        "ready",
        "diag",
        "call",
    ];
}

/// What a process is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "HelloRole")]
pub enum Role {
    Worker,
    Helper,
}

/// A kind at a version that a worker serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KindVersion {
    pub kind: Cow<'static, str>,
    pub version: u32,
}

impl KindVersion {
    /// For a family's `static` kind list.
    pub const fn new(kind: &'static str, version: u32) -> KindVersion {
        KindVersion {
            kind: Cow::Borrowed(kind),
            version,
        }
    }
}

/// `hello` (Decisions 3, 8). A worker names its `family`, `kinds` and `warm`; a helper its
/// helper `kind` and `id`. `envelope` is [`crate::envelope_id`] (bundled processes must send
/// it; plugin processes may omit it); `selfSandbox` the profile applied (Decision 9a).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// The protocol major ([`crate::PROTOCOL`]).
    pub v: u32,
    /// The protocol minor ([`crate::MINOR`]).
    pub minor: u32,
    pub role: Role,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub family: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<KindVersion>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub warm: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub envelope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub self_sandbox: Option<String>,
    pub pid: u32,
    // A helper's kind (helpers only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "HelperKind")]
    pub kind: Option<HelperKind>,
    /// A helper's id (helpers only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub id: Option<String>,
}

/// `progress`. The runtime fills `job` ([`crate::worker::JobCtx::progress`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub job: String,
    pub done: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "f64")]
    pub total: Option<f64>,
    pub unit: ProgressUnit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub phase: Option<String>,
}

impl Progress {
    /// `done` of `unit`, no total and no phase; `job` is filled when it is sent.
    pub fn new(done: f64, unit: ProgressUnit) -> Progress {
        Progress {
            job: String::new(),
            done,
            total: None,
            unit,
            phase: None,
        }
    }

    pub fn of(mut self, total: f64) -> Progress {
        self.total = Some(total);
        self
    }

    pub fn phase(mut self, phase: impl Into<String>) -> Progress {
        self.phase = Some(phase.into());
        self
    }
}

/// The unit of a progress figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProgressUnit {
    Bytes,
    Items,
    Frames,
    Seconds,
}
