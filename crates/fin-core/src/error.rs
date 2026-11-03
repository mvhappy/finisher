//! The one error shape used everywhere: `{ code, message, details, fix? }` (spec §19).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A structured, machine-readable error. `code` is a stable `SCREAMING_SNAKE` string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{code}: {message}")]
pub struct CoreError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl CoreError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        CoreError { code: code.to_string(), message: message.into(), details: json!({}), fix: None }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}
