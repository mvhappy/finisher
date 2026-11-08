//! Errors of plans and the store as `{code, message, details, fix?}` (spec §13). Package errors
//! ([`crate::package::PackageError`]) convert into them unchanged.

use std::fmt;
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::package::PackageError;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RegistryError {
    pub code: String,
    pub message: String,
    pub details: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl RegistryError {
    pub fn new(code: &str, message: impl Into<String>, details: Value) -> RegistryError {
        let details = match details {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        RegistryError {
            code: code.into(),
            message: message.into(),
            details,
            fix: None,
        }
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> RegistryError {
        self.fix = Some(fix.into());
        self
    }

    pub(crate) fn io(op: &str, path: &Path, e: impl fmt::Display) -> RegistryError {
        RegistryError::new(
            "IO_ERROR",
            format!("{op} {}: {e}", path.display()),
            json!({ "op": op, "path": path.display().to_string() }),
        )
    }

    /// `PLAN_MISMATCH {reason}`: the staged plan or its tree changed since it was made.
    pub(crate) fn plan_mismatch(reason: &str, message: impl Into<String>) -> RegistryError {
        RegistryError::new("PLAN_MISMATCH", message, json!({ "reason": reason }))
            .with_fix("make a new plan")
    }

    /// `details.reason`, when the code has one.
    pub fn reason(&self) -> Option<&str> {
        self.details.get("reason").and_then(Value::as_str)
    }
}

impl From<PackageError> for RegistryError {
    fn from(e: PackageError) -> RegistryError {
        RegistryError {
            code: e.code.to_string(),
            message: e.message,
            details: e.details,
            fix: None,
        }
    }
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RegistryError {}
