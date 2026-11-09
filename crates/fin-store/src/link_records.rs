//! Records as their links say (plan 4B Task 20): the `merged` state's reads
//! (`fin_core::history::left_to_merge`) need only each record's kind and links (origins, merge
//! groups, the units an undo names), which `txs` and `record_links` hold, so a write or an
//! import pass settles it without parsing records. What these reads give is a [`LinkRec`]:
//! a record with its kind, links, branch and seq only. Its author, intent, ops, inverses,
//! bindings and diff are empty, so it is only for readers of links.
//!
//! `derived::compute` (verify's reference) still parses records: it must follow from the log
//! alone, `record_links` included in what verify checks.

use std::collections::BTreeMap;

use fin_core::history::LogRec;
use fin_core::{Diff, Record, RecordKind, SCHEMA_VERSION, Tx};
use rusqlite::{Connection, Row, params};

use crate::StoreError;
use crate::db::{self, to_i64, to_u64};

const SELECT: &str = "SELECT t.branch, t.seq, t.tx_id, t.kind, l.tx_id IS NOT NULL, l.origin,
        l.origin_hash, l.from_branch, l.merge_id, l.targets
 FROM txs t LEFT JOIN record_links l ON l.tx_id = t.tx_id";

type Cols = (
    String,
    i64,
    String,
    String,
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn cols(r: &Row<'_>) -> rusqlite::Result<Cols> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
    ))
}

/// A record as its link row says: kind, links (`origin`, `originHash`, `from`, `mergeId`,
/// `undoes`, `redoes`, `reverts`), branch and seq. Everything else is empty (no author, intent,
/// ops, inverses, bindings or diff), so it is unwrapped only where links are all that is read
/// ([`LinkRec::links_only`]).
#[derive(Debug, Clone)]
pub(crate) struct LinkRec(LogRec);

impl LinkRec {
    /// The record, for a reader of kinds and links only (the merged state's
    /// `left_to_merge`); never for one that reads its author, ops or diff.
    pub(crate) fn links_only(self) -> LogRec {
        self.0
    }
}

/// `branch`'s rows with `after < seq <= through`, oldest first.
pub(crate) fn records(
    conn: &Connection,
    branch: &str,
    after: u64,
    through: u64,
) -> Result<Vec<LinkRec>, StoreError> {
    let rows: Vec<Cols> = conn
        .prepare_cached(&format!(
            "{SELECT} WHERE t.branch = ?1 AND t.seq > ?2 AND t.seq <= ?3 ORDER BY t.seq"
        ))?
        .query_map(params![branch, to_i64(after)?, to_i64(through)?], cols)?
        .collect::<Result<_, _>>()?;
    rows.into_iter().map(|c| record(conn, c)).collect()
}

/// The row with `tx_id`, if any.
pub(crate) fn find(conn: &Connection, tx_id: &str) -> Result<Option<LinkRec>, StoreError> {
    let rows: Vec<Cols> = conn
        .prepare_cached(&format!("{SELECT} WHERE t.tx_id = ?1"))?
        .query_map(params![tx_id], cols)?
        .collect::<Result<_, _>>()?;
    rows.into_iter().next().map(|c| record(conn, c)).transpose()
}

/// One row as a record of its kind and links. A non-edit row without its link row (never on a
/// consistent DB) is parsed whole.
fn record(conn: &Connection, c: Cols) -> Result<LinkRec, StoreError> {
    let (branch, seq, tx_id, kind, linked, origin, origin_hash, from, merge_id, targets) = c;
    let seq = to_u64(seq)?;
    let kind: RecordKind = serde_json::from_value(serde_json::Value::String(kind))?;
    if kind != RecordKind::Edit && !linked {
        let bytes: Vec<u8> = conn
            .prepare_cached("SELECT record_bytes FROM txs WHERE tx_id = ?1")?
            .query_row(params![tx_id], |r| r.get(0))?;
        return Ok(LinkRec(LogRec {
            record: db::parse_record(Some(seq), &bytes)?,
            branch,
            seq,
            diff: Diff::default(),
        }));
    }
    let targets: Vec<String> = match targets {
        Some(t) => serde_json::from_str(&t)?,
        None => Vec::new(),
    };
    let intent = Tx {
        tx_id: tx_id.clone(),
        author: String::new(),
        branch: branch.clone(),
        base_rev: seq.saturating_sub(1),
        schema_version: SCHEMA_VERSION,
        ops: Vec::new(),
        args_hash: None,
        via: None,
    };
    let mut r = Record::new_edit(
        tx_id,
        seq,
        SCHEMA_VERSION,
        None,
        String::new(),
        0,
        intent,
        Vec::new(),
        Vec::new(),
        BTreeMap::new(),
    );
    r.kind = kind;
    let first = targets.first().cloned();
    match kind {
        RecordKind::Merge => (r.merge_id, r.from, r.origin) = (merge_id, from, origin),
        RecordKind::CherryPick => (r.origin, r.from, r.origin_hash) = (origin, from, origin_hash),
        RecordKind::Undo => r.undoes = Some(targets),
        RecordKind::Redo => r.redoes = first,
        RecordKind::Revert => r.reverts = first,
        _ => {}
    }
    Ok(LinkRec(LogRec {
        record: r,
        branch,
        seq,
        diff: Diff::default(),
    }))
}
