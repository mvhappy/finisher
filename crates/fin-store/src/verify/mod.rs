//! `fin project verify` (sub-project 4 spec §3.8): checks that change nothing and report,
//! never fail. A check whose own reads fail reports `ok: false` with `details.error`.
//!
//! Checks, in this order (what each checks exactly):
//! - `integrity_check`: `PRAGMA integrity_check` of the database says `ok`.
//! - `segment_checksums`: a fresh scan of `.finisher/` (every segment and snapshot is decoded
//!   again: CRC-32C and BLAKE3 over all bytes before the trailer, and name, directory and
//!   header agree). A canonically named file that does not verify, or stays incomplete, and
//!   has not changed for 60 s is `SEGMENT_CORRUPT` (`details.corrupt`); younger ones may still
//!   be arriving (`details.unsettled`, not a failure). Unreadable files fail; files needing a
//!   newer Finisher are listed (`newer`). A `segments` row of the database whose file is gone
//!   and that no present pack replaces fails (`missing`).
//! - `chain_continuity`: per branch, the database's rows are dense from seq 1, each
//!   `parent_hash` is the previous row's `tx_hash`, each `tx_hash` is the BLAKE3 of its
//!   `record_bytes`, each record reads as that row (seq, txId, parentHash), and exported rows
//!   are a prefix. The folder, resolved from scratch as a fresh reader would, must hold every
//!   exported row at its seq with the same hash, with no fork and nothing waiting; a folder
//!   ahead of the database (not imported yet) is reported, not a failure.
//! - `epoch_order`: along the chain resolved from scratch, each record's epoch, taken as the
//!   lowest epoch of the present segments holding it (so an original wins over the pack that
//!   replaces it), never decreases. Per segment labels, not per `txs.epoch` row: imported rows
//!   carry their segment's epoch, an upper bound. A pack must be labelled with the highest
//!   epoch of the present segments it replaces (`packLabels`). The database's unexported rows
//!   have non-decreasing commit epochs, none below the highest epoch of an exported row.
//! - `replay_matches_live`: per branch, replaying every row from the branch's first snapshot
//!   gives each stored snapshot at its seq and the live head (rev and state).
//! - `diffs_match`: each stored `diff` equals the diff that replay computes. Import and open
//!   never re-check stored diffs (they are served by `ops_since` and used by rebase), so the
//!   stored diff wins until this check reports it. Diffs a seeded rebuild has not computed
//!   yet are counted (`notStored`), not failures.
//! - `genesis`: `new_project` of the project file's genesis equals the database's rev-0
//!   snapshot (an edited `genesis.rate` would otherwise silently change a rebuild). A file
//!   without a genesis is skipped.
//! - `record_size`: no stored record is larger than `segment::MAX_RECORD_BYTES` (only a
//!   migrated sub-project 2 record can be; export stops at it, `RECORD_TOO_LARGE`).

mod folder;
mod replay;

use std::collections::HashMap;
use std::path::Path;

use fin_core::{Project, new_project};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Value, json};

use crate::storage_kind::StorageKind;
use crate::{MAIN, Store, StoreError, db};

/// One check's outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub details: Value,
}

/// [`Store::verify`]: `ok` when every check is.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub ok: bool,
    pub checks: Vec<Check>,
}

/// What is verified.
pub(crate) struct Subject<'a> {
    pub(crate) conn: &'a Connection,
    pub(crate) project_dir: &'a Path,
    pub(crate) project_id: &'a str,
    pub(crate) kind: StorageKind,
    /// The live heads `(rev, state)` per branch; `None` skips the replay checks (a rebuild,
    /// which just replayed).
    pub(crate) live: Option<HashMap<String, (u64, Project)>>,
}

/// A check from `f`; an error inside it is reported, not raised.
fn guarded(name: &str, f: impl FnOnce() -> Result<(bool, Value), StoreError>) -> Check {
    let (ok, details) = match f() {
        Ok(r) => r,
        Err(e) => {
            let e = fin_core::CoreError::from(e);
            (false, json!({ "error": e }))
        }
    };
    Check {
        name: name.to_string(),
        ok,
        details,
    }
}

pub(crate) fn run(s: &Subject) -> VerifyReport {
    let mut checks = vec![guarded("integrity_check", || integrity(s.conn))];
    let f = folder::Folder::read(s);
    checks.push(guarded("segment_checksums", || f.checksums(s)));
    checks.push(guarded("chain_continuity", || f.continuity(s)));
    checks.push(guarded("epoch_order", || f.epochs(s)));
    match &s.live {
        Some(live) => {
            let r = replay::run(s.conn, live);
            checks.push(guarded("replay_matches_live", || r.replay));
            checks.push(guarded("diffs_match", || r.diffs));
        }
        None => {
            for name in ["replay_matches_live", "diffs_match"] {
                checks.push(Check {
                    name: name.to_string(),
                    ok: true,
                    details: json!({ "skipped": "a rebuild replays while it imports; fin project verify runs this check" }),
                });
            }
        }
    }
    checks.push(guarded("genesis", || genesis(s)));
    checks.push(guarded("record_size", || record_size(s.conn)));
    VerifyReport {
        ok: checks.iter().all(|c| c.ok),
        checks,
    }
}

fn integrity(conn: &Connection) -> Result<(bool, Value), StoreError> {
    let mut stmt = conn.prepare("PRAGMA integrity_check(100)")?;
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let ok = rows == ["ok"];
    Ok((ok, json!({ "messages": if ok { vec![] } else { rows } })))
}

fn genesis(s: &Subject) -> Result<(bool, Value), StoreError> {
    let file = crate::project_file::read_project_file_once(s.project_dir)?;
    let Some(g) = file.genesis else {
        return Ok((
            true,
            json!({ "skipped": "the project file records no genesis (backfilled by the lease holder's next open)" }),
        ));
    };
    let fresh = new_project(s.project_id.to_string(), g.name.clone(), g.rate)?;
    let rev0 = db::snapshot_at(s.conn, MAIN, 0)?;
    let ok = rev0.as_ref() == Some(&fresh);
    let details = json!({
        "file": { "name": g.name, "rate": g.rate.to_string() },
        "rev0": rev0.map(|p| json!({ "name": p.name, "rate": p.rate.to_string() })),
    });
    Ok((ok, details))
}

fn record_size(conn: &Connection) -> Result<(bool, Value), StoreError> {
    let limit = crate::segment::MAX_RECORD_BYTES;
    let mut stmt = conn.prepare(
        "SELECT branch, seq, length(record_bytes) FROM txs WHERE length(record_bytes) > ?1
         ORDER BY branch, seq LIMIT 100",
    )?;
    let big: Vec<Value> = stmt
        .query_map([db::to_i64(limit as u64)?], |r| {
            Ok(
                json!({ "branch": r.get::<_, String>(0)?, "seq": r.get::<_, i64>(1)?,
                       "bytes": r.get::<_, i64>(2)? }),
            )
        })?
        .collect::<Result<_, _>>()?;
    Ok((big.is_empty(), json!({ "limit": limit, "tooLarge": big })))
}

impl Store {
    /// Verifies the database against itself and the project folder (module docs), changing
    /// nothing. Mismatches are reported, never fatal. Long-running (it replays every
    /// branch): `fin-node` runs it on the per-engine worker thread (plan Decision 7).
    pub fn verify(&self) -> VerifyReport {
        let mut live = HashMap::new();
        {
            let mut heads = self.heads.borrow_mut();
            for branch in db::branches(&self.conn).unwrap_or_default() {
                if crate::refresh(&self.conn, &mut heads, &branch).is_ok()
                    && let Some(h) = heads.get(&branch)
                {
                    live.insert(branch, (h.rev, h.project.clone()));
                }
            }
        }
        run(&Subject {
            conn: &self.conn,
            project_dir: &self.project_dir,
            project_id: &self.project_id,
            kind: self.storage_kind(),
            live: Some(live),
        })
    }
}
