//! The client RPCs of plugin commands (spec §3.8, sp8b Task 9): `plugin.commands` lists the
//! commands a caller can run, `plugin.command.run` runs one. The host builds the job, commits a
//! returned transaction under the invoker with `Tx.via`, and answers [`PluginCommandRunResult`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::schema_subset::SchemaSubset;

/// One command as `plugin.commands` lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCommandInfo {
    /// The registry name, `plugin:<id>/<command>` (the name 7B's command rules match).
    pub name: String,
    /// `<id>@<version>` of the enabled plugin that serves it.
    pub plugin: String,
    /// The command's name in its manifest.
    pub command: String,
    /// A message key in the plugin's `en` catalogue.
    pub title: String,
    pub args: SchemaSubset,
    /// Whether it returns a transaction (running it needs `project.write`).
    pub writes: bool,
    /// Its timeout in seconds (the manifest's, 10 when absent, at most 60).
    pub timeout_s: u16,
}

/// `plugin.commands`: `digest` is the BLAKE3 of the canonical JSON of `commands`, so a client
/// caches the list per server and digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCommandList {
    pub digest: String,
    pub commands: Vec<PluginCommandInfo>,
}

/// `plugin.command.run`'s request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCommandRunRequest {
    pub project_id: String,
    /// A branch name, id or `name#id`; `main` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `plugin:<id>/<command>`.
    pub name: String,
    #[serde(default)]
    pub args: Value,
    /// Validate the returned transaction against the snapshot the plugin saw; commit nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}

/// What became of a command run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CommandOutcome {
    /// The transaction was committed on the revision the plugin saw.
    Accepted,
    /// The branch had moved on: fin-core rebased the transaction and committed it.
    Rebased,
    /// fin-core refused it (`CONFLICT`): nothing was committed; `ops` are what was proposed.
    Conflict,
    /// The command returned data, or a dry run validated its transaction.
    Data,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCommandRunResult {
    pub outcome: CommandOutcome,
    /// The committed transaction's id (accepted, rebased).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_id: Option<String>,
    /// The committed revision; for a dry run or a conflict, the revision the plugin saw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
    /// A data command's value as the plugin returned it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// The proposed ops (dry run, conflict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<Vec<fin_core::Op>>,
    /// A dry run: nothing was committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}
