//! `replay_matches_live` and `diffs_match` (`super` module docs): one replay per branch from
//! its first snapshot.

use std::collections::HashMap;

use fin_core::{Diff, Project};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::{StoreError, db};

/// How many mismatches are listed (all are counted).
const LIST: usize = 20;

pub(super) struct Outcome {
    pub(super) replay: Result<(bool, Value), StoreError>,
    pub(super) diffs: Result<(bool, Value), StoreError>,
}

#[derive(Default)]
struct Branch {
    rows: u64,
    snapshot_mismatches: Vec<u64>,
    stopped: Option<Value>,
    head_matches: bool,
    diff_mismatches: Vec<Value>,
    diff_mismatch_count: u64,
    not_stored: u64,
}

pub(super) fn run(conn: &Connection, live: &HashMap<String, (u64, Project)>) -> Outcome {
    let branches = match db::branches(conn) {
        Ok(b) => b,
        Err(e) => {
            let msg = fin_core::CoreError::from(e);
            return Outcome {
                replay: Ok((false, json!({ "error": msg }))),
                diffs: Ok((false, json!({ "error": msg }))),
            };
        }
    };
    let mut out = vec![];
    for branch in branches {
        match one(conn, &branch, live.get(&branch)) {
            Ok(b) => out.push((branch, b)),
            Err(e) => {
                return Outcome {
                    replay: Err(e),
                    diffs: Ok((
                        false,
                        json!({ "error": "the replay failed to read the database" }),
                    )),
                };
            }
        }
    }
    let replay_ok = out
        .iter()
        .all(|(_, b)| b.head_matches && b.snapshot_mismatches.is_empty() && b.stopped.is_none());
    let replay = json!({ "branches": out.iter().map(|(name, b)| json!({
        "branch": name, "rows": b.rows, "headMatches": b.head_matches,
        "snapshotMismatches": b.snapshot_mismatches, "stopped": b.stopped,
    })).collect::<Vec<_>>() });
    let diffs_ok = out.iter().all(|(_, b)| b.diff_mismatch_count == 0);
    let mismatches: Vec<Value> = out
        .iter()
        .flat_map(|(_, b)| b.diff_mismatches.iter().cloned())
        .take(LIST)
        .collect();
    let diffs = json!({
        "checked": out.iter().map(|(_, b)| b.rows - b.not_stored).sum::<u64>(),
        "notStored": out.iter().map(|(_, b)| b.not_stored).sum::<u64>(),
        "mismatchCount": out.iter().map(|(_, b)| b.diff_mismatch_count).sum::<u64>(),
        "mismatches": mismatches,
    });
    Outcome {
        replay: Ok((replay_ok, replay)),
        diffs: Ok((diffs_ok, diffs)),
    }
}

fn one(
    conn: &Connection,
    branch: &str,
    live: Option<&(u64, Project)>,
) -> Result<Branch, StoreError> {
    let mut b = Branch::default();
    let first: Option<i64> = conn.query_row(
        "SELECT MIN(seq) FROM snapshots WHERE branch = ?1",
        params![branch],
        |r| r.get(0),
    )?;
    let Some(first) = first else {
        return Ok(b);
    };
    let first = db::to_u64(first)?;
    let mut project =
        db::snapshot_at(conn, branch, first)?.ok_or_else(|| crate::branch_not_found(branch))?;
    let snapshots: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT seq FROM snapshots WHERE branch = ?1 AND seq > ?2")?;
        stmt.query_map(params![branch, db::to_i64(first)?], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    let snapshots: std::collections::HashSet<u64> = snapshots
        .into_iter()
        .filter_map(|s| u64::try_from(s).ok())
        .collect();
    let mut rev = first;
    let mut stmt = conn.prepare(
        "SELECT seq, tx_id, record_bytes, diff FROM txs WHERE branch = ?1 AND seq > ?2 ORDER BY seq",
    )?;
    let mut rows = stmt.query(params![branch, db::to_i64(first)?])?;
    while let Some(row) = rows.next()? {
        let seq = db::to_u64(row.get(0)?)?;
        let tx_id: String = row.get(1)?;
        let bytes: Vec<u8> = row.get(2)?;
        let stored: String = row.get(3)?;
        b.rows += 1;
        let applied = match db::replay_row(&project, branch, &(seq, tx_id.clone(), bytes)) {
            Ok(a) => a,
            Err(e) => {
                b.stopped = Some(json!({ "seq": seq, "error": fin_core::CoreError::from(e) }));
                return Ok(b);
            }
        };
        if stored == crate::diffs::NOT_STORED {
            b.not_stored += 1;
        } else if serde_json::from_str::<Diff>(&stored).ok().as_ref() != Some(&applied.diff) {
            b.diff_mismatch_count += 1;
            if b.diff_mismatches.len() < LIST {
                b.diff_mismatches
                    .push(json!({ "branch": branch, "seq": seq, "txId": tx_id }));
            }
        }
        project = applied.project;
        rev = seq;
        if snapshots.contains(&seq)
            && db::snapshot_at(conn, branch, seq)?.as_ref() != Some(&project)
        {
            b.snapshot_mismatches.push(seq);
        }
    }
    b.head_matches = live.is_some_and(|(r, p)| *r == rev && *p == project);
    Ok(b)
}
