//! `fin effect add|remove|move` (plan 4C Task 24). `add` binds `$fx`; its `--set p=v` params
//! are typed by the kind's specs (`fin schema param --scope effect`).

use clap::{Args, Subcommand};
use fin_core::params::ScopeOf;
use fin_core::{CoreError, EffectAdd, EffectMove, EffectRemove, Op};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use super::param::parse_sets;
use crate::addr;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum EffectCmd {
    Add(AddArgs),
    Remove(RemoveArgs),
    Move(MoveArgs),
    Ls(super::inspect::EffectLsArgs),
}

/// Add an effect of `kind` (colorAdjust, lut, blur) to an item; binds $fx.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(value_name = "kind")]
    pub kind: String,
    /// Place in the item's effect list (default: last).
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u32>,
    /// `param=value`, typed by the kind's spec.
    #[arg(long = "set", value_name = "p=v", allow_hyphen_values = true)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<String>,
}

/// Remove an effect from an item.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(value_name = "fxN")]
    pub effect: String,
}

/// Move an effect to another place in its item's list.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(value_name = "fxN")]
    pub effect: String,
    #[arg(long, value_name = "n")]
    pub to: u32,
}

pub fn name(cmd: &EffectCmd) -> &'static str {
    match cmd {
        EffectCmd::Add(_) => "effect.add",
        EffectCmd::Remove(_) => "effect.remove",
        EffectCmd::Move(_) => "effect.move",
        EffectCmd::Ls(_) => "effect.ls",
    }
}

pub fn plan(b: &mut Builder<'_>, cmd: &EffectCmd, suffix: &str) -> Result<Value, CoreError> {
    match cmd {
        EffectCmd::Ls(_) => Err(super::inspect::not_a_write(name(cmd))),
        EffectCmd::Add(a) => {
            let item = b.id(&a.item);
            let (tl, _, _) = addr::item(&b.work, &item)?;
            let rate = tl.rate;
            let params = parse_sets(&ScopeOf::Effect { kind: &a.kind }, &a.set, rate)?;
            let id = format!("$fx{suffix}");
            b.push(Op::EffectAdd(EffectAdd {
                item: a.item.clone(),
                id: id.clone(),
                kind: a.kind.clone(),
                at: a.at,
                params: params.clone(),
            }))?;
            Ok(
                json!({ "item": a.item, "kind": a.kind, "effect": id, "at": a.at, "params": params }),
            )
        }
        EffectCmd::Remove(a) => {
            b.push(Op::EffectRemove(EffectRemove {
                item: a.item.clone(),
                effect: a.effect.clone(),
            }))?;
            Ok(json!({ "item": a.item, "effect": a.effect }))
        }
        EffectCmd::Move(a) => {
            b.push(Op::EffectMove(EffectMove {
                item: a.item.clone(),
                effect: a.effect.clone(),
                to: a.to,
            }))?;
            Ok(json!({ "item": a.item, "effect": a.effect, "to": a.to }))
        }
    }
}
