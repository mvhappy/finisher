//! Checkpoints (spec §4.5; plan 4B Decisions 11, 26 and 30, Task 15): `Store::checkpoint`
//! appends a `checkpoint` record (no ops, never part of the merged state or of the undo
//! stacks) through the one write path; `Store::find_checkpoint` finds one by name on a
//! branch's lineage, for `undo --to` and `branch new --from <b>@<checkpoint>`.
//!
//! Names follow the branch-name rules, must not be all digits (so `<b>@<n>` stays a seq), and
//! are unique per branch, case-insensitively, among the checkpoints the branch itself holds.
//! A lookup reads the lineage newest first, so a branch's own checkpoint hides an ancestor's
//! of the same name; the ancestor's is still `<ancestor>@<name>`. Checkpoint names are kept
//! in `record_links.name`, derived from the log like the rest of that table, so a follower
//! and a rebuild find the same ones.

use fin_core::branch::validate_checkpoint_name;
use fin_core::{CommittedTx, CoreError, Link};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use crate::branch::create::{check_key, key_reused, metadata_draft};
use crate::branch::{index, lineage};
use crate::commit::{check_args_hash, same_args_hash};
use crate::db::{self, to_i64, to_u64};
use crate::write::{WriteCtx, writable};
use crate::{Store, StoreError};

/// [`Store::checkpoint`]'s request.
///
/// - `tx_id`: the idempotency key, the record's txId (Decision 26);
/// - `author`: the record's author;
/// - `branch`: a branch reference;
/// - `name`: the checkpoint's name (module docs);
/// - `dry_run`: plan against the local database and keep nothing, on a follower too;
/// - `args_hash`: the command input's hash, stored on the record's intent; a retry of the
///   txId with another hash is `IDEM_KEY_REUSED`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckpointReq {
    pub tx_id: String,
    pub author: String,
    pub branch: String,
    pub name: String,
    #[serde(default)]
    pub dry_run: bool,
    pub args_hash: Option<String>,
}

impl Store {
    /// Appends a `checkpoint` record named `name` at the head of `branch` and returns it.
    ///
    /// Errors, in order: `INVALID_OP` (empty txId, bad `argsHash`), `INVALID_NAME`; a retried
    /// txId answers with its record (`IDEM_KEY_REUSED` when it holds another kind or another
    /// `argsHash`); `PROJECT_READ_ONLY`; `IDEM_KEY_REUSED` when the txId is a merge group's
    /// id; `BRANCH_NOT_FOUND`, `BRANCH_ARCHIVED`; `NAME_TAKEN` when the branch holds a
    /// checkpoint of that name (`details {name, branch, seq, txId}`). A dry run plans it
    /// against the local database and keeps nothing, on a follower too.
    pub fn checkpoint(&mut self, r: CheckpointReq) -> Result<CommittedTx, StoreError> {
        check_key(&r.tx_id)?;
        check_args_hash(r.args_hash.as_deref(), &r.tx_id)?;
        validate_checkpoint_name(&r.name)?;
        loop {
            if let Some(c) = self.write(&r.tx_id, r.dry_run, |ctx| plan(ctx, &r))? {
                return Ok(c);
            }
            // Deduped; gone again (an import rewound it since): the checkpoint runs again.
            if let Some(existing) = db::find_tx(&self.conn, &r.tx_id)? {
                return match existing.link.as_deref() {
                    Some(Link::Checkpoint { .. }) => {
                        same_args_hash(r.args_hash.as_deref(), existing)
                    }
                    _ => Err(key_reused(&existing)),
                };
            }
        }
    }

    /// The checkpoint `name` on `branch`'s lineage (case-insensitively), newest first: the
    /// branch holding it and its seq. `BRANCH_NOT_FOUND`; `NOT_FOUND` (`details.kind`
    /// `checkpoint`).
    pub fn find_checkpoint(&self, branch: &str, name: &str) -> Result<(String, u64), StoreError> {
        let id = index::resolve(&self.conn, branch)?.id;
        find(&self.conn, &id, name)
    }
}

fn plan(ctx: &mut WriteCtx<'_>, r: &CheckpointReq) -> Result<CommittedTx, StoreError> {
    let txn = ctx.txn;
    super::merge_id_taken(txn, &r.tx_id)?;
    let branch = writable(txn, &r.branch)?.id;
    if let Some((seq, tx_id)) = own(txn, &branch, &r.name)? {
        return Err(CoreError::new(
            "NAME_TAKEN",
            format!(
                "branch {branch} has a checkpoint {:?} already (seq {seq})",
                r.name
            ),
        )
        .with_details(json!({ "name": r.name, "branch": branch, "seq": seq, "txId": tx_id }))
        .with_fix("choose another checkpoint name")
        .into());
    }
    let link = Link::Checkpoint {
        name: r.name.clone(),
        recovers: None,
    };
    let mut draft = metadata_draft(ctx, &branch, &r.tx_id, &r.author, link)?;
    draft.intent.args_hash = r.args_hash.clone();
    ctx.append(draft)
}

/// The checkpoint of `branch`'s own named `name` (case-insensitively): its seq and txId.
fn own(conn: &Connection, branch: &str, name: &str) -> Result<Option<(u64, String)>, StoreError> {
    let row: Option<(i64, String)> = conn
        .prepare_cached(
            "SELECT seq, tx_id FROM record_links
             WHERE kind = 'checkpoint' AND branch = ?1 AND name = ?2 COLLATE NOCASE
             ORDER BY seq DESC LIMIT 1",
        )?
        .query_row(params![branch, name], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    row.map(|(seq, tx)| Ok((to_u64(seq)?, tx))).transpose()
}

/// [`Store::find_checkpoint`] over `conn` (inside a write, its transaction), `branch` an id.
pub(crate) fn find(
    conn: &Connection,
    branch: &str,
    name: &str,
) -> Result<(String, u64), StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq FROM record_links
         WHERE kind = 'checkpoint' AND branch = ?1 AND seq > ?2 AND (?3 IS NULL OR seq <= ?3)
           AND name = ?4 COLLATE NOCASE
         ORDER BY seq DESC LIMIT 1",
    )?;
    for seg in lineage::lineage(conn, branch)? {
        let through = seg.through.map(to_i64).transpose()?;
        let seq: Option<i64> = stmt
            .query_row(
                params![seg.branch, to_i64(seg.after)?, through, name],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(seq) = seq {
            return Ok((seg.branch, to_u64(seq)?));
        }
    }
    Err(CoreError::new(
        "NOT_FOUND",
        format!("no checkpoint {name:?} on branch {branch} or its history"),
    )
    .with_details(json!({ "kind": "checkpoint", "name": name, "branch": branch }))
    .with_fix("check the name and the branch (--branch); a checkpoint made on a branch after another forked from it is not in that branch's history")
    .into())
}
