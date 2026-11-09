//! The manifest, `finisher-plugin.json` at a package's root (spec §4.1, 8A Decisions 1–3).
//! Every struct refuses unknown fields, so a typo never drops a permission.

mod contributes;
mod id;
mod runtime;

use schemars::JsonSchema;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

pub use contributes::{CaptionStyleContribution, Contributes, FontCatalogContribution};
pub use id::{DEV_PUBLISHER, IdError, PluginId, RESERVED_DEVICE_NAMES, RESERVED_PUBLISHERS};
pub use runtime::{Platform, Runtime};

use crate::permission::Permission;

/// The manifest's file name at the package root.
pub const MANIFEST_FILE: &str = "finisher-plugin.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PluginKind {
    /// Data only, validated in Rust by the batch that consumes it.
    Content,
    /// OS processes under the `plugin` sandbox profile (8B).
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Publisher {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A model a code plugin needs (spec §8.2); never inside the package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelRef {
    pub id: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    /// A message key in the `en` catalogue.
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Always 1.
    pub manifest_version: u16,
    pub id: PluginId,
    /// Semver 2.0 without build metadata, immutable once published.
    #[schemars(with = "String")]
    pub version: Version,
    pub kind: PluginKind,
    /// The host API range this plugin works with (`^1.0`).
    #[schemars(with = "String")]
    pub host_api: VersionReq,
    /// An SPDX expression.
    pub license: String,
    pub publisher: Publisher,
    /// The package directory holding `<locale>.json` ICU catalogues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locales: Vec<String>,
    /// Code plugins only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<Platform>,
    /// Code plugins only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Runtime>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<Permission>,
    #[serde(default, skip_serializing_if = "Contributes::is_empty")]
    pub contributes: Contributes,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelRef>,
}

impl Manifest {
    /// The package paths that get mode 0755 when unpacked: a native runtime's executables.
    pub fn executables(&self) -> Vec<String> {
        let mut v: Vec<String> = match &self.runtime {
            Some(Runtime::Native { exec }) => exec.values().cloned().collect(),
            _ => Vec::new(),
        };
        v.sort();
        v.dedup();
        v
    }

    /// Whether any contribution steers agents (spec §4.2 medium risk; never auto-updated).
    pub fn agent_facing(&self) -> bool {
        self.contributes.kinds().iter().any(|k| k.agent_facing())
    }
}
