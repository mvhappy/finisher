//! The broker's errors: `code()` is the `CoreError` code the binding answers.

use std::io;
use std::path::PathBuf;

use serde_json::{Value, json};

/// Why an operation failed: `code()` is the `CoreError` code the binding answers.
#[derive(Debug)]
pub enum BrokerError {
    /// Malformed: a relative path, a mask beyond modify, a SID or owner that is not one.
    Invalid(String),
    /// A protected root (refused before the log).
    Protected { path: PathBuf, why: &'static str },
    /// The path does not exist (refused before the log).
    NotFound(PathBuf),
    /// Another owner holds this `(path, sid)`.
    Owned { path: PathBuf, owner: String },
    /// The log or the DACL could not be read or written.
    Io(String),
}

impl BrokerError {
    pub fn code(&self) -> &'static str {
        match self {
            BrokerError::Invalid(_) => "INVALID_ARGS",
            BrokerError::Protected { .. } => "PATH_NOT_ALLOWED",
            BrokerError::NotFound(_) => "NOT_FOUND",
            BrokerError::Owned { .. } => "INVALID_OP",
            BrokerError::Io(_) => "IO_ERROR",
        }
    }

    pub fn details(&self) -> Value {
        match self {
            BrokerError::Invalid(m) => json!({ "reason": "aclInvalid", "problem": m }),
            BrokerError::Protected { path, why } => {
                json!({ "reason": "aclProtected", "path": path, "why": why })
            }
            BrokerError::NotFound(p) => json!({ "path": p }),
            BrokerError::Owned { path, owner } => {
                json!({ "reason": "aclOwned", "path": path, "owner": owner })
            }
            BrokerError::Io(m) => json!({ "reason": "aclBroker", "error": m }),
        }
    }
}

impl std::fmt::Display for BrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrokerError::Invalid(m) => write!(f, "not an ACL grant: {m}"),
            BrokerError::Protected { path, why } => {
                write!(f, "{} is protected ({why}): never granted", path.display())
            }
            BrokerError::NotFound(p) => write!(f, "{} does not exist", p.display()),
            BrokerError::Owned { path, owner } => {
                write!(f, "{} is granted to that SID for {owner}", path.display())
            }
            BrokerError::Io(m) => write!(f, "the ACL broker failed: {m}"),
        }
    }
}

pub(crate) fn io_err(what: &str, e: io::Error) -> BrokerError {
    BrokerError::Io(format!("{what}: {e}"))
}
