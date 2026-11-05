//! Rewound history: branch generations, the rewinds behind them, and what became of a `txId`.
//!
//! An import can take records out of a branch's history: a diverged local tail (spec §3.6
//! step 7), or an applied line that lost to a higher lease epoch (`import::rewind`). Clients
//! that saw those records must learn that the history they hold is gone, so every such change
//! bumps the branch's **generation** (`meta` `generation.<branch>`, starting at 0) and is
//! recorded (`meta` `rewinds.<branch>`, the last [`KEEP`] as JSON):
//! - [`crate::StoreEvent::Rewound`] reports it as it happens (finisherd ends `ops.subscribe`
//!   with `HISTORY_REWOUND {branch, rev}`, plan Task 15);
//! - a commit that names the generation its caller read ([`crate::Store::commit_at_generation`])
//!   is refused with `HISTORY_REWOUND` when a rewind since then reaches its `baseRev`: it was
//!   written against records that no longer exist, and rebasing it over other history would
//!   silently change what it means;
//! - status reports the latest rewind of an applied line while its records are set aside.
//!
//! Generations live in the database: a database rebuilt from the folder starts again at 0,
//! and a caller's generation above the current one is treated as stale too.

use fin_core::CoreError;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{find_tx, meta, set_meta};
use crate::{StoreError, TxStatus};

/// How many rewinds per branch `meta` keeps.
pub(crate) const KEEP: usize = 32;

/// Lost records spanning more commit time than this suggest two writers at once.
pub(crate) const SPLIT_BRAIN_SPAN_MS: i64 = 30_000;

/// What was rewound, as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RewindInfo {
    /// The generation this rewind started.
    pub(crate) generation: u64,
    /// The first rev that no longer holds what it held.
    pub(crate) from_rev: u64,
    /// `appliedLine` or `divergedTail`.
    pub(crate) kind: String,
    /// Records taken out (set aside in quarantine).
    pub(crate) count: u32,
    /// Of those, this server's own (exported by it, or never exported).
    pub(crate) own: u32,
    /// The `writerId`s of the lost records' segments.
    pub(crate) writers: Vec<String>,
    /// The epochs of the lost records' segments.
    pub(crate) epochs: Vec<u64>,
    /// The epoch of the line that won (0 for a diverged tail).
    pub(crate) winner_epoch: u64,
    /// Distinct segments that held the lost records.
    pub(crate) segments: u32,
    /// Commit time from the first lost record to the last.
    pub(crate) span_ms: i64,
    /// Unix ms of the rewind.
    pub(crate) at: i64,
}

impl RewindInfo {
    /// Whether this looks like two writers at once rather than one late write.
    pub(crate) fn split_brain(&self) -> bool {
        self.segments > 1 || self.span_ms > SPLIT_BRAIN_SPAN_MS
    }
}

fn generation_key(branch: &str) -> String {
    format!("generation.{branch}")
}

fn rewinds_key(branch: &str) -> String {
    format!("rewinds.{branch}")
}

/// The generation of `branch` (0 before any rewind).
pub(crate) fn current(conn: &Connection, branch: &str) -> Result<u64, StoreError> {
    Ok(meta(conn, &generation_key(branch))?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// The recorded rewinds of `branch`, oldest first.
pub(crate) fn rewinds(conn: &Connection, branch: &str) -> Result<Vec<RewindInfo>, StoreError> {
    Ok(meta(conn, &rewinds_key(branch))?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

/// Bumps the generation of `branch` and records `info` under it; returns the new generation.
pub(crate) fn record(
    conn: &Connection,
    branch: &str,
    mut info: RewindInfo,
) -> Result<u64, StoreError> {
    let generation = current(conn, branch)? + 1;
    set_meta(conn, &generation_key(branch), &generation.to_string())?;
    info.generation = generation;
    let mut all = rewinds(conn, branch)?;
    all.push(info);
    let drop = all.len().saturating_sub(KEEP);
    all.drain(..drop);
    set_meta(conn, &rewinds_key(branch), &serde_json::to_string(&all)?)?;
    Ok(generation)
}

/// `HISTORY_REWOUND` unless a commit read at generation `expected` with `base_rev` is still
/// valid: no rewind since `expected` starts at or below `base_rev`. A generation that is not
/// the current one and whose rewinds are no longer all recorded (or a future one) is stale
/// whatever its base.
pub(crate) fn check(
    conn: &Connection,
    branch: &str,
    expected: u64,
    base_rev: u64,
) -> Result<(), StoreError> {
    let now = current(conn, branch)?;
    if expected == now {
        return Ok(());
    }
    let since: Vec<RewindInfo> = rewinds(conn, branch)?
        .into_iter()
        .filter(|r| r.generation > expected)
        .collect();
    let complete = expected < now && since.len() as u64 == now - expected;
    let rev = if complete {
        since.iter().map(|r| r.from_rev).min().unwrap_or(0)
    } else {
        0
    };
    if complete && base_rev < rev {
        return Ok(());
    }
    Err(CoreError::new(
        "HISTORY_REWOUND",
        format!(
            "branch {branch:?} was rewound from rev {rev} since generation {expected} (now {now}): the transaction was made against history that no longer exists"
        ),
    )
    .with_details(json!({
        "branch": branch, "rev": rev, "generation": now, "expected": expected, "baseRev": base_rev,
    }))
    .with_fix("read the branch again (its head and generation) and redo the edit on top of it")
    .into())
}

/// What became of `tx_id`: committed (on the resolved chain), set aside in quarantine (the
/// newest such row: `SUPERSEDED`, `DIVERGED_TAIL`, `STALE_EPOCH`, …), or unknown.
pub(crate) fn tx_status(conn: &Connection, tx_id: &str) -> Result<TxStatus, StoreError> {
    if let Some(c) = find_tx(conn, tx_id)? {
        return Ok(TxStatus::Committed(c));
    }
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT branch, reason, origin FROM quarantine
             WHERE CASE WHEN json_valid(CAST(record_bytes AS TEXT))
                        THEN json_extract(CAST(record_bytes AS TEXT), '$.txId') END = ?1
             ORDER BY current DESC, seen_at DESC, id DESC LIMIT 1",
            params![tx_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(match row {
        Some((branch, reason, origin)) => TxStatus::SetAside {
            branch,
            reason,
            origin,
        },
        None => TxStatus::Unknown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(from_rev: u64) -> RewindInfo {
        RewindInfo {
            generation: 0,
            from_rev,
            kind: "appliedLine".into(),
            count: 1,
            own: 0,
            writers: vec![],
            epochs: vec![],
            winner_epoch: 2,
            segments: 1,
            span_ms: 0,
            at: 0,
        }
    }

    #[test]
    fn a_stale_generation_is_refused_from_the_lowest_rewind_since() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        assert_eq!(current(&c, "main").unwrap(), 0);
        check(&c, "main", 0, 99).unwrap();
        assert_eq!(record(&c, "main", info(10)).unwrap(), 1);
        assert_eq!(record(&c, "main", info(7)).unwrap(), 2);
        let code = |g, base| check(&c, "main", g, base).map_err(|e| e.code().to_string());
        assert_eq!(code(2, 50), Ok(()));
        assert_eq!(code(1, 6), Ok(()), "below the rewind since 1");
        assert_eq!(code(1, 7), Err("HISTORY_REWOUND".into()));
        assert_eq!(code(0, 6), Ok(()));
        assert_eq!(
            code(3, 0),
            Err("HISTORY_REWOUND".into()),
            "a future generation"
        );
        assert_eq!(current(&c, "other").unwrap(), 0, "per branch");
        // Only the last KEEP rewinds are kept: older generations are stale whatever the base.
        for _ in 0..KEEP {
            record(&c, "main", info(100)).unwrap();
        }
        assert_eq!(rewinds(&c, "main").unwrap().len(), KEEP);
        assert_eq!(code(0, 0), Err("HISTORY_REWOUND".into()));
        assert_eq!(code(2, 99), Ok(()));
    }
}
