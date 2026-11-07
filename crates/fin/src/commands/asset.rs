//! `fin asset add|move|remove`: for `add` the server probes the file, then the probe result is
//! committed as `asset.add`; `move` files an asset in a bin (or at the top level), `remove`
//! takes an unused one off the project (`ASSET_IN_USE` lists its users).

use clap::{Args, Subcommand};
use fin_core::{AssetMove, CoreError, Op};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use crate::paths;

#[derive(Debug, Clone, Subcommand)]
pub enum AssetCmd {
    Add(AddArgs),
    Move(MoveArgs),
    Remove(RemoveArgs),
}

/// The asset edits that are not probes (`fin asset move|remove`).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum AssetEdit {
    Move(MoveArgs),
    Remove(RemoveArgs),
}

/// Put an asset in a bin, or at the top level without `bin`.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArgs {
    #[arg(value_name = "aN")]
    pub asset: String,
    #[arg(long, value_name = "bN")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<String>,
}

/// Take an asset no item, effect or font pin uses off the project.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    #[arg(value_name = "aN")]
    pub asset: String,
}

pub fn edit_name(cmd: &AssetEdit) -> &'static str {
    match cmd {
        AssetEdit::Move(_) => "asset.move",
        AssetEdit::Remove(_) => "asset.remove",
    }
}

pub fn plan_edit(b: &mut Builder<'_>, cmd: &AssetEdit) -> Result<Value, CoreError> {
    match cmd {
        AssetEdit::Move(a) => {
            b.push(Op::AssetMove(AssetMove {
                asset: a.asset.clone(),
                bin: a.bin.clone(),
            }))?;
            Ok(json!({ "asset": a.asset, "bin": a.bin }))
        }
        AssetEdit::Remove(a) => {
            b.push(Op::AssetRemove {
                id: a.asset.clone(),
            })?;
            Ok(json!({ "asset": a.asset }))
        }
    }
}

/// Probe a media file and add it to the project.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    /// Media file path; `~` expands to the home directory and relative paths resolve against the
    /// working directory. The resolved absolute path is echoed and stored.
    #[arg(value_name = "path")]
    pub path: String,
}

pub fn plan_add(b: &mut Builder<'_>, a: &AddArgs, placeholder: &str) -> Result<Value, CoreError> {
    let probed = b.backend.probe(&paths::resolve_str(&a.path))?;
    let resolved = json!({ "path": probed.path, "duration": probed.duration });
    b.push(
        fin_core::AssetAdd::media(
            placeholder.to_string(),
            probed.path,
            probed.duration,
            probed.video,
            probed.audio,
            probed.fingerprint,
        )
        .into(),
    )?;
    Ok(resolved)
}
