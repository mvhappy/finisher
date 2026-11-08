//! Signed advisories (spec §6.5): per publisher in `<publisher>/advisories.json` (signed by
//! the publisher's role) and registry-wide in `advisories.json` (signed by top-level
//! `targets`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AdvisoryAction {
    /// Shown with the plugin; nothing changes.
    Warn,
    /// The plugin is disabled on refresh by any principal (`PLUGIN_REVOKED`).
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// An advisory with the role that signed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignedAdvisory {
    /// `targets` for registry-wide advisories, else the publisher.
    pub signed_by: String,
    #[serde(flatten)]
    pub advisory: Advisory,
}

impl SignedAdvisory {
    /// Whether this advisory may speak about `plugin`: registry-wide ones about any plugin,
    /// a publisher's only about its own (spec §6.5; `@a` never vouches for `@b`).
    pub fn in_scope(&self) -> bool {
        if self.signed_by == "targets" {
            return fin_plugin_api::PluginId::parse(&self.advisory.plugin).is_ok();
        }
        fin_plugin_api::PluginId::parse(&self.advisory.plugin)
            .is_ok_and(|id| id.publisher() == self.signed_by)
    }

    /// Whether it applies to `plugin`@`version`. A range that does not parse matches nothing.
    pub fn applies_to(&self, plugin: &str, version: &semver::Version) -> bool {
        self.in_scope()
            && self.advisory.plugin == plugin
            && semver::VersionReq::parse(&self.advisory.versions)
                .is_ok_and(|req| req.matches(version))
    }
}

/// The in-scope advisories that apply to `plugin`@`version`.
pub fn applicable<'a>(
    all: &'a [SignedAdvisory],
    plugin: &str,
    version: &semver::Version,
) -> Vec<&'a SignedAdvisory> {
    all.iter()
        .filter(|a| a.applies_to(plugin, version))
        .collect()
}
