//! History operations (plan 4B Tasks 12–16): each plans its record with a pure `fin-core`
//! planner over [`SqlLog`], the log as one transaction sees it, and appends it through the
//! one write path ([`crate::Store::write`]).
//!
//! - [`undo`]: `Store::undo` (also `undo --to <checkpoint>`), `Store::redo`,
//!   `Store::undo_stacks`.
//! - [`checkpoint`]: `Store::checkpoint`, `Store::find_checkpoint`.
//! - [`drop`]: `Store::drop` (durable rejections, plan 5E Decision 5), `Store::dropped`.
//! - [`merge`] holds `Store::merge`; [`review_plan`] `Store::review_plan` (plan 5E Decision 7).
//! - [`pick`]: `Store::cherry_pick`, `Store::revert`.
//! - [`origins`]: what a result's records stand for (`origins`, 7A Decision 27).

pub(crate) mod checkpoint;
pub(crate) mod drop;
pub(crate) mod merge;
pub(crate) mod origins;
pub(crate) mod pick;
pub(crate) mod review_plan;
pub(crate) mod undo;

#[cfg(test)]
#[path = "stack_rows_tests.rs"]
mod stack_rows_tests;

use std::collections::{HashMap, HashSet};

use fin_core::history::{DropMark, Lineage, LogRec, LogView, StackRec};
use fin_core::layout::TextMeasurer;
use fin_core::{CoreError, Link, Project, RecordKind};
use rusqlite::{Connection, OptionalExtension, params};

use crate::branch::{index, lineage};
use crate::db::{self, to_i64, to_u64};
use crate::{StoreError, diffs};

/// The log over `conn` (inside a write, its transaction: what the planner reads is what the
/// write appends to). `head`: one branch's head as the write sees it, so its state is not
/// replayed again and states below it rewind from it.
pub(crate) struct SqlLog<'a> {
    conn: &'a Connection,
    head: Option<(&'a str, u64, &'a Project)>,
    measurer: Option<&'a dyn TextMeasurer>,
}

impl<'a> SqlLog<'a> {
    pub(crate) fn new(conn: &'a Connection) -> SqlLog<'a> {
        SqlLog {
            conn,
            head: None,
            measurer: None,
        }
    }

    /// The log whose planners' re-runs check caption overflow with `m`.
    pub(crate) fn measuring(self, m: Option<&'a dyn TextMeasurer>) -> SqlLog<'a> {
        SqlLog {
            measurer: m,
            ..self
        }
    }

    pub(crate) fn with_head(
        conn: &'a Connection,
        branch: &'a str,
        rev: u64,
        project: &'a Project,
    ) -> SqlLog<'a> {
        SqlLog {
            conn,
            head: Some((branch, rev, project)),
            measurer: None,
        }
    }

    /// `branch`'s own rows with `after < seq <= through`, pending diffs filled first.
    fn rows(&self, branch: &str, after: u64, through: u64) -> Result<Vec<LogRec>, StoreError> {
        if diffs::pending(self.conn, branch)?.is_some_and(|p| p > after) {
            diffs::fill(self.conn, branch)?;
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, record_bytes, diff FROM txs WHERE branch = ?1 AND seq > ?2 AND seq <= ?3
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![branch, to_i64(after)?, to_i64(through)?], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|r| {
            let (seq, bytes, diff) = r?;
            log_rec(branch, to_u64(seq)?, &bytes, &diff)
        })
        .collect()
    }

    /// `branch`'s own rows with `after < seq <= through` as the stacks read them: kind and
    /// links from `txs` and `record_links`, the intent's author out of the record's JSON (no
    /// record is parsed, plan 4B Task 20). A link-bearing row without its link row (never on a
    /// consistent DB) is parsed whole.
    fn stack_rows(
        &self,
        branch: &str,
        after: u64,
        through: u64,
    ) -> Result<Vec<StackRec>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.seq, t.tx_id, t.kind,
                    json_extract(CAST(t.record_bytes AS TEXT), '$.intent.author'),
                    l.tx_id IS NOT NULL, l.merge_id, l.targets
             FROM txs t LEFT JOIN record_links l ON l.tx_id = t.tx_id
             WHERE t.branch = ?1 AND t.seq > ?2 AND t.seq <= ?3 ORDER BY t.seq",
        )?;
        type Row = (
            i64,
            String,
            String,
            Option<String>,
            bool,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = stmt
            .query_map(params![branch, to_i64(after)?, to_i64(through)?], |r| {
                // Not a string (or absent): parsed whole below, as the record's own error.
                let author = match r.get_ref(3)? {
                    rusqlite::types::ValueRef::Text(t) => std::str::from_utf8(t).ok(),
                    _ => None,
                };
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    author.map(str::to_string),
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for (seq, tx_id, kind, author, linked, merge_id, targets) in rows {
            let seq = to_u64(seq)?;
            let kind: RecordKind = serde_json::from_value(serde_json::Value::String(kind))?;
            let needs_link = matches!(
                kind,
                RecordKind::Undo | RecordKind::Redo | RecordKind::Merge
            );
            let (Some(author), false) = (author, needs_link && !linked) else {
                out.extend(self.rows(branch, seq - 1, seq)?.iter().map(StackRec::of));
                continue;
            };
            let targets: Vec<String> = match targets {
                Some(t) => serde_json::from_str(&t)?,
                None => Vec::new(),
            };
            let (undoes, redoes) = match kind {
                RecordKind::Undo => (targets, None),
                RecordKind::Redo => (Vec::new(), targets.into_iter().next()),
                _ => (Vec::new(), None),
            };
            out.push(StackRec {
                seq,
                tx_id,
                kind,
                author,
                merge_id,
                undoes,
                redoes,
            });
        }
        Ok(out)
    }

    fn find_row(&self, tx_id: &str) -> Result<Option<LogRec>, StoreError> {
        let read = || -> Result<Option<TxRow>, StoreError> {
            Ok(self
                .conn
                .query_row(
                    "SELECT branch, seq, record_bytes, diff FROM txs WHERE tx_id = ?1",
                    params![tx_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?)
        };
        let mut row = read()?;
        if let Some((branch, ..)) = row.as_ref().filter(|r| r.3 == diffs::NOT_STORED) {
            diffs::fill(self.conn, branch)?;
            row = read()?;
        }
        row.map(|(branch, seq, bytes, diff)| log_rec(&branch, to_u64(seq)?, &bytes, &diff))
            .transpose()
    }

    fn merged_away_ids(&self, branch: &str) -> Result<HashSet<String>, StoreError> {
        let ix = index::load(self.conn)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT l.origin, l.branch FROM record_links l
             JOIN txs t ON t.tx_id = l.origin AND t.branch = ?1
             WHERE l.kind = 'merge' AND l.from_branch = ?1 AND l.origin IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![branch], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = HashSet::new();
        for row in rows {
            let (origin, target) = row?;
            if target != branch && !ix.is_descendant(&target, branch) {
                out.insert(origin);
            }
        }
        Ok(out)
    }
}

/// A `txs` row by txId: `(branch, seq, record_bytes, diff)`.
type TxRow = (String, i64, Vec<u8>, String);

fn log_rec(branch: &str, seq: u64, bytes: &[u8], diff: &str) -> Result<LogRec, StoreError> {
    if diff == diffs::NOT_STORED {
        return Err(CoreError::new(
            "DB_ERROR",
            format!("the diff of {branch} seq {seq} is not stored"),
        )
        .with_fix("rebuild the project database (fin project rebuild)")
        .into());
    }
    let (record, diff) = db::parse_record_with_diff(Some(seq), bytes, Some(diff))?;
    Ok(LogRec {
        branch: branch.to_string(),
        seq,
        record,
        diff: diff.unwrap_or_default(),
    })
}

impl LogView for SqlLog<'_> {
    fn measurer(&self) -> Option<&dyn TextMeasurer> {
        self.measurer
    }

    fn lineage(&self, branch: &str) -> Result<Lineage, CoreError> {
        let segs = lineage::lineage(self.conn, branch)?;
        Ok(Lineage {
            segs: segs
                .into_iter()
                .map(|s| (s.branch, s.after, s.through))
                .collect(),
        })
    }

    fn records(&self, branch: &str, after: u64, through: u64) -> Result<Vec<LogRec>, CoreError> {
        Ok(self.rows(branch, after, through)?)
    }

    fn stack_records(
        &self,
        branch: &str,
        after: u64,
        through: u64,
    ) -> Result<Vec<StackRec>, CoreError> {
        Ok(self.stack_rows(branch, after, through)?)
    }

    fn head(&self, branch: &str) -> Result<(u64, Project), CoreError> {
        if let Some((b, rev, p)) = self.head
            && b == branch
        {
            return Ok((rev, p.clone()));
        }
        let mut one = HashMap::new();
        crate::refresh(self.conn, &mut one, branch)?;
        let head = one
            .remove(branch)
            .ok_or_else(|| crate::branch_not_found(branch))?;
        Ok((head.rev, head.project))
    }

    fn state_at(&self, branch: &str, seq: u64) -> Result<Project, CoreError> {
        Ok(match self.head {
            Some((b, rev, p)) if b == branch => {
                lineage::state_near_head(self.conn, branch, seq, rev, p)?
            }
            _ => lineage::state_at(self.conn, branch, seq)?,
        })
    }

    fn find(&self, tx_id: &str) -> Result<Option<LogRec>, CoreError> {
        Ok(self.find_row(tx_id)?)
    }

    fn merge_group(&self, merge_id: &str) -> Result<Vec<LogRec>, CoreError> {
        let ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare_cached(
                    "SELECT tx_id FROM record_links WHERE kind = 'merge' AND merge_id = ?1
                     ORDER BY branch, seq",
                )
                .map_err(StoreError::from)?;
            let rows = stmt
                .query_map(params![merge_id], |r| r.get(0))
                .map_err(StoreError::from)?;
            rows.collect::<Result<_, _>>().map_err(StoreError::from)?
        };
        let mut out = vec![];
        for id in ids {
            out.extend(self.find_row(&id)?);
        }
        Ok(out)
    }

    fn is_descendant(&self, a: &str, of: &str) -> Result<bool, CoreError> {
        Ok(index::load(self.conn)?.is_descendant(a, of))
    }

    fn merged_away(&self, branch: &str) -> Result<HashSet<String>, CoreError> {
        Ok(self.merged_away_ids(branch)?)
    }

    fn drops_on(&self, lineage: &Lineage) -> Result<Vec<DropMark>, CoreError> {
        Ok(crate::drops_index::marks(self.conn, lineage)?)
    }
}

/// The units an `undo` record names, or the undo a `redo` record reverses.
pub(crate) fn targets(link: Option<&Link>) -> Vec<String> {
    match link {
        Some(Link::Undo { undoes }) => undoes.clone(),
        Some(Link::Redo { redoes }) => vec![redoes.clone()],
        _ => vec![],
    }
}

/// `IDEM_KEY_REUSED` when `tx_id`, the txId a write would use, already names a merge group:
/// a group's id and a record's txId never share a value (the merge checks the other way).
pub(crate) fn merge_id_taken(conn: &Connection, tx_id: &str) -> Result<(), StoreError> {
    let taken = conn
        .prepare_cached("SELECT 1 FROM record_links WHERE merge_id = ?1 LIMIT 1")?
        .exists(params![tx_id])?;
    if !taken {
        return Ok(());
    }
    Err(CoreError::new(
        "IDEM_KEY_REUSED",
        format!("txId {tx_id} is the id of a merge"),
    )
    .with_details(serde_json::json!({ "txId": tx_id, "mergeId": tx_id }))
    .with_fix("use a new idempotency key (txId)")
    .into())
}
