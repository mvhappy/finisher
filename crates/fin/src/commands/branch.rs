//! `fin branch new|ls|rename|archive`. References are an id, an active name or `<name>#<id>`.

use clap::{Args, Subcommand};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::history::{Req, record_doc, send};
use super::{Globals, target_branch};
use crate::backend::Backend;
use crate::output::usage;

#[derive(Debug, Clone, Subcommand)]
pub enum BranchCmd {
    New(NewArgs),
    Ls(LsArgs),
    Rename(RenameArgs),
    Archive(ArchiveArgs),
}

/// Fork a new branch (a copy of the project at a point of another branch).
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewArgs {
    /// Branch name: letters, digits, `.`, `_`, `-`; not `main` or `br<n>`.
    #[arg(value_name = "name")]
    pub name: String,
    /// Parent branch, optionally `@<seq>` or `@<checkpoint>` (digits mean a seq). Default:
    /// the current branch at its head.
    #[arg(long, value_name = "b[@x]")]
    #[serde(default)]
    pub from: Option<String>,
}

/// List the branches.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LsArgs {
    /// Include archived branches.
    #[arg(long)]
    #[serde(default)]
    pub all: bool,
}

/// Rename a branch.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameArgs {
    #[arg(value_name = "old")]
    pub old: String,
    #[arg(value_name = "new")]
    pub new: String,
}

/// Archive a branch, or bring an archived one back.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveArgs {
    /// The branch; an archived one by id or `<name>#<id>`.
    /// (Not named `branch`: that is the global `--branch`'s clap id.)
    #[arg(value_name = "b")]
    #[serde(rename = "branch")]
    pub target: String,
    /// Restore instead of archive.
    #[arg(long)]
    #[serde(default)]
    pub restore: bool,
    /// With --restore: the name to restore it under (when its own is taken).
    #[arg(long = "as", value_name = "new", requires = "restore")]
    #[serde(default, rename = "as")]
    pub as_name: Option<String>,
}

/// `<branch>[@<seq|checkpoint>]` → the branch and `at` (`{seq}` or `{checkpoint}`).
pub fn parse_from(from: &str) -> Result<(String, Option<Value>), CoreError> {
    let Some((branch, at)) = from.split_once('@') else {
        return Ok((from.to_string(), None));
    };
    if branch.is_empty() || at.is_empty() {
        return Err(usage(
            format!("--from {from:?} needs a branch and a point: <branch>@<seq|checkpoint>"),
            json!({ "from": from }),
            "e.g. --from main@12 or --from main@before-fx",
        ));
    }
    let at = if at.bytes().all(|b| b.is_ascii_digit()) {
        let seq: u64 = at.parse().map_err(|_| {
            usage(
                format!("{at:?} is not a valid seq"),
                json!({ "from": from }),
                "use a seq below 2^64 or a checkpoint name",
            )
        })?;
        json!({ "seq": seq })
    } else {
        json!({ "checkpoint": at })
    };
    Ok((branch.to_string(), Some(at)))
}

/// `branch.create`: `{branch, record, dryRun}`.
pub fn create(
    backend: &dyn Backend,
    g: &Globals,
    name: Option<&str>,
    from: &str,
    at: Option<Value>,
) -> Result<Value, CoreError> {
    let args = json!({ "name": name, "from": from, "at": at });
    let mut fields = json!({ "from": from });
    if let Some(n) = name {
        fields["name"] = json!(n);
    }
    if let Some(at) = at {
        fields["at"] = at;
    }
    send(
        backend,
        g,
        Req {
            tag: "branch.create",
            cmd: "branch.new",
            args,
            fields,
            hashed: true,
            group: false,
        },
    )
}

/// `branch.set`: the committed record.
pub fn set(
    backend: &dyn Backend,
    g: &Globals,
    cmd: &str,
    branch: &str,
    name: Option<&str>,
    archived: Option<bool>,
) -> Result<Value, CoreError> {
    let args = json!({ "branch": branch, "name": name, "archived": archived });
    let mut fields = json!({ "branch": branch });
    if let Some(n) = name {
        fields["name"] = json!(n);
    }
    if let Some(a) = archived {
        fields["archived"] = json!(a);
    }
    send(
        backend,
        g,
        Req {
            tag: "branch.set",
            cmd,
            args,
            fields,
            hashed: true,
            group: false,
        },
    )
}

/// `branch.list`.
pub fn list(backend: &dyn Backend, all: bool) -> Result<Vec<Value>, CoreError> {
    let r = backend.project_call("branch.list", json!({ "all": all }))?;
    Ok(r["branches"].as_array().cloned().unwrap_or_default())
}

pub fn not_found(reference: &str, views: &[Value]) -> CoreError {
    let alternatives: Vec<&str> = views.iter().filter_map(|v| v["display"].as_str()).collect();
    CoreError::new("BRANCH_NOT_FOUND", format!("no branch {reference:?}"))
        .with_details(json!({ "branch": reference, "alternatives": alternatives }))
        .with_fix("fin branch ls --all lists the branches (an archived one by id or <name>#<id>)")
}

/// The listing entry a reference names.
pub fn find(views: &[Value], reference: &str) -> Result<Value, CoreError> {
    let named = |v: &&Value| v["display"] == json!(reference) || v["name"] == json!(reference);
    views
        .iter()
        .find(|v| v["id"] == json!(reference))
        .or_else(|| views.iter().find(named))
        .cloned()
        .ok_or_else(|| not_found(reference, views))
}

pub fn run(backend: &dyn Backend, g: &Globals, cmd: &BranchCmd) -> Result<Value, CoreError> {
    match cmd {
        BranchCmd::New(a) => {
            let (from, at) = match &a.from {
                Some(f) => parse_from(f)?,
                None => (target_branch(backend, g)?, None),
            };
            let r = create(backend, g, Some(&a.name), &from, at)?;
            let mut doc = record_doc(&r["record"], r["dryRun"] == json!(true));
            doc["branch"] = r["branch"].clone();
            Ok(doc)
        }
        BranchCmd::Ls(a) => Ok(json!({ "branches": list(backend, a.all)? })),
        BranchCmd::Rename(a) => {
            let r = set(backend, g, "branch.rename", &a.old, Some(&a.new), None)?;
            Ok(record_doc(&r, g.dry_run))
        }
        BranchCmd::Archive(a) => {
            let r = set(
                backend,
                g,
                "branch.archive",
                &a.target,
                a.as_name.as_deref(),
                Some(!a.restore),
            )?;
            Ok(record_doc(&r, g.dry_run))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_splits_branch_and_point() {
        assert_eq!(parse_from("main").unwrap(), ("main".into(), None));
        assert_eq!(
            parse_from("b@12").unwrap(),
            ("b".into(), Some(json!({ "seq": 12 })))
        );
        assert_eq!(
            parse_from("b@before-fx").unwrap(),
            ("b".into(), Some(json!({ "checkpoint": "before-fx" })))
        );
        assert_eq!(
            parse_from("old#br3@0").unwrap(),
            ("old#br3".into(), Some(json!({ "seq": 0 })))
        );
        for bad in ["@3", "b@", "b@99999999999999999999999"] {
            assert_eq!(parse_from(bad).unwrap_err().code, "USAGE", "{bad}");
        }
    }
}
