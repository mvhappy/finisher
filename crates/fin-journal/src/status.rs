//! The journal's public value types: the consent it records under, its status and errors.

use std::fmt;

use fin_core::CoreError;
use fin_core::events::{Basis, Purposes};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The effective consent (D3's `resolve` makes it): the purposes in force, the consent
/// revision and the weaker basis of the two purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent {
    pub purposes: Purposes,
    pub rev: u64,
    pub basis: Basis,
}

impl Consent {
    /// Nothing recorded.
    pub const OFF: Consent = Consent {
        purposes: Purposes::NONE,
        rev: 0,
        basis: Basis::Default,
    };
}

/// A high-rate timing aggregated into per-window histograms (Decision 19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Metric {
    /// `rpc.stats`, keyed by `RPC_METHODS`.
    Rpc,
    /// `eval.stats`, keyed by `EVAL_KEYS`.
    Eval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JournalState {
    Off,
    On,
    Suspended,
}

/// What the journal holds and whether it is writing (`fin diagnostics show`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JournalStatus {
    pub state: JournalState,
    // Why it is suspended: `JOURNAL_UNWRITABLE`, `JOURNAL_DISK_FULL`, `JOURNAL_IO` or
    // `JOURNAL_PANIC`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub code: Option<String>,
    pub files: u64,
    pub bytes: u64,
    /// Entries dropped since the journal opened.
    pub dropped: u64,
    /// The oldest entry's `ts` and the newest's (ms since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub oldest: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub newest: Option<u64>,
}

/// A journal call that could not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalError {
    pub code: &'static str,
    pub message: String,
}

impl JournalError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> JournalError {
        JournalError {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for JournalError {}

impl From<JournalError> for CoreError {
    fn from(e: JournalError) -> CoreError {
        CoreError::new(e.code, e.message)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    pub(crate) suspended: Option<&'static str>,
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) oldest: Option<u64>,
    pub(crate) newest: Option<u64>,
}
