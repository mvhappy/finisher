//! `fin try [name]`, `fin try keep`, `fin try drop`: a scratch branch the default switches to
//! (`<server-data>/fin-config.json`), merged back or archived when done (Decision 32). Under an
//! agent session's credential the default and the tries are the session's own, on the server
//! (`agent.self.try`; plan 7A Decision 14a), and the file is untouched.

use clap::{Args, Subcommand};
use fin_core::{CoreError, Link};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::branch::{self, find};
use super::history::{Limits, key, merge_doc, merge_refs, record_doc};
use super::{Globals, configured_branch, lookup, target_branch};
use crate::agent_session;
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
    #[expect(dead_code, reason = "read through the schema only")]
    pub name: Option<String>,
}

/// No input besides the global flags.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

fn scope(backend: &dyn Backend) -> Result<(std::path::PathBuf, String), CoreError> {
    backend
        .config_scope()?
        .ok_or_else(|| crate::output::unavailable(json!({ "operation": "fin try" })))
}

pub fn run(backend: &dyn Backend, g: &Globals, a: &TryArgs) -> Result<Value, CoreError> {
    if agent_session::active() && !backend.dry_run_only() {
        return match &a.sub {
            None => session_start(backend, g, a.name.as_deref()),
            Some(TrySub::Keep) => session_finish(backend, g, true),
            Some(TrySub::Drop) => session_finish(backend, g, false),
        };
    }
    match &a.sub {
        None => start(backend, g, a.name.as_deref()),
        Some(TrySub::Keep) => finish(backend, g, true),
        Some(TrySub::Drop) => finish(backend, g, false),
    }
}

/// `branch.create` of a try from `from`. `argsHash` covers what was typed (the name and an
/// explicit `--branch`), not `from` when it is the default: the first attempt made the try the
/// default, so a retry with the key resolves another `from` and must still be answered from
/// the log (review #27).
fn create_try(
    backend: &dyn Backend,
    g: &Globals,
    name: Option<&str>,
    from: &str,
) -> Result<Value, CoreError> {
    let typed = json!({ "name": name, "branch": g.branch });
    branch::create_as(backend, g, ("try", typed), name, from, None)
}

/// Whether a try made (or answered from the log) should become the default: not a plan (a
/// `--dry-run` answered from the log is not one either), and not a retry whose branch was
/// kept or dropped since.
fn adopt(g: &Globals, r: &Value) -> bool {
    !g.dry_run && r["dryRun"] != json!(true) && r["branch"]["state"] == json!("active")
}

/// What a `keep`/`drop` retried with `--idem-key` acted on, read from its committed record.
struct Retried {
    /// The try branch.
    branch: String,
    /// A keep's merge target (the branch its `<key>-1` record is on).
    into: Option<String>,
}

/// The try a `keep`/`drop` retried with `--idem-key` acted on, from its committed record
/// (`<key>-1` of a merge, the `branchSet` of a drop): its first attempt switched the default
/// away from the try and forgot it, so the default no longer names it (review #27). `None`
/// without a key, offline, or when the key committed nothing (or something else, which the
/// server then refuses as `IDEM_KEY_REUSED`). A keep that merged nothing commits no record,
/// so its retry is not recognised.
fn retried_try(
    backend: &dyn Backend,
    g: &Globals,
    keep: bool,
) -> Result<Option<Retried>, CoreError> {
    if g.idem_key.is_none() || backend.dry_run_only() {
        return Ok(None);
    }
    let key = key(g)?;
    let first = if keep { format!("{key}-1") } else { key };
    let Some(c) = lookup(backend, &first)? else {
        return Ok(None);
    };
    Ok(match c.link.as_deref() {
        Some(Link::Merge { from, .. }) if keep => Some(Retried {
            branch: from.clone(),
            into: Some(c.tx.branch.clone()),
        }),
        Some(Link::BranchSet { branch_id, .. }) if !keep => Some(Retried {
            branch: branch_id.clone(),
            into: None,
        }),
        _ => None,
    })
}

/// An explicit `--branch` on a retry must name the try the key acted on (as typed: its id, its
/// name or `<name>#<id>`, case-insensitively; it may be archived by now), else the key belongs
/// to another command: `IDEM_KEY_REUSED` (review #26).
fn check_typed(g: &Globals, view: &Value) -> Result<(), CoreError> {
    let Some(typed) = &g.branch else {
        return Ok(());
    };
    let id = view["id"].as_str().unwrap_or_default();
    let name = view["name"].as_str().unwrap_or_default();
    let names = typed == id
        || typed.eq_ignore_ascii_case(name)
        || typed.eq_ignore_ascii_case(&format!("{name}#{id}"));
    if names {
        return Ok(());
    }
    let key = g.idem_key.clone().unwrap_or_default();
    Err(CoreError::new(
        "IDEM_KEY_REUSED",
        format!("--idem-key {key} already finished try {id}, not {typed:?}"),
    )
    .with_details(json!({ "txId": key, "branch": id, "requested": typed }))
    .with_fix("use a new --idem-key for a different command"))
}

fn start(backend: &dyn Backend, g: &Globals, name: Option<&str>) -> Result<Value, CoreError> {
    let (sd, project_id) = scope(backend)?;
    let from = target_branch(backend, g)?;
    let r = create_try(backend, g, name, &from)?;
    let dry = r["dryRun"] == json!(true);
    let view = r["branch"].clone();
    if adopt(g, &r) {
        let id = branch_id(&view)?;
        let parent = view["parent"].as_str().unwrap_or("main");
        config::start_try(&sd, &project_id, id, parent)?;
    }
    let mut doc = record_doc(&r["record"], dry);
    doc["branch"] = view;
    Ok(doc)
}

/// `keep` (merge into the parent) or `drop` (archive), then back to the parent as default.
/// Works without validating the default: a try whose branch is gone is dropped from the file.
fn finish(backend: &dyn Backend, g: &Globals, keep: bool) -> Result<Value, CoreError> {
    let (sd, project_id) = scope(backend)?;
    let retried = retried_try(backend, g, keep)?;
    let (current, from_config) = match &retried {
        Some(r) => (r.branch.clone(), false),
        None => configured_branch(backend, g)?,
    };
    let views = branch::list(backend, true)?;
    let not_try = || {
        usage(
            format!("{current:?} is not a try branch made by `fin try`"),
            json!({ "branch": current }),
            "fin try",
        )
    };
    let Ok(view) = find(&views, &current) else {
        // A stale default (the branch is gone): `drop` is the way out of it.
        // Cleared only while it is still the default: another fin may have selected a
        // branch meanwhile (review #50).
        if from_config && !keep && !g.dry_run {
            let now = config::end_try(&sd, &project_id, &current, None)?;
            let mut doc = json!({ "cleared": current });
            switched(&mut doc, now, "main");
            return Ok(doc);
        }
        return Err(if from_config {
            super::stale_default(&current, &views)
        } else {
            branch::not_found(&current, &views)
        });
    };
    let id = view["id"].as_str().unwrap_or_default().to_string();
    let recorded = config::try_parent(&sd, &project_id, &id)?;
    // A retry: the merge's own target, else (the try forgotten) the branch's parent, which is
    // what `start` recorded.
    let parent = match &retried {
        Some(r) => {
            check_typed(g, &view)?;
            r.into
                .clone()
                .or(recorded)
                .or_else(|| view["parent"].as_str().map(str::to_string))
        }
        None => recorded,
    }
    .ok_or_else(not_try)?;
    let mut doc = if keep {
        merge_doc(&merge_refs(
            backend,
            g,
            "try.keep",
            &id,
            &parent,
            false,
            Limits::default(),
        )?)
    } else {
        let (r, dry) = branch::set(backend, g, "try.drop", &id, None, Some(true))?;
        let mut doc = record_doc(&r, dry);
        let mut archived = view.clone();
        archived["state"] = json!("archived");
        doc["branch"] = archived;
        doc
    };
    if g.dry_run {
        doc["switchedTo"] = json!(parent);
    } else {
        // Back to where the try started, unless the default was moved off it meanwhile: one
        // compare-and-set under the file's lock with forgetting the try (review #50).
        let back = (parent != "main").then_some(parent.as_str());
        let now = config::end_try(&sd, &project_id, &id, back)?;
        switched(&mut doc, now, &parent);
    }
    Ok(doc)
}

/// `switchedTo`: the default in effect after a try ended (`main` when none is set). When it is
/// not `back`, another fin selected it meanwhile and it was kept: `defaultKept: true`.
fn switched(doc: &mut Value, now: Option<String>, back: &str) {
    let now = now.unwrap_or_else(|| "main".to_string());
    if now != back {
        doc["defaultKept"] = json!(true);
    }
    doc["switchedTo"] = json!(now);
}

/// `fin try` in an agent session: the branch is made from the session's default (in its
/// subtree), recorded on the server and made the default.
fn session_start(
    backend: &dyn Backend,
    g: &Globals,
    name: Option<&str>,
) -> Result<Value, CoreError> {
    let from = target_branch(backend, g)?;
    let r = create_try(backend, g, name, &from)?;
    let dry = r["dryRun"] == json!(true);
    let view = r["branch"].clone();
    if adopt(g, &r) {
        agent_session::record_try(backend, branch_id(&view)?)?;
    }
    let mut doc = record_doc(&r["record"], dry);
    doc["branch"] = view;
    Ok(doc)
}

/// `fin try keep|drop` in an agent session: the current default must be one of the session's
/// tries; it is merged into (or archived back to) the default it replaced, which the server
/// restores when the try is cleared.
fn session_finish(backend: &dyn Backend, g: &Globals, keep: bool) -> Result<Value, CoreError> {
    let state = agent_session::state(backend)?;
    let retried = retried_try(backend, g, keep)?;
    let current = match &retried {
        Some(r) => r.branch.clone(),
        None => g
            .branch
            .clone()
            .unwrap_or_else(|| state.default_branch.clone()),
    };
    let views = branch::list(backend, true)?;
    let view = find(&views, &current)?;
    let id = view["id"].as_str().unwrap_or_default().to_string();
    let entry = state.tries.iter().find(|t| t.branch == id);
    let recorded = entry.map(|t| t.previous_default.clone());
    // A retry: the merge's own target, else (a drop, the try cleared) the default the server
    // restored when the first attempt cleared it.
    let parent = match &retried {
        Some(r) => {
            check_typed(g, &view)?;
            r.into
                .clone()
                .or(recorded)
                .or(Some(state.default_branch.clone()))
        }
        None => recorded,
    };
    let Some(parent) = parent else {
        return Err(usage(
            format!("{current:?} is not a try branch this session made with `fin try`"),
            json!({ "branch": current }),
            "fin try",
        ));
    };
    let mut doc = if keep {
        merge_doc(&merge_refs(
            backend,
            g,
            "try.keep",
            &id,
            &parent,
            false,
            Limits::default(),
        )?)
    } else {
        let (r, dry) = branch::set(backend, g, "try.drop", &id, None, Some(true))?;
        let mut doc = record_doc(&r, dry);
        let mut archived = view.clone();
        archived["state"] = json!("archived");
        doc["branch"] = archived;
        doc
    };
    if !g.dry_run && entry.is_some() {
        agent_session::clear_try(backend, &id)?;
    }
    doc["switchedTo"] = json!(parent);
    Ok(doc)
}

/// A branch view's id; an empty one would be written to `fin-config.json`, whose check then
/// refuses the file.
fn branch_id(view: &Value) -> Result<&str, CoreError> {
    match view["id"].as_str() {
        Some(id) if !id.is_empty() => Ok(id),
        _ => Err(crate::output::internal(
            "the server answered a branch without an id",
            json!({ "branch": view }),
        )
        .with_fix("report this as a bug in finisherd")),
    }
}

#[cfg(test)]
#[path = "try_cmd_tests.rs"]
mod tests;
