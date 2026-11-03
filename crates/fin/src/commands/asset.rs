//! `fin asset add`: the server probes the file, then the probe result is committed as
//! `asset.add`.

use std::path::Path;

use clap::{Args, Subcommand};
use fin_core::{CoreError, Op};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Builder;

#[derive(Debug, Clone, Subcommand)]
pub enum AssetCmd {
    Add(AddArgs),
}

/// Probe a media file and add it to the project.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    /// Media file path (relative paths resolve against the working directory).
    #[arg(value_name = "path")]
    pub path: String,
}

pub fn plan_add(b: &mut Builder<'_>, a: &AddArgs, placeholder: &str) -> Result<Value, CoreError> {
    let path = Path::new(&a.path);
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let probed = b.backend.probe(&absolute)?;
    let resolved = json!({ "path": probed.path, "duration": probed.duration });
    b.push(Op::AssetAdd {
        id: placeholder.to_string(),
        path: probed.path,
        duration: probed.duration,
        video: probed.video,
        audio: probed.audio,
        fingerprint: probed.fingerprint,
    })?;
    Ok(resolved)
}
