//! `fin asset add|move|remove`: for `add` the server probes the file (any asset kind: media,
//! image, SVG, dotLottie template, font or LUT), then the probe result is committed as
//! `asset.add` of that kind; `move` files an asset in a bin (or at the top level), `remove`
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

/// Probe a file (video, audio, image, SVG, dotLottie, font or .cube LUT) and add it to the
/// project as an asset of the kind its bytes show.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    /// File path; `~` expands to the home directory and relative paths resolve against the
    /// working directory. The resolved absolute path is echoed and stored.
    #[arg(value_name = "path")]
    pub path: String,
}

pub fn plan_add(b: &mut Builder<'_>, a: &AddArgs, placeholder: &str) -> Result<Value, CoreError> {
    // A `batch` line's path is confined like a typed one (agent credentials, 14c).
    crate::agent_paths::check(&a.path, b.project.as_deref())?;
    let probed = b.backend.probe(&paths::resolve_checked_str(&a.path)?)?;
    let mut resolved = json!({ "path": probed.path, "kind": probed.kind });
    if let Some(d) = probed.duration {
        resolved["duration"] = json!(d);
    }
    b.push(probed.into_asset_add(placeholder.to_string()).into())?;
    Ok(resolved)
}
