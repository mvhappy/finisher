//! `fin try [name]`, `fin try keep`, `fin try drop`: a scratch branch the default switches to
//! (`<server-data>/fin-config.json`), merged back or archived when done (Decision 32).

use clap::{Args, Subcommand};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::branch::{self, find};
use super::history::{merge_doc, merge_refs, record_doc};
use super::{Globals, target_branch};
use crate::backend::Backend;
use crate::config;
use crate::output::usage;

#[derive(Debug, Clone, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct TryArgs {
    /// Name of the try branch; default `try-<n>` for the new branch id `br<n>`.
    #[arg(value_name = "name")]
    pub name: Option<String>,
    #[command(subcommand)]
    pub sub: Option<TrySub>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum TrySub {
    /// Merge the try branch into the branch it was made from and switch back.
    Keep,
    /// Archive the try branch and switch back.
    Drop,
}

/// Fork a try branch from the current one and make it the default branch.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameArgs {
    /// Branch name; default `try-<n>`. (Schema only: clap reads it as `TryArgs.name`.)
    #[serde(default)]
    #[allow(dead_code)]
    pub name: Option<String>,
}

/// No input besides the global flags.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

fn scope(backend: &dyn Backend) -> Result<(std::path::PathBuf, String), CoreError> {
    backend
        .config_scope()
        .ok_or_else(|| crate::output::unavailable(json!({ "operation": "fin try" })))
}

pub fn run(backend: &dyn Backend, g: &Globals, a: &TryArgs) -> Result<Value, CoreError> {
    match &a.sub {
        None => start(backend, g, a.name.as_deref()),
        Some(TrySub::Keep) => finish(backend, g, true),
        Some(TrySub::Drop) => finish(backend, g, false),
    }
}

fn start(backend: &dyn Backend, g: &Globals, name: Option<&str>) -> Result<Value, CoreError> {
    let (sd, project_id) = scope(backend)?;
    let from = target_branch(backend, g)?;
    let r = branch::create(backend, g, name, &from, None)?;
    let dry = r["dryRun"] == json!(true);
    let view = r["branch"].clone();
    if !dry {
        let id = view["id"].as_str().unwrap_or_default();
        let parent = view["parent"].as_str().unwrap_or("main");
        config::record_try(&sd, id, parent)?;
        config::set_default_branch(&sd, &project_id, Some(id))?;
    }
    let mut doc = record_doc(&r["record"], dry);
    doc["branch"] = view;
    Ok(doc)
}

/// `keep` (merge into the parent) or `drop` (archive), then back to the parent as default.
fn finish(backend: &dyn Backend, g: &Globals, keep: bool) -> Result<Value, CoreError> {
    let (sd, project_id) = scope(backend)?;
    let current = target_branch(backend, g)?;
    let views = branch::list(backend, true)?;
    let not_try = || {
        usage(
            format!("{current:?} is not a try branch made by `fin try`"),
            json!({ "branch": current }),
            "fin try",
        )
    };
    let view = find(&views, &current).map_err(|e| if current == "main" { not_try() } else { e })?;
    let id = view["id"].as_str().unwrap_or_default().to_string();
    let parent = config::try_parent(&sd, &id)?.ok_or_else(not_try)?;
    let mut doc = if keep {
        merge_doc(&merge_refs(backend, g, &id, &parent, false)?)
    } else {
        let r = branch::set(backend, g, "try.drop", &id, None, Some(true))?;
        let mut doc = record_doc(&r, g.dry_run);
        let mut archived = view.clone();
        archived["state"] = json!("archived");
        doc["branch"] = archived;
        doc
    };
    if !g.dry_run {
        // Back to where the try started, unless the default was moved off it meanwhile.
        if config::default_branch(&sd, &project_id)?.as_deref() == Some(id.as_str()) {
            let back = (parent != "main").then_some(parent.as_str());
            config::set_default_branch(&sd, &project_id, back)?;
        }
        config::forget_try(&sd, &id)?;
    }
    doc["switchedTo"] = json!(parent);
    Ok(doc)
}
