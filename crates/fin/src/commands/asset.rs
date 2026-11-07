//! `fin asset add`: the server probes the file (any asset kind: media, image, SVG, dotLottie
//! template, font or LUT), then the probe result is committed as `asset.add` of that kind.

use clap::{Args, Subcommand};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Builder;
use crate::paths;

#[derive(Debug, Clone, Subcommand)]
pub enum AssetCmd {
    Add(AddArgs),
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
    let probed = b.backend.probe(&paths::resolve_str(&a.path))?;
    let mut resolved = json!({ "path": probed.path, "kind": probed.kind });
    if let Some(d) = probed.duration {
        resolved["duration"] = json!(d);
    }
    b.push(probed.into_asset_add(placeholder.to_string()).into())?;
    Ok(resolved)
}
