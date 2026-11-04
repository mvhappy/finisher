//! Commit records: one committed transaction as stored and exported (sub-project 4 spec §3.3,
//! §3.4). A record is serialised once at commit; those exact bytes are hashed, stored and
//! exported, and never re-encoded.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::CoreError;
use crate::ops::Op;
use crate::tx::Tx;

/// Version of the record line format written by this code.
pub const RECORD_FORMAT: u32 = 1;
/// The oldest reader that can read what this code writes.
pub const MIN_READER: u32 = 1;

/// What a record does. Sub-project 4A writes only edits; 4B adds kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum RecordKind {
    Edit,
}

/// One committed transaction. Fields serialise in declaration order.
///
/// - `intent`: the `Tx` as applied (after any rebase), still holding `$placeholders`.
/// - `resolved`: `intent.ops` with `bindings` substituted ([`resolve_ops`]).
/// - `inverse`: the ops that undo it, with bound ids.
/// - `parent_hash`: the previous record's hash on the branch, `None` for the first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub tx_id: String,
    pub seq: u64,
    pub kind: RecordKind,
    pub schema_version: u32,
    pub parent_hash: Option<String>,
    pub author: String,
    pub committed_at: i64,
    pub intent: Tx,
    pub resolved: Vec<Op>,
    pub inverse: Vec<Op>,
    pub bindings: BTreeMap<String, String>,
}

/// The record's bytes: compact JSON, no trailing newline. These are the bytes that are hashed.
pub fn encode_record(r: &Record) -> Vec<u8> {
    serde_json::to_vec(r)
        .expect("a Record holds only structs, strings, numbers and string-keyed maps, which always serialise")
}

/// `ops` with every `$placeholder` reference replaced by its binding. A placeholder without a
/// binding is `UNBOUND_PLACEHOLDER` (`details {placeholder, opIndex}`).
pub fn resolve_ops(ops: &[Op], bindings: &BTreeMap<String, String>) -> Result<Vec<Op>, CoreError> {
    ops.iter()
        .enumerate()
        .map(|(i, op)| {
            let mut op = op.clone();
            for r in op.refs_mut() {
                if !r.starts_with('$') {
                    continue;
                }
                let Some(id) = bindings.get(r.as_str()) else {
                    return Err(CoreError::new(
                        "UNBOUND_PLACEHOLDER",
                        format!("placeholder {r} has no recorded binding"),
                    )
                    .with_details(json!({ "placeholder": r, "opIndex": i })));
                };
                *r = id.clone();
            }
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
