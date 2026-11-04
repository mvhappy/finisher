//! What the DB holds for one branch, as chain resolution sees it ([`Known`]).

use std::collections::HashMap;

use fin_core::CoreError;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use crate::StoreError;
use crate::chain::Known;
use crate::db::{to_i64, to_u64};
use crate::storage_kind::StorageKind;

/// One unexported row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TailRow {
    pub(super) seq: u64,
    pub(super) tx_id: String,
    pub(super) tx_hash: String,
    /// The lease epoch it was committed at.
    pub(super) epoch: u64,
}

/// The branch's DB line: the highest exported row and the unexported rows after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Line {
    /// `(seq, txHash)` of the highest exported row.
    pub(super) head: Option<(u64, String)>,
    pub(super) tail: Vec<TailRow>,
}

pub(super) fn broken(branch: &str, why: String) -> StoreError {
    CoreError::new(
        "DB_ERROR",
        format!("cannot import branch {branch:?}: {why}"),
    )
    .with_details(json!({ "branch": branch }))
    .into()
}

/// Reads the line of `branch`, checking that the exported rows are a seq prefix and the tail
/// is consecutive.
pub(super) fn line(conn: &Connection, branch: &str) -> Result<Line, StoreError> {
    let head: Option<(i64, String)> = conn
        .query_row(
            "SELECT seq, tx_hash FROM txs WHERE branch = ?1 AND exported = 1
             ORDER BY seq DESC LIMIT 1",
            params![branch],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let head = head
        .map(|(s, h)| Ok::<_, StoreError>((to_u64(s)?, h)))
        .transpose()?;
    let mut stmt = conn.prepare(
        "SELECT seq, tx_id, tx_hash, epoch FROM txs WHERE branch = ?1 AND exported = 0
         ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![branch], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let mut tail: Vec<TailRow> = vec![];
    for row in rows {
        let (seq, tx_id, tx_hash, epoch) = row?;
        let seq = to_u64(seq)?;
        let after = tail.last().map(|t| t.seq).or(head.as_ref().map(|h| h.0));
        if after.is_some_and(|a| a + 1 != seq) {
            return Err(broken(
                branch,
                format!(
                    "exported records are not a seq prefix (unexported seq {seq} after {after:?})"
                ),
            ));
        }
        tail.push(TailRow {
            seq,
            tx_id,
            tx_hash,
            epoch: to_u64(epoch)?,
        });
    }
    Ok(Line { head, tail })
}

/// [`Known`] for `branch` from its `line` and `all_tx` (every record in the DB: `txId` is
/// unique across branches, so a folder record reusing another branch's `txId` must be a
/// `TxIdReused` fork, not an insert that fails).
pub(super) fn build(
    conn: &Connection,
    branch: &str,
    line: &Line,
    all_tx: HashMap<String, String>,
    holds_lease: bool,
    kind: StorageKind,
) -> Result<Known, StoreError> {
    let last_hash = match &line.head {
        Some((seq, _)) => segment_ending_at(conn, branch, *seq)?,
        None => None,
    };
    Ok(Known {
        head: line.head.as_ref().map(|h| h.1.clone()),
        last_hash,
        known_tx: all_tx,
        tail: line
            .tail
            .iter()
            .map(|t| (t.tx_id.clone(), t.tx_hash.clone()))
            .collect(),
        tail_epoch: line.tail.iter().map(|t| t.epoch).max().unwrap_or(0),
        holds_lease,
        kind,
    })
}

/// A known segment (exported or imported) whose last record is `seq`; packs first, as for
/// the export's `prevSegmentHash`.
pub(super) fn segment_ending_at(
    conn: &Connection,
    branch: &str,
    seq: u64,
) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT hash FROM segments WHERE branch = ?1 AND last_seq = ?2
             AND state IN ('exported', 'imported') ORDER BY pack DESC, hash LIMIT 1",
            params![branch, to_i64(seq)?],
            |r| r.get(0),
        )
        .optional()?)
}
