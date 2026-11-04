//! `fin project create|open`. Both need the server; `create --dry-run` validates locally.
//! `open` only registers the folder with the server: it is not a write, so `--dry-run` does not
//! apply to it.

use clap::{Args, Subcommand};
use fin_core::{CoreError, Rational, frame_duration, new_project};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Globals;
use crate::backend::Backend;
use crate::paths;

#[derive(Debug, Clone, Subcommand)]
pub enum ProjectCmd {
    Create(CreateArgs),
    Open(OpenArgs),
}

/// Create a project folder with a video track t1 and an audio track t2.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateArgs {
    /// Folder to create the project in.
    #[arg(value_name = "dir")]
    pub dir: String,
    /// Project name.
    #[arg(long, value_name = "s")]
    pub name: String,
    /// Timeline frame rate in frames per second, as an integer or rational ("30", "30000/1001").
    #[arg(long, value_name = "fps", default_value = "30")]
    #[serde(default)]
    pub rate: Option<String>,
}

/// Open an existing project folder: registers it with the server. Not a write, so --dry-run
/// does not apply (it is accepted and ignored).
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenArgs {
    /// Project folder.
    #[arg(value_name = "dir")]
    pub dir: String,
}

pub fn create(backend: &dyn Backend, globals: &Globals, a: &CreateArgs) -> Result<Value, CoreError> {
    let rate_input = a.rate.as_deref().unwrap_or("30");
    let rate: Rational = rate_input.parse().map_err(|mut e: CoreError| {
        e.details = json!({ "input": rate_input, "field": "rate" });
        e.fix = Some("give the rate as frames per second: 30, 25, 24000/1001".to_string());
        e
    })?;
    frame_duration(rate)?;
    let dir = paths::resolve_str(&a.dir);
    let resolved = json!({ "dir": dir.display().to_string(), "name": a.name, "rate": rate });
    if globals.dry_run || backend.dry_run_only() {
        let project = new_project("$project".to_string(), a.name.clone(), rate)?;
        return Ok(json!({ "dryRun": true, "project": project, "resolved": resolved }));
    }
    let project = backend.create_project(&dir, &a.name, rate)?;
    Ok(json!({ "project": project, "resolved": resolved }))
}

pub fn open(backend: &dyn Backend, a: &OpenArgs) -> Result<Value, CoreError> {
    let dir = paths::resolve_str(&a.dir);
    let project = backend.open_project(&dir)?;
    Ok(json!({ "project": project, "resolved": { "dir": dir.display().to_string() } }))
}
