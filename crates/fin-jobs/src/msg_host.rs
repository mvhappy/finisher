//! Host → process messages (Decisions 5, 5a, 8): `start`, `cancel`, `shutdown`, `attach`,
//! `reply`. On the wire each is `{"v":1,"t":"<type>","seq":n, …}` ([`crate::codec`]).

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::JobError;

/// A message from the host to a worker or a helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "t", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HostMsg {
    /// Run a job (workers).
    Start(Start),
    /// Stop the job: `cancel` gives up, `pause` checkpoints first (resumable families).
    Cancel { job: String, reason: CancelReason },
    /// Exit 0 (sent only to an idle worker, or to a helper).
    Shutdown {},
    /// A helper's job and files (helpers).
    Attach(Attach),
    /// The answer to a `call {id}` (Decision 5a): `value` or `error`.
    Reply {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Value")]
        value: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "JobError")]
        error: Option<JobError>,
    },
}

impl HostMsg {
    /// The wire type (`t`).
    pub fn type_name(&self) -> &'static str {
        match self {
            HostMsg::Start(_) => "start",
            HostMsg::Cancel { .. } => "cancel",
            HostMsg::Shutdown {} => "shutdown",
            HostMsg::Attach(_) => "attach",
            HostMsg::Reply { .. } => "reply",
        }
    }

    /// Every wire type, for the codec.
    pub const TYPES: &'static [&'static str] = &["start", "cancel", "shutdown", "attach", "reply"];
}

/// Why a `cancel` was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CancelReason {
    Cancel,
    Pause,
}

/// `start`: the job, its limits, its mapped files, its helpers and directories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    pub job: StartJob,
    pub limits: Limits,
    #[serde(default)]
    pub files: Vec<FileGrant>,
    #[serde(default)]
    pub helpers: Vec<HelperGrant>,
    pub dirs: Dirs,
    /// Whether `diag` messages are forwarded (Decision 7).
    #[serde(default)]
    pub diagnostics: bool,
}

/// The job a `start` runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartJob {
    pub id: String,
    pub kind: String,
    pub kind_version: u32,
    pub params: Value,
    /// The last checkpoint (resumable kinds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Value")]
    pub checkpoint: Option<Value>,
    /// 1 on the first attempt.
    pub attempt: u32,
    /// What the kind's `prepare` hook returned (Decision 40).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Value")]
    pub extra: Option<Value>,
}

/// The limits a process runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// The `alive` interval (5 s).
    pub alive_ms: u32,
    /// The job's group memory limit (Decision 20), informative: the host enforces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub memory_bytes: Option<u64>,
    /// The read budget ([`crate::ReadBudget`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub read_bytes_per_sec: Option<u64>,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            alive_ms: crate::DEFAULT_ALIVE_MS,
            memory_bytes: None,
            read_bytes_per_sec: None,
        }
    }
}

/// How a mapped file may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FileMode {
    Read,
    Write,
    ReadWrite,
}

/// One mapped file (or directory, `name` ending in `/`) of the job directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileGrant {
    pub name: String,
    pub path: PathBuf,
    pub mode: FileMode,
    /// The size 5C created it at; absent for a file written by `prepare` and for directories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub bytes: Option<u64>,
}

/// A helper's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HelperKind {
    Codec,
    Plugin,
}

/// A helper started beside the worker: its id, kind and the names of the files it sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HelperGrant {
    pub id: String,
    pub kind: HelperKind,
    #[serde(default)]
    pub files: Vec<String>,
}

/// The job directory and the process's private temp directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Dirs {
    pub job: PathBuf,
    pub tmp: PathBuf,
}

/// `attach`: a helper's job, its files only, and its directories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Attach {
    pub job: String,
    #[serde(default)]
    pub files: Vec<FileGrant>,
    pub dirs: Dirs,
}
