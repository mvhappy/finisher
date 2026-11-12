//! A `plugin.command` v1 job's params and result (spec §3.6 `command.run`, §3.8).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A command run: the host has checked `args` against the command's schema and pinned `rev`
/// to the head of `branch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRun {
    /// The command's name in the manifest (not the `plugin:<id>/` registry name).
    pub command: String,
    pub args: Value,
    pub project: String,
    pub branch: String,
    pub rev: u64,
    /// The host's handle on the snapshot it read for this run at submit (opaque to plugins):
    /// every page the run reads comes from that snapshot, of this run only, so a rewind or
    /// rebuild that reaches `rev` again in a newer history is never served from an older one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
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
