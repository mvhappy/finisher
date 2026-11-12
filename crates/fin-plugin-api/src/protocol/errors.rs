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
    /// A cold kind's output file that fails its checks (`{reason: link|notRegular|linked|size|truncated|schema}`).
    pub const PLUGIN_OUTPUT_INVALID: &str = "PLUGIN_OUTPUT_INVALID";
    pub const PLUGIN_PERMISSION_DENIED: &str = "PLUGIN_PERMISSION_DENIED";
    pub const EGRESS_DENIED: &str = "EGRESS_DENIED";
    pub const EGRESS_NOT_HTTPS: &str = "EGRESS_NOT_HTTPS";
    pub const EGRESS_PRIVATE_ADDRESS: &str = "EGRESS_PRIVATE_ADDRESS";
    pub const SECRET_MISSING: &str = "SECRET_MISSING";
    /// A request body over 768,000 bytes, a response over 64 MiB, or a job over its held
    /// bytes (`{what, limit}`; sp8b Task 8).
    pub const EGRESS_TOO_LARGE: &str = "EGRESS_TOO_LARGE";
    /// The 120 s cap of a `net.fetch` call (`{limitS}`).
    pub const EGRESS_TIMEOUT: &str = "EGRESS_TIMEOUT";
    /// The plugin's rate or in-flight cap, or the server's (`{reason, limit, retryable}`).
    pub const EGRESS_RATE_LIMITED: &str = "EGRESS_RATE_LIMITED";
    /// A name that does not resolve or an origin that does not answer (`{origin, reason}`).
    pub const EGRESS_UNREACHABLE: &str = "EGRESS_UNREACHABLE";
}
