//! The plan's types (spec §5.4), generated into `@finisher/core-types` by `export_schema`.

use fin_plugin_api::{
    Invalid, LicenceClass, Manifest, Permission, PermissionDiff, PluginKind, Risk,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// What a person approves: made by [`super::make_plan`], kept in `staging/<planId>/plan.json`
/// beside the staged tree, committed by [`crate::store::Store::commit`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallPlan {
    pub plan_id: String,
    /// BLAKE3 (hex) of the plan's canonical JSON with `digest` empty ([`super::plan_digest`]):
    /// it binds the permissions, risks, licence and the staged tree hash.
    pub digest: String,
    pub plugin: PlanPlugin,
    pub source: PlanSource,
    pub action: PlanAction,
    /// The installed version an update or downgrade replaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Every permission of the new version with its risk, in manifest order.
    pub permissions: Vec<PlannedPermission>,
    // Against the installed version's grants (everything is `added` for a first install).
    // Not a doc comment: it would become `PermissionDiff`'s description in TypeScript.
    pub diff: PermissionDiff,
    pub risk: Risk,
    pub licence: PlanLicence,
    /// Unpacked bytes.
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<PlanModel>,
    /// `thirdPartyCode` first when present (spec §3.1), then the others.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<PlanWarning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_stale_days: Option<u16>,
    /// `package::tree_hash` of the staged tree; re-checked at commit.
    pub tree_hash: String,
    /// The signed target's SHA-256, for registry plans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sha256: Option<String>,
    /// Unix ms.
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanPlugin {
    pub id: String,
    pub version: String,
    pub kind: PluginKind,
    /// The id's name until manifests carry a display name.
    pub display_name: String,
    /// The manifest's publisher name (shown to people, never journaled).
    pub publisher: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Bundled,
    Registry,
    Dev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanSource {
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PlanAction {
    Install,
    Update,
    Downgrade,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedPermission {
    pub permission: Permission,
    /// Within the whole set (combinations count, spec §4.3).
    pub risk: Risk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanLicence {
    pub spdx: String,
    pub class: LicenceClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanModel {
    pub id: String,
    pub bytes: u64,
    pub licence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
}

/// A warning shown with the plan: `thirdPartyCode {confined}`, `yanked`, `advisory` (from the
/// registry), `lookalikeId {similar}`, `copyleft {spdx}`, `licenceUnknown {spdx}`,
/// `registryStale {days}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanWarning {
    pub code: String,
    #[serde(default)]
    pub details: Map<String, Value>,
}

impl PlanWarning {
    pub fn new(code: &str, details: Value) -> PlanWarning {
        let details = match details {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        PlanWarning {
            code: code.into(),
            details,
        }
    }
}

/// The installed version a plan is made against (from `control.db` in `finisherd`), and what a
/// commit returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledPlugin {
    pub id: String,
    pub version: String,
    pub kind: PluginKind,
    pub source: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_id: Option<String>,
    /// The granted permissions.
    #[serde(default)]
    pub permissions: Vec<Permission>,
    pub tree_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sha256: Option<String>,
    /// The package directory, relative to `<server-data>/plugins` (`/`-separated).
    pub dir: String,
}

/// What a person accepted with an approval, beyond the permissions (spec §5.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Accept {
    #[serde(default)]
    pub copyleft: bool,
    #[serde(default)]
    pub third_party_code: bool,
}

/// `validateManifest`: the manifest when valid, else every violation.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Manifest>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<Invalid>,
}
