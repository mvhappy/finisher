//! `fin bin add|move|remove` (plan 4C Task 23): folders of assets. Without `--parent` a bin
//! sits (or moves) at the top level.

use clap::{Args, Subcommand};
use fin_core::{BinAdd, BinMove, BinRemove, CoreError, Op};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum BinCmd {
    Add(AddArgs),
    Move(MoveArgs),
    Remove(RemoveArgs),
    Ls(super::inspect::BinLsArgs),
}

/// Add a bin; binds $bin.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    #[arg(long, value_name = "s")]
    pub name: String,
    #[arg(long, value_name = "bN")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// Put a bin inside another one (`parent`), or at the top level without it.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArgs {
    #[arg(value_name = "bN")]
    pub bin: String,
    #[arg(long, value_name = "bN")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// Take an empty bin off the project.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    #[arg(value_name = "bN")]
    pub bin: String,
}

pub fn name(cmd: &BinCmd) -> &'static str {
    match cmd {
        BinCmd::Add(_) => "bin.add",
        BinCmd::Move(_) => "bin.move",
        BinCmd::Remove(_) => "bin.remove",
        BinCmd::Ls(_) => "bin.ls",
    }
}

pub fn plan(b: &mut Builder<'_>, cmd: &BinCmd, suffix: &str) -> Result<Value, CoreError> {
    match cmd {
        BinCmd::Ls(_) => Err(super::inspect::not_a_write(name(cmd))),
        BinCmd::Add(a) => {
            let id = format!("$bin{suffix}");
            b.push(Op::BinAdd(BinAdd {
                id,
                name: a.name.clone(),
                parent: a.parent.clone(),
            }))?;
            Ok(json!({ "name": a.name, "parent": a.parent }))
        }
        BinCmd::Move(a) => {
            b.push(Op::BinMove(BinMove {
                bin: a.bin.clone(),
                parent: a.parent.clone(),
            }))?;
            Ok(json!({ "bin": a.bin, "parent": a.parent }))
        }
        BinCmd::Remove(a) => {
            b.push(Op::BinRemove(BinRemove { bin: a.bin.clone() }))?;
            Ok(json!({ "bin": a.bin }))
        }
    }
}
