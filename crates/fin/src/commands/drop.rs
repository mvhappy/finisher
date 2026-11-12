//! `fin drop <txId>… --from <b> [--into <b>]` and `fin drop --undo …` (5E, spec §8.3): write a
//! `drop` record on the target branch that keeps a rejected change out of every later merge of
//! the source into it, on this server or any other, or lift it again. The txIds are origin
//! transactions of the source branch (the change list of `fin review show`). On a protected
//! target the server asks for the reviewer's scope; elsewhere it is a plain write.

use clap::Args;
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::history::{Req, record_doc, send_flagged};
use super::{Globals, target_branch};
use crate::backend::Backend;

/// Keep rejected changes out of merges into a branch (or bring them back with --undo).
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DropArgs {
    /// The origin txIds to drop (with --undo: to bring back).
    #[arg(value_name = "txId", required = true)]
    pub tx: Vec<String>,
    /// The branch the changes are on.
    #[arg(long, value_name = "b")]
    pub from: String,
    /// The branch they must stay out of (default: the current branch).
    #[arg(long, value_name = "b")]
    #[serde(default)]
    pub into: Option<String>,
    /// Lift the drop: later merges of --from into --into copy these again.
    #[arg(long)]
    #[serde(default)]
    pub undo: bool,
}

pub fn run(backend: &dyn Backend, g: &Globals, a: &DropArgs) -> Result<Value, CoreError> {
    let into = match &a.into {
        Some(b) => b.clone(),
        None => target_branch(backend, g)?,
    };
    // Sorted and deduplicated: the argsHash must not depend on the order given.
    let mut ids = a.tx.clone();
    ids.sort();
    ids.dedup();
    let list = if a.undo { "undrops" } else { "drops" };
    let fields = json!({ "from": a.from, "into": into, list: ids });
    let (answer, dry) = send_flagged(
        backend,
        g,
        Req {
            tag: "history.drop",
            cmd: "drop",
            args: fields.clone(),
            fields,
            hashed: true,
            group: false,
        },
    )?;
    let mut doc = record_doc(&answer, dry);
    doc["from"] = json!(a.from);
    doc["into"] = json!(into);
    doc[if a.undo { "undropped" } else { "dropped" }] = json!(ids);
    Ok(json!({ "drop": doc }))
}
