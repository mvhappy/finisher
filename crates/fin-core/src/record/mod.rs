//! Commit records: one committed transaction as stored and exported (sub-project 4 spec §3.3,
//! §3.4, §4.2). A record is serialised once at commit; those exact bytes are hashed, stored and
//! exported, and never re-encoded.
//!
//! A record's kind and its kind fields (4B, [`Link`]) are flat, optional fields right after
//! `kind`, each omitted when absent, so an `edit` record is byte-identical to what 4A wrote.

mod link;

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::CoreError;
use crate::ops::Op;
use crate::tx::Tx;

pub use link::{Link, MAX_DROP_IDS, kind_name};

/// Version of the record line format written by this code for every kind but `drop` (4B:
/// record kinds); a `drop` record is format 3 ([`record_format`]).
pub const RECORD_FORMAT: u32 = 2;
/// The oldest reader that can read every record of a kind other than `drop` this code writes.
/// Files holding only `edit` records stay readable by version 1
/// (`fin_store::segment::version_for`).
pub const MIN_READER: u32 = 2;

/// The record format a record of `kind` is written at and the oldest reader that reads it:
/// `(3, 3)` for `drop` (plan 5E Decision 4), [`RECORD_FORMAT`] and [`MIN_READER`] otherwise.
/// Every other kind stays byte-identical to what 4B wrote.
pub fn record_format(kind: RecordKind) -> (u32, u32) {
    match kind {
        RecordKind::Drop => (3, 3),
        _ => (RECORD_FORMAT, MIN_READER),
    }
}

/// What a record does (spec §4.2). Sub-project 4A writes only edits; 4B adds the rest up to
/// `branchSet`; 5E adds `drop` (durable rejections, sub-project 5 spec §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum RecordKind {
    Edit,
    Undo,
    Redo,
    Merge,
    CherryPick,
    Revert,
    Checkpoint,
    Branch,
    BranchSet,
    Drop,
}

impl RecordKind {
    /// A change its author made on the branch: `edit`, `undo`, `redo`, `cherryPick`, `revert`.
    /// A `merge` copy belongs to its merge group instead.
    pub fn is_content(self) -> bool {
        matches!(
            self,
            RecordKind::Edit
                | RecordKind::Undo
                | RecordKind::Redo
                | RecordKind::CherryPick
                | RecordKind::Revert
        )
    }

    /// Whether the record carries ops: not `checkpoint`, `branch`, `branchSet` or `drop`.
    pub fn has_ops(self) -> bool {
        !matches!(
            self,
            RecordKind::Checkpoint | RecordKind::Branch | RecordKind::BranchSet | RecordKind::Drop
        )
    }

    /// Whether a sub-project 4A reader knows the kind: only `edit`.
    pub fn is_4a(self) -> bool {
        self == RecordKind::Edit
    }
}

/// One committed transaction. Fields serialise in declaration order.
///
/// - `intent`: the `Tx` as applied (after any rebase), still holding `$placeholders`.
/// - `resolved`: `intent.ops` with `bindings` substituted ([`resolve_ops`]).
/// - `inverse`: the ops that undo it, with bound ids.
/// - `parent_hash`: the previous record's hash on the branch, `None` for the first.
/// - `undoes` … `undrops`: the kind's fields, read as a [`Link`] ([`Record::link`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub tx_id: String,
    pub seq: u64,
    pub kind: RecordKind,
    /// `undo`: the units it undoes, newest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undoes: Option<Vec<String>>,
    /// `redo`: the `undo` record it reverses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redoes: Option<String>,
    /// `merge`: the merge group (the caller's txId).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_id: Option<String>,
    /// `merge`, `cherryPick`: the source branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `merge`, `cherryPick`: the txId of the copied record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// `cherryPick`: the txHash of the copied record (its origin root), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_hash: Option<String>,
    /// `revert`: the merge group it reverts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverts: Option<String>,
    /// `checkpoint`, `branch`, `branchSet`: a checkpoint or branch name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `branch`, `branchSet`: the branch id (`br<n>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_id: Option<String>,
    /// `branch`: the parent's seq the branch forks at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_seq: Option<u64>,
    /// `branch`: the parent's txHash at `forkSeq`; absent when `forkSeq` is 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_hash: Option<String>,
    /// `branch`: the lease epoch the record was committed at (the fence epoch). Two `branch`
    /// records that make the same id (two servers across a takeover) are ordered by it, then
    /// by txHash (plan 4B Decision 33).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    /// `branchSet`: archive (`true`) or restore (`false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// `cherryPick`, `checkpoint`, `branch`, `branchSet` written by a recovery: the txHash of
    /// the quarantined record this one stands for (plan 4B Decisions 5, 27). Only recovery
    /// writes it, so only it marks a quarantine row recovered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovers: Option<String>,
    /// `drop`: the origin roots it drops for this branch, sorted, deduplicated, absent when
    /// none (plan 5E Decision 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drops: Option<Vec<String>>,
    /// `drop`: the origin roots it makes candidates again, as `drops`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undrops: Option<Vec<String>>,
    pub schema_version: u32,
    pub parent_hash: Option<String>,
    pub author: String,
    pub committed_at: i64,
    pub intent: Tx,
    pub resolved: Vec<Op>,
    pub inverse: Vec<Op>,
    pub bindings: BTreeMap<String, String>,
}

impl Record {
    /// An `edit` record (no kind fields). [`Record::set_link`] makes it another kind.
    #[allow(clippy::too_many_arguments)]
    pub fn new_edit(
        tx_id: String,
        seq: u64,
        schema_version: u32,
        parent_hash: Option<String>,
        author: String,
        committed_at: i64,
        intent: Tx,
        resolved: Vec<Op>,
        inverse: Vec<Op>,
        bindings: BTreeMap<String, String>,
    ) -> Record {
        Record {
            tx_id,
            seq,
            kind: RecordKind::Edit,
            undoes: None,
            redoes: None,
            merge_id: None,
            from: None,
            origin: None,
            origin_hash: None,
            reverts: None,
            name: None,
            branch_id: None,
            fork_seq: None,
            fork_hash: None,
            epoch: None,
            archived: None,
            recovers: None,
            drops: None,
            undrops: None,
            schema_version,
            parent_hash,
            author,
            committed_at,
            intent,
            resolved,
            inverse,
            bindings,
        }
    }
}

/// The record's bytes: compact JSON, no trailing newline. These are the bytes that are hashed.
pub fn encode_record(r: &Record) -> Vec<u8> {
    let bytes = serde_json::to_vec(r)
        .expect("a Record holds only structs, strings, numbers and string-keyed maps, which always serialise");
    // A non-finite float serialises as `null` and never decodes: the ops' value checks keep
    // them out of every record.
    debug_assert!(
        serde_json::from_slice::<Record>(&bytes).is_ok(),
        "an encoded record decodes (no NaN or infinity in its ops)"
    );
    bytes
}

/// `ops` with every `$placeholder` reference replaced by its binding. A placeholder without a
/// binding is `UNBOUND_PLACEHOLDER` (`details {placeholder, opIndex}`).
pub fn resolve_ops(ops: &[Op], bindings: &BTreeMap<String, String>) -> Result<Vec<Op>, CoreError> {
    ops.iter()
        .enumerate()
        .map(|(i, op)| {
            let mut op = op.clone();
            op.try_for_each_ref(|r| {
                if !r.starts_with('$') {
                    return Ok(());
                }
                let Some(id) = bindings.get(r.as_str()) else {
                    return Err(CoreError::new(
                        "UNBOUND_PLACEHOLDER",
                        format!("placeholder {r} has no recorded binding"),
                    )
                    .with_details(json!({ "placeholder": r, "opIndex": i })));
                };
                *r = id.clone();
                Ok(())
            })?;
            Ok(op)
        })
        .collect()
}

/// `INVALID_FORMAT`: stored data that does not have the shape its version promises.
pub(crate) fn invalid_format(what: &str, why: impl std::fmt::Display) -> CoreError {
    CoreError::new(
        "INVALID_FORMAT",
        format!("the {what} does not decode: {why}"),
    )
    .with_details(json!({ "kind": what, "reason": why.to_string() }))
}

/// Decodes an already upcast record value.
pub(crate) fn decode_record(v: Value) -> Result<Record, CoreError> {
    serde_json::from_value(v).map_err(|e| invalid_format("record", e))
}
