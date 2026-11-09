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
    send_flagged(backend, globals, req).map(|(answer, _)| answer)
}

/// [`send`], plus whether the answer is a plan only: `--dry-run` with a key that already
/// committed gets that commit back, which is not a dry run.
pub fn send_flagged(
    backend: &dyn Backend,
    globals: &Globals,
    req: Req<'_>,
) -> Result<(Value, bool), CoreError> {
    let tx_id = key(globals)?;
    let mut payload = req.fields;
    payload["txId"] = json!(tx_id);
    payload["author"] = json!(author());
    if globals.dry_run {
        payload["dryRun"] = json!(true);
    }
    // Always hashed: a retry with the key a lost reply names, but other args, is
    // `IDEM_KEY_REUSED`, never the earlier command's answer.
    if req.hashed {
        payload["argsHash"] = json!(args_hash(&[json!({ "cmd": req.cmd, "args": req.args })]));
    }
    let mut committed = false;
    if globals.idem_key.is_some() && !backend.dry_run_only() {
        // A retry whose record was set aside is reported, never sent again blindly.
        let first = if req.group {
            format!("{tx_id}-1")
        } else {
            tx_id.clone()
        };
        committed = lookup(backend, &first)?.is_some();
    }
    // A merge group's records are `{txId}-1`…; the key to retry with is still `tx_id`.
    let call = |p: Value| {
        if globals.dry_run {
            backend.project_call(req.tag, p)
        } else {
            backend.project_tx(req.tag, p, &tx_id)
        }
    };
    let answer = match call(payload.clone()) {
        // The answer was lost after the request was sent (the error names the key): retry once
        // with the same key, which the server answers from its log if it applied it. Not
        // `project recover` (unhashed): its dismiss and import run before the key's dedupe.
        Err(e)
            if req.hashed
                && IN_FLIGHT_CODES.contains(&e.code.as_str())
                && e.details["txId"] == tx_id =>
        {
            call(payload).map_err(|again| unanswered(e, again, &tx_id))
        }
        other => other,
    }?;
    Ok((answer, globals.dry_run && !committed))
}

/// The retry's error, unless it failed without reaching a server that answered (reconnect,
/// handshake, deadline): then the first error, which names the key to retry with.
fn unanswered(first: CoreError, again: CoreError, tx_id: &str) -> CoreError {
    let unreached = IN_FLIGHT_CODES.contains(&again.code.as_str())
        || matches!(
            again.code.as_str(),
            "SERVER_UNTRUSTED" | "UNAUTHENTICATED" | "PROTOCOL_MISMATCH"
        );
    if unreached && again.details["txId"] != tx_id {
        first
    } else {
        again
    }
}

/// Lists longer than this are cut to their first [`CAP`] entries; `<key>Total` has the count.
const CAP: usize = 20;

pub fn cap(doc: &mut Value, key: &str) {
    let Some(list) = doc.get_mut(key).and_then(Value::as_array_mut) else {
        return;
    };
    let total = list.len();
    if total > CAP {
        list.truncate(CAP);
        doc[format!("{key}Total")] = json!(total);
    }
}

/// An undo/redo refusal says whose stack it looked at, and how to look at another's.
fn with_author(mut e: CoreError, branch: &str) -> CoreError {
    if matches!(
        e.code.as_str(),
        "CONFLICT" | "NOTHING_TO_UNDO" | "NOTHING_TO_REDO"
    ) {
        let who = author();
        if let Some(map) = e.details.as_object_mut() {
            map.entry("author").or_insert(json!(who));
            map.entry("branch").or_insert(json!(branch));
        }
        let note = if crate::agent_session::active() {
            format!("an agent session's undo and redo act on its own edits on {branch} only")
        } else {
            format!(
                "undo and redo act on edits by author {who:?} on {branch}; set FIN_AUTHOR to act as another author"
            )
        };
        e.fix = Some(match e.fix.take() {
            Some(f) => format!("{f}; {note}"),
            None => note,
        });
    }
    e
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

/// `placements` (owner decision 2026-11-09: where each item a ripple restore or move put back
/// went; always present, capped) and, only when one was relocated, a one-line `notice`.
fn with_placements(doc: &mut Value, r: &Value) {
    let placements = r.get("placements").cloned().unwrap_or_else(|| json!([]));
    if let Some(n) = notice(&placements) {
        doc["notice"] = json!(n);
    }
    doc["placements"] = placements;
    cap(doc, "placements");
}

/// What a person reads about the relocated items: where each went and why; `None` when every
/// item went back where it was.
pub fn notice(placements: &Value) -> Option<String> {
    let lines: Vec<String> = placements
        .as_array()?
        .iter()
        .filter(|p| p["outcome"] == "relocated")
        .map(|p| {
            let l = &p["landed"];
            let place = match (l["after"].as_str(), l["before"].as_str()) {
                (Some(a), _) => format!("after {a}"),
                (None, Some(b)) => format!("before {b}"),
                (None, None) => format!("at {}", l["at"].as_str().unwrap_or("?")),
            };
            let frame = l["frame"]
                .as_i64()
                .map_or_else(String::new, |f| format!(", frame {f}"));
            let track = l["track"].as_str().unwrap_or("?");
            let why: Vec<String> = p["causes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    let by = c["author"]
                        .as_str()
                        .map_or_else(String::new, |a| format!(" ({a})"));
                    format!(
                        "{} was {} by {}{by}",
                        c["neighbour"].as_str().unwrap_or("?"),
                        c["change"].as_str().unwrap_or("changed"),
                        c["txId"].as_str().unwrap_or("?"),
                    )
                })
                .collect();
            let why = if why.is_empty() {
                String::new()
            } else {
                format!(": {}", why.join(", "))
            };
            let item = p["item"].as_str().unwrap_or("?");
            format!("{item} went back {place} on {track}{frame}{why}")
        })
        .collect();
    // A person reads a few; the rest are in `placements`.
    const SHOWN: usize = 3;
    let more = lines.len().saturating_sub(SHOWN);
    let mut text = lines.into_iter().take(SHOWN).collect::<Vec<_>>().join("; ");
    if more > 0 {
        text.push_str(&format!("; and {more} more (placements)"));
    }
    (!text.is_empty()).then(|| format!("relocated: {text}"))
}

/// `{[<key>: [...],] rev, txId, noOp?, dryRun?, placements, notice?}` of a `HistoryResult`.
/// `rev` is null when the operation committed nothing (`noOp` says why).
fn result_doc(r: &Value, named: Option<(&str, &str)>) -> Value {
    let (rev, tx_id) = last_of(&r["committed"]);
    let mut doc = json!({ "rev": rev, "txId": tx_id });
    with_placements(&mut doc, r);
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
    if let Some(to) = &a.to
        && !to.is_empty()
        && to.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(usage(
            "--to takes a checkpoint name; seqs are not accepted",
            json!({ "to": to }),
            "name a point with `fin checkpoint <name>` first, then `fin undo --to <name>`",
        ));
    }
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
    )
    .map_err(|e| with_author(e, &branch))?;
    let mut doc = result_doc(&r, Some(("undone", "undone")));
    cap(&mut doc, "undone");
    Ok(doc)
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
    )
    .map_err(|e| with_author(e, &branch))?;
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
    let (r, dry) = send_flagged(
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
    Ok(record_doc(&r, dry))
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
    let mut doc = json!({ "merge": merge });
    with_placements(&mut doc["merge"], report);
    for key in ["merged", "skipped", "excluded"] {
        cap(&mut doc["merge"], key);
    }
    doc
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Owner decision 2026-11-09: a notice only when an item was relocated; the placements
    /// always.
    #[test]
    fn notice_only_when_relocated() {
        let clean = json!({ "committed": [], "placements": [{ "txId": "u1", "opIndex": 0,
            "item": "c2", "outcome": "clean" }] });
        let doc = result_doc(&clean, None);
        assert!(doc.get("notice").is_none(), "{doc}");
        assert_eq!(doc["placements"][0]["outcome"], "clean");
        let none = result_doc(&json!({ "committed": [] }), None);
        assert_eq!(none["placements"], json!([]));
        let moved = json!({ "committed": [], "placements": [{ "txId": "u1", "opIndex": 0,
            "item": "c2", "outcome": "relocated",
            "landed": { "track": "t1", "at": "30", "frame": 900, "after": "c1" },
            "causes": [{ "neighbour": "c1", "change": "moved", "txId": "b1", "author": "bob" }] }] });
        let doc = result_doc(&moved, None);
        assert_eq!(
            doc["notice"],
            "relocated: c2 went back after c1 on t1, frame 900: c1 was moved by b1 (bob)"
        );
        let merged = merge_doc(&json!({ "committed": [], "placements": moved["placements"] }));
        assert!(
            merged["merge"]["notice"]
                .as_str()
                .unwrap()
                .starts_with("relocated: c2")
        );
    }

    #[test]
    fn long_lists_are_cut_with_a_total() {
        let mut doc = json!({ "a": (0..25).collect::<Vec<_>>(), "b": [1, 2] });
        cap(&mut doc, "a");
        cap(&mut doc, "b");
        assert_eq!(doc["a"].as_array().unwrap().len(), CAP);
        assert_eq!(doc["aTotal"], 25);
        assert!(doc.get("bTotal").is_none());
    }
}
