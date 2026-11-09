//! The per-job context a plugin process receives in `start.extra` (Decision 2; spec §3.4's
//! `initialize`).
//!
//! Host → plugin payloads decode leniently: a field from a newer host is ignored (forward
//! compatibility), so none of these types denies unknown fields.

use schemars::JsonSchema;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::manifest::PluginId;
use crate::permission::Permission;

/// A plugin at one version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginRefFull {
    pub id: PluginId,
    #[schemars(with = "String")]
    pub version: Version,
}

/// Directories the plugin may use, as absolute paths on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginDirs {
    /// The installed package (read-only).
    pub plugin: String,
    /// The caller's `pluginData` directory, only when `fs pluginData` is granted (spec §5.1):
    /// `plugins/data/@pub/name/<projectId|_>/` for uniform `person`/`system` rights, else
    /// `plugins/authdata/@pub/name/<projectId|_>/<authorityKey>/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// The process's private temporary directory.
    pub tmp: String,
}

/// The limits of this job's calls and results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CallLimits {
    /// The job's timeout in seconds (a command's own, at most 60).
    pub timeout_s: u16,
    /// The largest envelope line, in bytes.
    pub message_bytes: u32,
    /// The largest page of a paged reply, in bytes before base64.
    pub page_bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginContext {
    /// The host API the running host implements.
    #[schemars(with = "String")]
    pub host_api: Version,
    pub plugin: PluginRefFull,
    /// The permissions granted to this plugin (the job's rights are the invoker's
    /// intersected with these, checked by the host on every call).
    pub grants: Vec<Permission>,
    pub dirs: PluginDirs,
    /// The invoker's locale (a BCP 47 tag) for plugin-facing text.
    pub locale: String,
    pub limits: CallLimits,
}
