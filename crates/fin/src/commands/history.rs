//! `fin undo|redo|checkpoint|merge|cherry-pick|revert` and the request path every branch and
//! history write shares: `txId` (the `--idem-key` or a fresh ULID), `author`, `dryRun` and,
//! with a key, `argsHash` (Decision 26). The server plans, commits and answers retries from
//! the log; with no server there is nothing to plan against, so these commands need one even
//! for `--dry-run`.

use clap::Args;
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Globals, args_hash, author, lookup, target_branch};
use crate::backend::Backend;
use crate::client::IN_FLIGHT_CODES;
use crate::output::usage;

/// Undo your last edit on the branch, or everything of yours made after a checkpoint.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoArgs {
    /// Undo all of your edits made after this checkpoint.
    #[arg(long, value_name = "checkpoint")]
    #[serde(default)]
    pub to: Option<String>,
}

/// Name the branch's current head.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointArgs {
    /// Checkpoint name: letters, digits, `.`, `_`, `-`; not all digits.
    #[arg(value_name = "name")]
    pub name: String,
}

/// Merge a branch into another (default: the current branch).
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeArgs {
    /// The branch to merge from.
    /// (Not named `branch`: that is the global `--branch`'s clap id.)
    #[arg(value_name = "branch")]
    #[serde(rename = "branch")]
    pub source: String,
    /// The branch to merge into.
    #[arg(long, value_name = "b")]
    #[serde(default)]
    pub into: Option<String>,
    /// Leave records that conflict on the source and merge the rest.
    #[arg(long = "skip-conflicting")]
    #[serde(default, rename = "skipConflicting")]
    pub skip_conflicting: bool,
}

/// Re-apply one record of another branch here.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CherryPickArgs {
    /// The txId of the record to pick.
    #[arg(value_name = "txId")]
    pub tx: String,
    /// The branch to pick into.
    #[arg(long, value_name = "b")]
    #[serde(default)]
    pub into: Option<String>,
}

/// Undo a merge on the branch.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevertArgs {
    /// The mergeId (the merge's `--idem-key` or generated txId).
    #[arg(value_name = "mergeId")]
    pub merge: String,
}

/// One branch or history request.
pub struct Req<'a> {
    pub tag: &'a str,
    /// The command's name for `argsHash`.
    pub cmd: &'a str,
    /// The command input `argsHash` covers.
    pub args: Value,
    /// The request's own fields (a JSON object).
    pub fields: Value,
    /// Whether the RPC takes `argsHash` (`project.recover` does not).
    pub hashed: bool,
    /// The record is `<txId>-1` (a merge group).
    pub group: bool,
}

pub fn key(globals: &Globals) -> Result<String, CoreError> {
    match &globals.idem_key {
        Some(k) if k.trim().is_empty() => Err(usage(
            "--idem-key must not be empty",
            json!({ "field": "idem-key" }),
            "pass a non-empty key",
        )),
        Some(k) => Ok(k.clone()),
        None => Ok(ulid::Ulid::new().to_string()),
    }
}

/// Sends `req` and returns the server's answer.
pub fn send(backend: &dyn Backend, globals: &Globals, req: Req<'_>) -> Result<Value, CoreError> {
    let tx_id = key(globals)?;
    let mut payload = req.fields;
    payload["txId"] = json!(tx_id);
    payload["author"] = json!(author());
    if globals.dry_run {
        payload["dryRun"] = json!(true);
    }
    if req.hashed && globals.idem_key.is_some() {
        payload["argsHash"] = json!(args_hash(&[json!({ "cmd": req.cmd, "args": req.args })]));
    }
    let retry_safe = globals.idem_key.is_some() && !globals.dry_run;
    if retry_safe && !backend.dry_run_only() {
        // A retry whose record was set aside is reported, never sent again blindly.
        let first = if req.group {
            format!("{tx_id}-1")
        } else {
            tx_id.clone()
        };
        lookup(backend, &first)?;
    }
    match backend.project_call(req.tag, payload.clone()) {
        // The answer was lost: the server answers the same txId from the log.
        Err(e) if retry_safe && IN_FLIGHT_CODES.contains(&e.code.as_str()) => {
            backend.project_call(req.tag, payload)
        }
        other => other,
    }
}

fn last_of(committed: &Value) -> (Value, Value) {
    match committed.as_array().and_then(|a| a.last()) {
        Some(c) => (c["rev"].clone(), c["tx"]["txId"].clone()),
        None => (Value::Null, Value::Null),
    }
}

/// `{rev, txId}` of a bare committed record, plus `dryRun` when planned only.
pub fn record_doc(c: &Value, dry: bool) -> Value {
    let mut doc = json!({ "rev": c["rev"], "txId": c["tx"]["txId"] });
    if dry {
        doc["dryRun"] = json!(true);
    }
    doc
}

/// `{[<key>: [...],] rev, txId, noOp?, dryRun?}` of a `HistoryResult`. `rev` is null when the
/// operation committed nothing (`noOp` says why).
fn result_doc(r: &Value, named: Option<(&str, &str)>) -> Value {
    let (rev, tx_id) = last_of(&r["committed"]);
    let mut doc = json!({ "rev": rev, "txId": tx_id });
    if let Some((key, source)) = named {
        doc[key] = r[source].clone();
    }
    if let Some(n) = r.get("noOp").filter(|n| !n.is_null()) {
        doc["noOp"] = n.clone();
    }
    if r["dryRun"] == json!(true) {
        doc["dryRun"] = json!(true);
    }
    doc
}

pub fn undo(backend: &dyn Backend, g: &Globals, a: &UndoArgs) -> Result<Value, CoreError> {
    let branch = target_branch(backend, g)?;
    let mut fields = json!({ "branch": branch });
    if let Some(to) = &a.to {
        fields["to"] = json!(to);
    }
    let args = json!({ "branch": branch, "to": a.to });
    let r = send(
        backend,
        g,
        Req {
            tag: "history.undo",
            cmd: "undo",
            args,
            fields,
            hashed: true,
            group: false,
        },
    )?;
    Ok(result_doc(&r, Some(("undone", "undone"))))
}

pub fn redo(backend: &dyn Backend, g: &Globals) -> Result<Value, CoreError> {
    let branch = target_branch(backend, g)?;
    let args = json!({ "branch": branch });
    let r = send(
        backend,
        g,
        Req {
            tag: "history.redo",
            cmd: "redo",
            args,
            fields: json!({ "branch": branch }),
            hashed: true,
            group: false,
        },
    )?;
    // The result's `undone` names the undo a redo reverses.
    Ok(result_doc(&r, Some(("redone", "undone"))))
}

pub fn checkpoint(
    backend: &dyn Backend,
    g: &Globals,
    a: &CheckpointArgs,
) -> Result<Value, CoreError> {
    let branch = target_branch(backend, g)?;
    let args = json!({ "branch": branch, "name": a.name });
    let fields = json!({ "branch": branch, "name": a.name });
    let r = send(
        backend,
        g,
        Req {
            tag: "history.checkpoint",
            cmd: "checkpoint",
            args,
            fields,
            hashed: true,
            group: false,
        },
    )?;
    Ok(record_doc(&r, g.dry_run))
}

/// `{merge: {...report, rev, records}}`: every report field but the committed records, which
/// are summarised (`records`, `rev` of the last).
pub fn merge_doc(report: &Value) -> Value {
    let mut merge = report.clone();
    if let Some(map) = merge.as_object_mut() {
        let committed = map.remove("committed").unwrap_or(Value::Null);
        let (rev, _) = last_of(&committed);
        map.insert("rev".into(), rev);
        map.insert(
            "records".into(),
            json!(committed.as_array().map_or(0, Vec::len)),
        );
    }
    json!({ "merge": merge })
}

/// Merges `from` into `into`.
pub fn merge_refs(
    backend: &dyn Backend,
    g: &Globals,
    from: &str,
    into: &str,
    skip_conflicting: bool,
) -> Result<Value, CoreError> {
    let args = json!({ "from": from, "into": into, "skipConflicting": skip_conflicting });
    let mut fields = json!({ "from": from, "into": into });
    if skip_conflicting {
        fields["skipConflicting"] = json!(true);
    }
    send(
        backend,
        g,
        Req {
            tag: "history.merge",
            cmd: "merge",
            args,
            fields,
            hashed: true,
            group: true,
        },
    )
}

pub fn merge(backend: &dyn Backend, g: &Globals, a: &MergeArgs) -> Result<Value, CoreError> {
    let into = match &a.into {
        Some(b) => b.clone(),
        None => target_branch(backend, g)?,
    };
    let report = merge_refs(backend, g, &a.source, &into, a.skip_conflicting)?;
    Ok(merge_doc(&report))
}

pub fn cherry_pick(
    backend: &dyn Backend,
    g: &Globals,
    a: &CherryPickArgs,
) -> Result<Value, CoreError> {
    let into = match &a.into {
        Some(b) => b.clone(),
        None => target_branch(backend, g)?,
    };
    let args = json!({ "origin": a.tx, "into": into });
    let fields = json!({ "origin": a.tx, "into": into });
    let r = send(
        backend,
        g,
        Req {
            tag: "history.cherryPick",
            cmd: "cherry-pick",
            args,
            fields,
            hashed: true,
            group: false,
        },
    )?;
    Ok(result_doc(&r, None))
}

pub fn revert(backend: &dyn Backend, g: &Globals, a: &RevertArgs) -> Result<Value, CoreError> {
    let branch = target_branch(backend, g)?;
    let args = json!({ "mergeId": a.merge, "branch": branch });
    let fields = json!({ "mergeId": a.merge, "branch": branch });
    let r = send(
        backend,
        g,
        Req {
            tag: "history.revert",
            cmd: "revert",
            args,
            fields,
            hashed: true,
            group: false,
        },
    )?;
    Ok(result_doc(&r, Some(("reverted", "undone"))))
}
