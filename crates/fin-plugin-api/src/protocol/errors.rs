//! The error a failed host call, job or command answers: `{code, message, details, fix?}`
//! (arch error shape; spec §13 codes). The SDKs map it to and from `fin-jobs`' `JobError`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl PluginError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> PluginError {
        PluginError {
            code: code.into(),
            message: message.into(),
            details: Map::new(),
            fix: None,
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> PluginError {
        self.details.insert(key.into(), value.into());
        self
    }
}

/// The codes of spec §13 that the protocol itself answers.
pub mod codes {
    /// A command's arguments do not match its schema (`{problems}`).
    pub const PLUGIN_COMMAND_ARGS: &str = "PLUGIN_COMMAND_ARGS";
    /// A returned transaction fin-core refuses (`{problems}`).
    pub const PLUGIN_TX_INVALID: &str = "PLUGIN_TX_INVALID";
    pub const PLUGIN_TIMEOUT: &str = "PLUGIN_TIMEOUT";
    pub const PLUGIN_CRASHED: &str = "PLUGIN_CRASHED";
    pub const PLUGIN_CRASH_LOOP: &str = "PLUGIN_CRASH_LOOP";
    /// A malformed message or call (the process is not killed for a call).
    pub const PLUGIN_PROTOCOL: &str = "PLUGIN_PROTOCOL";
    pub const PLUGIN_PERMISSION_DENIED: &str = "PLUGIN_PERMISSION_DENIED";
    pub const EGRESS_DENIED: &str = "EGRESS_DENIED";
    pub const EGRESS_NOT_HTTPS: &str = "EGRESS_NOT_HTTPS";
    pub const EGRESS_PRIVATE_ADDRESS: &str = "EGRESS_PRIVATE_ADDRESS";
    pub const SECRET_MISSING: &str = "SECRET_MISSING";
}
