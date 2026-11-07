//! `plugin.lifecycle` and `registry.refresh` (sub-project 8 spec §11, 8A plan Task 11 and
//! Decision 27): emitted by `fin-node` (`source: finNode`) from its plugin registry calls.
//!
//! Neither holds free text: a bundled plugin is a [`Name`] from
//! [`names::BUNDLED_PLUGINS`](crate::events::names::BUNDLED_PLUGINS), any other plugin a keyed
//! [`Hashed`] ([`PluginRef`]); no registry id or path, publisher, version or package file name
//! is recorded; sizes are log₂ buckets ([`log2_bucket`]).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::{ApproxBytes, approx_struct};
use crate::events::class::{Code, Hashed, Name};
use crate::events::names;

/// A plugin as the journal names it: a bundled plugin by its table name, any other by the
/// journal's keyed hash of its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PluginRef {
    #[serde(deserialize_with = "names::bundled_plugin")]
    Bundled(Name),
    Hashed(Hashed),
}

impl PluginRef {
    /// The reference for plugin `id` (`@publisher/name`): [`PluginRef::Bundled`] when
    /// `BUNDLED_PLUGINS` lists it, else `hash(id)` (the journal's keyed hash; `None` when the
    /// journal cannot hash, and then nothing is recorded).
    pub fn of(id: &str, hash: impl FnOnce(&str) -> Option<Hashed>) -> Option<PluginRef> {
        match names::bundled_plugin_name(id) {
            Some(n) => Some(PluginRef::Bundled(n)),
            None => hash(id).map(PluginRef::Hashed),
        }
    }
}

impl ApproxBytes for PluginRef {
    fn approx_bytes(&self) -> usize {
        12 + match self {
            PluginRef::Bundled(n) => n.approx_bytes(),
            PluginRef::Hashed(h) => h.approx_bytes(),
        }
    }
}

/// How a plugin or registry call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PluginOutcome {
    Ok,
    /// Refused by a check (validation, a hash, TUF, a permission): `code` says which.
    Refused,
    /// Failed for another reason (I/O, internal).
    Failed,
}

/// Where an installed plugin came from (spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PluginSourceName {
    Bundled,
    Registry,
    Dev,
}

/// `content` or `code`; `unknown` when the call failed before a manifest was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PluginKindName {
    Content,
    Code,
    Unknown,
}

/// The overall risk shown to people (spec §4.3); `unknown` as for [`PluginKindName`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RiskName {
    Low,
    Medium,
    High,
    Unknown,
}

/// `plugin.lifecycle`: one plugin call (plan, install, update, downgrade, remove, enable,
/// disable, revoke).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginLifecycle {
    /// From `PLUGIN_ACTIONS`.
    #[serde(deserialize_with = "names::plugin_action")]
    pub action: Name,
    pub outcome: PluginOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
    // Absent when the call never learnt which plugin (a commit of a plan that is gone). Not a
    // doc comment: it would become `PluginRef`'s description in TypeScript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "PluginRef")]
    pub plugin: Option<PluginRef>,
    pub source: PluginSourceName,
    pub kind: PluginKindName,
    /// Permissions added or widened against the installed version (all of them for a first
    /// install), capped at 255.
    pub permissions_added: u8,
    pub risk: RiskName,
    /// The package's unpacked bytes as a log₂ bucket ([`log2_bucket`]).
    pub bytes_log2: u8,
    pub micros: u64,
}
approx_struct!(PluginLifecycle {
    action,
    outcome,
    code,
    plugin,
    source,
    kind,
    permissions_added,
    risk,
    bytes_log2,
    micros
});

/// `registry.refresh`: one refresh of a registry's trusted metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryRefresh {
    pub outcome: PluginOutcome,
    /// A `TUF_*` code, `CLOCK_SUSPECT`, `REGISTRY_UNREACHABLE`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
    /// Roles whose trusted version changed.
    pub roles_updated: u16,
    /// Targets listed after the refresh, as a log₂ bucket.
    pub targets_log2: u8,
    /// Days the registry's metadata is past its expiry (relaxed freshness), when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u16")]
    pub stale_days: Option<u16>,
    /// A human principal asked for it (agents' and `system`'s refreshes install nothing).
    pub by_human: bool,
    pub micros: u64,
}
approx_struct!(RegistryRefresh {
    outcome,
    code,
    roles_updated,
    targets_log2,
    stale_days,
    by_human,
    micros
});

/// `n` as a log₂ bucket: 0 for 0, else `floor(log2(n)) + 1` (1 for 1, 2 for 2–3, 11 for
/// 1,024–2,047, ...).
pub fn log2_bucket(n: u64) -> u8 {
    (u64::BITS - n.leading_zeros()) as u8
}
