//! `fin asset add`: the server probes the file, then the probe result is committed as
//! `asset.add`.

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
