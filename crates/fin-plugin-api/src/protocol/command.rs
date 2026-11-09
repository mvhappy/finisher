//! A `plugin.command` v1 job's params and result (spec §3.6 `command.run`, §3.8).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A command run: the host has checked `args` against the command's schema and pinned `rev`
/// to the head of `branch`. Decoded leniently (a field from a newer host is ignored).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommandRun {
    /// The command's name in the manifest (not the `plugin:<id>/` registry name).
    pub command: String,
    pub args: Value,
    pub project: String,
    pub branch: String,
    pub rev: u64,
}

/// What a command returns, at most one envelope line (1 MiB). `tx` is committed by the host
/// under the invoker with `baseRev = rev` and `via` set by the host only; plugins propose,
/// fin-core decides (spec §2.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum CommandOut {
    /// A transaction of fin-core ops (only from a command whose manifest says `writes`).
    Tx { ops: Vec<fin_core::Op> },
    /// Data returned to the caller as is.
    Data { value: Value },
}
