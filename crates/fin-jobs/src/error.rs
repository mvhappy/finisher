//! A worker's error (`{code, message, details, retry}`) and an item's (Decisions 5, 31).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How the engine may retry after a worker `error` (Decision 31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Retry {
    /// Deterministic: no retry (the default).
    #[default]
    Never,
    /// Transient (I/O, an offline root): retry with backoff.
    Backoff,
    /// Retry once, at once, on a fresh process (`GPU_DEVICE_LOST`).
    Fresh,
}

impl Retry {
    fn is_never(&self) -> bool {
        *self == Retry::Never
    }
}

/// An error a worker, a helper or a host call answers: `{code, message, details, retry}`.
/// In a `reply`, `retry` is absent (it reads as `never`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "JobError")]
pub struct JobError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Value,
    #[serde(default, skip_serializing_if = "Retry::is_never")]
    pub retry: Retry,
}

impl JobError {
    /// `{code, message}` with `details: {}` and `retry: never`.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> JobError {
        JobError {
            code: code.into(),
            message: message.into(),
            details: Value::Object(Default::default()),
            retry: Retry::Never,
        }
    }

    pub fn with_details(mut self, details: Value) -> JobError {
        self.details = details;
        self
    }

    pub fn with_retry(mut self, retry: Retry) -> JobError {
        self.retry = retry;
        self
    }

    /// `JOB_CANCELLED`: what a family returns once its [`crate::worker::CancelToken`] is set.
    pub fn cancelled() -> JobError {
        JobError::new("JOB_CANCELLED", "the job was cancelled")
    }

    /// `WORKER_CRASHED` with `retry: backoff` (a caught panic, Decision 6).
    pub fn crashed(message: impl Into<String>) -> JobError {
        JobError::new("WORKER_CRASHED", message).with_retry(Retry::Backoff)
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for JobError {}

/// A failed item of a batch job (`itemErrors`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemError {
    pub item: String,
    pub error: JobError,
}
