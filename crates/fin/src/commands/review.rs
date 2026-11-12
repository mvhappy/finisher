//! `fin review request|ls|show|refresh|comment|decide|merge|close` (5E plan Decision 24, spec
//! §8.6): the review workflow over the `review.*` RPCs. The server holds the items; these
//! commands only shape the requests. Every write that needs the item's snapshot (`decide`,
//! `merge`) sends it as `expect` from a fresh `review.get` unless `--expect` carries one, so a
//! change list that moved since the person looked is `REVIEW_STALE` (exit 4), never a surprise.
//! Review text (titles, comments, reasons) is content: the server never audits it, and these
//! commands print it back only in the item's own document.
//!
//! The review RPCs take no `txId`: `--idem-key` (or `FIN_CALL`) applies to `merge` only, as its
//! `idemKey`; the other writes are harmless to repeat (`request` answers the open item, `decide`
//! sets the same marks). `--dry-run` is refused: a review has no plan to preview beyond `show`.

use clap::{Args, Subcommand};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::history::key;
use super::{Globals, target_branch};
use crate::backend::Backend;
use crate::client::IN_FLIGHT_CODES;
use crate::output::usage;

#[derive(Debug, Clone, Subcommand)]
pub enum ReviewCmd {
    Request(RequestArgs),
    Ls(LsArgs),
    Show(IdArgs),
    Refresh(IdArgs),
    Comment(CommentArgs),
    Decide(DecideArgs),
    Merge(MergeArgs),
    Close(CloseArgs),
}

/// Ask for a review of a branch (the open one, when there is already one).
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestArgs {
    /// The branch the review would merge into (default: main).
    #[arg(long, value_name = "b")]
    #[serde(default)]
    pub into: Option<String>,
    /// What the change is, in a line (1 to 200 characters).
    #[arg(long, value_name = "t")]
    pub title: String,
    /// More about it (at most 8 KiB).
    #[arg(long, value_name = "s")]
    #[serde(default)]
    pub summary: Option<String>,
}

/// List the project's review items.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LsArgs {
    /// Only items in this state (open, changesRequested, merged, rejected, dismissed, superseded).
    #[arg(long, value_name = "s")]
    #[serde(default)]
    pub state: Option<String>,
    /// Only items of this kind (branchReview, mergeConflict, pluginSuggestion).
    #[arg(long, value_name = "k")]
    #[serde(default)]
    pub kind: Option<String>,
}

/// An item by id.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    /// The review item's id.
    #[arg(value_name = "id")]
    pub id: String,
}

/// Comment on a review item.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentArgs {
    /// The review item's id.
    #[arg(value_name = "id")]
    pub id: String,
    /// The comment (at most 16 KiB).
    #[arg(value_name = "text")]
    pub text: String,
    /// About this origin transaction of the change list.
    #[arg(long, value_name = "txId")]
    #[serde(default)]
    pub tx: Option<String>,
    /// About this time on the timeline (a rational, as the ops write it).
    #[arg(long, value_name = "t")]
    #[serde(default)]
    pub at: Option<String>,
}

/// Accept, reject or take back the marks on a change list, and/or send the branch back.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecideArgs {
    /// The review item's id.
    #[arg(value_name = "id")]
    pub id: String,
    /// Origin txIds to accept: one per flag, repeat the flag or separate with commas.
    #[arg(long, value_name = "txId", value_delimiter = ',')]
    #[serde(default)]
    pub accept: Vec<String>,
    /// Origin txIds to reject: a drop record keeps them out of every merge at once.
    #[arg(long, value_name = "txId", value_delimiter = ',')]
    #[serde(default)]
    pub reject: Vec<String>,
    /// Origin txIds to take the mark back from.
    #[arg(long, value_name = "txId", value_delimiter = ',')]
    #[serde(default)]
    pub pending: Vec<String>,
    /// Why (at most 2 KiB; kept with each mark).
    #[arg(long, value_name = "r")]
    #[serde(default)]
    pub reason: Option<String>,
    /// Hand the branch back for changes.
    #[arg(long = "request-changes")]
    #[serde(default, rename = "requestChanges")]
    pub request_changes: bool,
    /// The snapshot the decisions are about, as `item.snapshot` of `fin review show` (JSON);
    /// default: the item's snapshot, read now.
    #[arg(long, value_name = "json")]
    #[serde(default)]
    pub expect: Option<String>,
}

/// Merge what was reviewed.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeArgs {
    /// The review item's id.
    #[arg(value_name = "id")]
    pub id: String,
    /// Merge the undecided entries too (otherwise only the accepted ones merge). Under the
    /// project's `requireReview`, any undecided entry refuses the merge whatever this says.
    #[arg(long = "accept-pending")]
    #[serde(default, rename = "acceptPending")]
    pub accept_pending: bool,
    /// The snapshot that was reviewed (JSON, see `decide --expect`); default: read now.
    #[arg(long, value_name = "json")]
    #[serde(default)]
    pub expect: Option<String>,
}

/// Close a review item without merging it.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloseArgs {
    /// The review item's id.
    #[arg(value_name = "id")]
    pub id: String,
    /// Close it as rejected.
    #[arg(long, conflicts_with = "dismissed")]
    #[serde(default)]
    pub rejected: bool,
    /// Close it as dismissed.
    #[arg(long)]
    #[serde(default)]
    pub dismissed: bool,
}

/// The snapshot to send as `expect`: `--expect`, else the item's, from a fresh `review.get`.
fn expect(backend: &dyn Backend, id: &str, given: &Option<String>) -> Result<Value, CoreError> {
    if let Some(text) = given {
        return serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| {
                usage(
                    "--expect is not a snapshot",
                    json!({ "field": "expect" }),
                    "pass item.snapshot of `fin review show` as JSON, or leave --expect out to use the current one",
                )
            });
    }
    let view = backend.server_call("review.get", json!({ "itemId": id }))?;
    // An item with no snapshot (not a branchReview) gets a placeholder: the server checks the
    // item's kind and state before the snapshot and answers with its own error.
    Ok(match view["item"].get("snapshot") {
        Some(s) if s.is_object() => s.clone(),
        _ => json!({ "sourceSeq": 0, "targetSeq": 0, "plannedAt": 0 }),
    })
}

fn marks(a: &DecideArgs) -> Result<Vec<Value>, CoreError> {
    let mut out: Vec<Value> = Vec::new();
    for (decision, ids) in [
        ("accept", &a.accept),
        ("reject", &a.reject),
        ("pending", &a.pending),
    ] {
        for tx in ids {
            if out.iter().any(|m| m["txId"] == json!(tx)) {
                return Err(usage(
                    format!("txId {tx} is named twice"),
                    json!({ "txId": tx }),
                    "give each txId to one of --accept, --reject and --pending",
                ));
            }
            let mut m = json!({ "txId": tx, "decision": decision });
            if let Some(r) = &a.reason {
                m["reason"] = json!(r);
            }
            out.push(m);
        }
    }
    Ok(out)
}

/// The `review.decide` payload (without `expect`).
pub fn decide_payload(a: &DecideArgs) -> Result<Value, CoreError> {
    let decisions = marks(a)?;
    if decisions.is_empty() && !a.request_changes {
        return Err(usage(
            "decide needs --accept, --reject, --pending or --request-changes",
            json!({}),
            "e.g. fin review decide <id> --accept <txId> --reject <txId>",
        ));
    }
    let mut p = json!({ "itemId": a.id });
    if !decisions.is_empty() {
        p["decisions"] = json!(decisions);
    }
    if a.request_changes {
        p["requestChanges"] = match &a.reason {
            Some(r) => json!({ "reason": r }),
            None => json!({}),
        };
    }
    Ok(p)
}

/// A merge whose answer was lost names the key to repeat it with.
fn with_hint(mut e: CoreError, id: &str) -> CoreError {
    if e.code == "REVIEW_UNDECIDED" && e.details["reason"] == "nothingAccepted" {
        e.fix = Some(format!(
            "accept something first (fin review decide {id} --accept <txId>), or add --accept-pending to merge the undecided entries"
        ));
    }
    e
}

fn with_key(mut e: CoreError, key: &str) -> CoreError {
    if IN_FLIGHT_CODES.contains(&e.code.as_str()) {
        e.details["idemKey"] = json!(key);
        e.fix = Some(format!(
            "the merge may have gone through: run it again with --idem-key {key} (the server answers from what it did)"
        ));
    }
    e
}

pub fn run(backend: &dyn Backend, g: &Globals, c: &ReviewCmd) -> Result<Value, CoreError> {
    if g.dry_run {
        return Err(usage(
            "fin review takes no --dry-run",
            json!({ "flag": "dry-run" }),
            "`fin review show <id>` is the preview: it lists what a merge would copy",
        ));
    }
    match c {
        ReviewCmd::Request(a) => {
            let mut p = json!({ "source": target_branch(backend, g)?, "title": a.title });
            if let Some(into) = &a.into {
                p["into"] = json!(into);
            }
            if let Some(s) = &a.summary {
                p["summary"] = json!(s);
            }
            backend.project_call("review.request", p)
        }
        ReviewCmd::Ls(a) => {
            let mut p = json!({});
            if let Some(s) = &a.state {
                p["state"] = json!(s);
            }
            if let Some(k) = &a.kind {
                p["kind"] = json!(k);
            }
            backend.project_call("review.list", p)
        }
        ReviewCmd::Show(a) => backend.server_call("review.get", json!({ "itemId": a.id })),
        ReviewCmd::Refresh(a) => backend.server_call("review.refresh", json!({ "itemId": a.id })),
        ReviewCmd::Comment(a) => {
            let mut p = json!({ "itemId": a.id, "body": a.text });
            if let Some(t) = &a.tx {
                p["txId"] = json!(t);
            }
            if let Some(at) = &a.at {
                p["at"] = json!(at);
            }
            backend.server_call("review.comment", p)
        }
        ReviewCmd::Decide(a) => {
            let mut p = decide_payload(a)?;
            p["expect"] = expect(backend, &a.id, &a.expect)?;
            backend.server_call("review.decide", p)
        }
        ReviewCmd::Merge(a) => {
            let idem = key(g)?;
            let mut p = json!({ "itemId": a.id, "idemKey": idem });
            if a.accept_pending {
                p["acceptPending"] = json!(true);
            }
            p["expect"] = expect(backend, &a.id, &a.expect)?;
            backend
                .server_call("review.merge", p)
                .map_err(|e| with_hint(with_key(e, &idem), &a.id))
        }
        ReviewCmd::Close(a) => {
            let state = match (a.rejected, a.dismissed) {
                (true, false) => "rejected",
                (false, true) => "dismissed",
                _ => {
                    return Err(usage(
                        "close needs --rejected or --dismissed",
                        json!({}),
                        "fin review close <id> --rejected|--dismissed",
                    ));
                }
            };
            backend.server_call("review.close", json!({ "itemId": a.id, "state": state }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide(accept: &[&str], reject: &[&str], reason: Option<&str>, rc: bool) -> DecideArgs {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect();
        DecideArgs {
            id: "r1".into(),
            accept: v(accept),
            reject: v(reject),
            pending: vec![],
            reason: reason.map(String::from),
            request_changes: rc,
            expect: None,
        }
    }

    #[test]
    fn decide_payload_marks_each_tx_once() {
        let p = decide_payload(&decide(&["a"], &["b"], Some("no"), false)).unwrap();
        assert_eq!(
            p["decisions"],
            json!([
                { "txId": "a", "decision": "accept", "reason": "no" },
                { "txId": "b", "decision": "reject", "reason": "no" },
            ])
        );
        assert!(decide_payload(&decide(&["a"], &["a"], None, false)).is_err());
        assert!(decide_payload(&decide(&[], &[], None, false)).is_err());
        let rc = decide_payload(&decide(&[], &[], Some("redo"), true)).unwrap();
        assert_eq!(rc["requestChanges"], json!({ "reason": "redo" }));
        assert!(rc.get("decisions").is_none());
    }
}
