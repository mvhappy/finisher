//! Signed advisories (spec §6.5): per publisher in `<publisher>/advisories.json` (signed by
//! the publisher's role) and registry-wide in `advisories.json` (signed by top-level
//! `targets`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdvisoryAction {
    /// Shown with the plugin; nothing changes.
    Warn,
    /// The plugin is disabled on refresh by any principal (`PLUGIN_REVOKED`).
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Advisory {
    pub id: String,
    /// The plugin id, `@publisher/name`.
    pub plugin: String,
    /// A semver range of affected versions.
    pub versions: String,
    pub severity: Severity,
    pub action: AdvisoryAction,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The content of an advisories target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvisoryFile {
    pub advisories: Vec<Advisory>,
}
