//! `fin log [<txId>]` (review batch L): the branch's history a page at a time, newest first, from
//! `history.log`; with a txId, that one record from `tx.get`. Each record is summarised (kind,
//! author, the branch holding it, op and change counts, what it links to), lists cut to
//! [`LIST_CAP`] with `<key>Total`, so one page stays small however large its records are.

use clap::Args;
use fin_core::{CommittedTx, CoreError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::addr;
use crate::backend::Backend;
use crate::output::usage;

/// Records a page holds without `--limit`.
pub const DEFAULT_LIMIT: u64 = 20;
/// The largest `--limit` (`history.log`'s `HISTORY_LOG_MAX`).
pub const MAX_LIMIT: u64 = 100;
/// Longer lists in a record (undone units, dropped ids, ops, changes, shifts, bindings) are cut
/// to this, with `<key>Total`: the same 20 as `undo` and `merge` (spec §3.10).
pub const LIST_CAP: usize = 20;

/// The branch's records, newest first; with a txId, that record's ops and changes.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogArgs {
    /// One record by txId (or a merge's records by mergeId: `<mergeId>-1`, `-2`, ...).
    #[arg(value_name = "txId")]
    #[serde(default, rename = "txId")]
    pub tx: Option<String>,
    /// Only records below this rev (the previous page's `next` names it).
    #[arg(long, value_name = "rev", conflicts_with = "tx")]
    #[serde(default)]
    pub before: Option<u64>,
    /// Records to list (default 20, at most 100).
    #[arg(long, value_name = "n", conflicts_with = "tx")]
    #[serde(default)]
    pub limit: Option<u64>,
}

/// Cuts `doc[key]` to [`LIST_CAP`] entries, with `<key>Total` when it was longer.
fn cap(doc: &mut Value, key: &str) {
    if let Some(list) = doc.get_mut(key).and_then(Value::as_array_mut)
        && list.len() > LIST_CAP
    {
        let total = list.len();
        list.truncate(LIST_CAP);
        doc[format!("{key}Total")] = json!(total);
    }
}

/// One record's summary: `{rev, txId, kind, author, branch, ops, changes, ...link}` where the
/// link's fields (`mergeId`, `from`, `origin`, `undoes`, `reverts`, `name`, ...) are inlined
/// and its lists capped.
fn summary(c: &CommittedTx) -> Value {
    let mut out = json!({
        "rev": c.rev, "txId": c.tx.tx_id, "kind": "edit", "author": c.tx.author,
        "branch": c.tx.branch, "ops": c.tx.ops.len(), "changes": c.diff.changes.len(),
    });
    if let Some(link) = &c.link
        && let Ok(Value::Object(fields)) = serde_json::to_value(link.as_ref())
    {
        for (k, v) in fields {
            out[k] = v;
        }
    }
    if let Some(via) = &c.tx.via {
        out["via"] = json!(via);
    }
    for key in ["undoes", "drops", "undrops"] {
        cap(&mut out, key);
    }
    out
}

pub fn run(backend: &dyn Backend, g: &Globals, a: &LogArgs) -> Result<Value, CoreError> {
    if let Some(tx) = &a.tx {
        return show(backend, tx);
    }
    let limit = a.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(usage(
            format!("--limit {limit} is out of range"),
            json!({ "field": "limit", "min": 1, "max": MAX_LIMIT }),
            format!("pass --limit from 1 to {MAX_LIMIT}"),
        ));
    }
    if a.before == Some(0) {
        return Err(usage(
            "--before 0 lists nothing",
            json!({ "field": "before", "min": 1 }),
            "pass --before <rev> from a page's next (revs start at 1)",
        ));
    }
    let branch = target_branch(backend, g)?;
    let mut payload = json!({ "branch": branch, "limit": limit });
    if let Some(b) = a.before {
        payload["before"] = json!(b);
    }
    let answer = backend.project_call("history.log", payload)?;
    let records: Vec<CommittedTx> = serde_json::from_value(answer["records"].clone())
        .map_err(|e| CoreError::new("INTERNAL", format!("history.log answered badly: {e}")))?;
    let first = records.first().map(|c| c.rev);
    let list: Vec<Value> = records.iter().rev().map(summary).collect();
    // The next page: everything below this page's oldest record, while there is any.
    let next = first.filter(|&r| r > 1).map(|r| {
        let mut cmd = format!("log --before {r}");
        if a.limit.is_some() {
            cmd.push_str(&format!(" --limit {limit}"));
        }
        crate::follow::cmd(&cmd)
    });
    Ok(json!({ "branch": branch, "head": answer["head"], "records": list, "next": next }))
}

/// `fin log <txId>`: the record with its ops and diff (each list capped), or where it was set
/// aside; `NOT_FOUND` when the server never saw it.
fn show(backend: &dyn Backend, tx_id: &str) -> Result<Value, CoreError> {
    let answer = backend.project_call("tx.get", json!({ "txId": tx_id }))?;
    if answer.is_null() {
        return Err(
            addr::not_found("transaction", tx_id, Vec::new()).with_fix(format!(
                "`{}` lists the branch's txIds (a merge's are <mergeId>-1, -2, ...)",
                crate::follow::cmd("log")
            )),
        );
    }
    if let Some(aside) = answer.get("setAside") {
        return Ok(json!({ "txId": tx_id, "setAside": aside }));
    }
    let c: CommittedTx = serde_json::from_value(answer.clone())
        .map_err(|e| CoreError::new("INTERNAL", format!("tx.get answered badly: {e}")))?;
    let mut tx = summary(&c);
    tx["ops"] = json!(c.tx.ops);
    tx["changes"] = json!(c.diff.changes);
    tx["shifts"] = json!(c.diff.shifts);
    for key in ["ops", "changes", "shifts"] {
        cap(&mut tx, key);
    }
    // Bindings (placeholder → id) in order, the first [`LIST_CAP`] of them, with the total.
    let bindings: serde_json::Map<String, Value> = c
        .bindings
        .iter()
        .take(LIST_CAP)
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    tx["bindings"] = Value::Object(bindings);
    if c.bindings.len() > LIST_CAP {
        tx["bindingsTotal"] = json!(c.bindings.len());
    }
    Ok(json!({ "tx": tx }))
}
