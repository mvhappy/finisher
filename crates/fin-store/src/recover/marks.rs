//! The recovered marks for status, without scanning quarantine (Task 20 measure, plan 4B
//! Decision 5).
//!
//! `quarantine_marks` holds, per quarantine row, what the marks need from its record: its
//! kind and, for a `branch` or `branchSet` record, the branch id (`target`). A record's bytes
//! never change under its `txHash`, so a row is read once: [`fill`] reads the rows past `meta`
//! `quarantineMarksThrough` (`quarantine.id` only grows) on every import pass, after
//! `--import`, and nothing else ever has to. The table is derived from `quarantine` alone, so a
//! rebuild or a follower fills the same rows, and verify compares it with the bytes
//! ([`check`]).
//!
//! [`uncounted`] is the set [`super::groups::Forest::recovered`] gives plus the dismissed
//! rows, from indexes: the rows a record in `txs` names as `originHash` (read from the side of
//! `record_links`, so its cost follows the copies, not the quarantine), the `branchSet` rows
//! of branches whose `branch` row is among them, and `meta` `recoverDismissed`. Rows not
//! filled yet (written by a path that does not fill) are read from their bytes.

use std::collections::{BTreeMap, HashSet};

use fin_core::{RecordKind, kind_name};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::StoreError;
use crate::db::{meta, parse_record, set_meta};

const THROUGH: &str = "quarantineMarksThrough";

/// What the marks need of a record's bytes: its kind (`?` when they do not read) and the
/// branch a `branch` or `branchSet` record names.
fn mark(bytes: &[u8]) -> (String, Option<String>) {
    match parse_record(None, bytes) {
        Ok(r) => {
            let target = matches!(r.kind, RecordKind::Branch | RecordKind::BranchSet)
                .then_some(r.branch_id)
                .flatten();
            (kind_name(r.kind), target)
        }
        Err(_) => ("?".to_string(), None),
    }
}

fn through(conn: &Connection) -> Result<i64, StoreError> {
    Ok(meta(conn, THROUGH)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

type Unread = Vec<(String, String, Option<String>)>;

/// The rows past the watermark: `(txHash, kind, target)` and the highest id.
fn unread(conn: &Connection) -> Result<(Unread, Option<i64>), StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, tx_hash, record_bytes FROM quarantine WHERE id > ?1 ORDER BY id",
    )?;
    let mut rows = stmt.query(params![through(conn)?])?;
    let (mut out, mut last) = (vec![], None);
    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(2)?;
        let (kind, target) = mark(&bytes);
        out.push((row.get(1)?, kind, target));
        last = Some(row.get(0)?);
    }
    Ok((out, last))
}

/// Reads the quarantine rows added since the last fill into `quarantine_marks`.
pub(crate) fn fill(conn: &Connection) -> Result<(), StoreError> {
    let (rows, last) = unread(conn)?;
    let Some(last) = last else {
        return Ok(());
    };
    let mut put = conn.prepare_cached(
        "INSERT OR REPLACE INTO quarantine_marks (tx_hash, kind, target) VALUES (?1, ?2, ?3)",
    )?;
    for (hash, kind, target) in rows {
        put.execute(params![hash, kind, target])?;
    }
    set_meta(conn, THROUGH, &last.to_string())?;
    Ok(())
}

/// The current rows status does not count, by `txHash` (module docs).
pub(crate) fn uncounted(conn: &Connection) -> Result<HashSet<String>, StoreError> {
    let dismissed = super::dismiss::load(conn)?;
    let stood: HashSet<String> = {
        // `record_links` first: the rows named by a copy, looked up by their unique hash.
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT l.origin_hash FROM record_links l CROSS JOIN quarantine q
             WHERE l.origin_hash IS NOT NULL AND q.tx_hash = l.origin_hash AND q.current = 1",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let (unread, _) = unread(conn)?;
    let mut out = stood.clone();
    if !stood.is_empty() {
        // Branches whose `branch` row is stood for: their `branchSet` rows come back too.
        let mut targets = HashSet::new();
        let mut kind = conn.prepare_cached(
            "SELECT target FROM quarantine_marks WHERE tx_hash = ?1 AND kind = 'branch'",
        )?;
        for h in &stood {
            if let Some(t) = kind
                .query_map(params![h], |r| r.get::<_, Option<String>>(0))?
                .next()
                .transpose()?
                .flatten()
            {
                targets.insert(t);
            }
        }
        for (h, k, t) in &unread {
            if k == "branch" && stood.contains(h) {
                targets.extend(t.clone());
            }
        }
        let mut sets = conn.prepare_cached(
            "SELECT m.tx_hash FROM quarantine_marks m CROSS JOIN quarantine q
             WHERE m.kind = 'branchSet' AND m.target = ?1
             AND q.tx_hash = m.tx_hash AND q.current = 1",
        )?;
        for t in &targets {
            for h in sets.query_map(params![t], |r| r.get::<_, String>(0))? {
                out.insert(h?);
            }
            out.extend(
                unread
                    .iter()
                    .filter(|(_, k, tt)| k == "branchSet" && tt.as_ref() == Some(t))
                    .map(|(h, ..)| h.clone()),
            );
        }
    }
    out.extend(dismissed);
    Ok(out)
}

/// Verify: the stored marks against the rows' bytes, up to the watermark (rows past it are
/// read on the next import pass). Mismatches and marks without a row.
pub(crate) fn check(conn: &Connection) -> Result<Vec<Value>, StoreError> {
    let upto = through(conn)?;
    let stored: BTreeMap<String, (String, Option<String>)> = {
        let mut stmt = conn.prepare("SELECT tx_hash, kind, target FROM quarantine_marks")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut seen = HashSet::new();
    let mut out = vec![];
    let mut stmt = conn.prepare("SELECT id, tx_hash, record_bytes FROM quarantine ORDER BY id")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let (id, hash, bytes): (i64, String, Vec<u8>) = (row.get(0)?, row.get(1)?, row.get(2)?);
        seen.insert(hash.clone());
        if id > upto {
            continue;
        }
        let want = mark(&bytes);
        if stored.get(&hash) != Some(&want) {
            out.push(json!({ "txHash": hash, "fromBytes": want, "stored": stored.get(&hash) }));
        }
    }
    for h in stored.keys().filter(|h| !seen.contains(*h)) {
        out.push(json!({ "txHash": h, "fromBytes": null, "stored": stored.get(h) }));
    }
    Ok(out)
}
