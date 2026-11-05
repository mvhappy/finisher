//! `fin project verify` (sub-project 4 spec §3.8): checks that change nothing and report,
//! never fail. A check whose own reads fail reports `ok: false` with `details.error`.
//!
//! Checks, in this order (what each checks exactly):
//! - `integrity_check`: `PRAGMA integrity_check` of the database says `ok`.
//! - `segment_checksums`: a fresh scan of `.finisher/` (every segment and snapshot is decoded
//!   again: CRC-32C and BLAKE3 over all bytes before the trailer, and name, directory and
//!   header agree). A canonically named file that does not verify, or stays incomplete, is
//!   `SEGMENT_CORRUPT` (`details.corrupt`) once this store has seen it with the same length
//!   and mtime for 60 s of its monotonic clock ([`DamageSeen`]: every import pass and every
//!   verify observes); before that it may still be arriving (`details.unsettled`, not a
//!   failure). Cloud placeholders are not read (`details.unchecked`, not a failure: verify
//!   does not force a download). A file that cannot be read right now (locked, or pending
//!   deletion: [`locked_too_long`]) is `details.unsettled` (`SEGMENT_LOCKED`) until it has
//!   looked the same for 60 s, then unreadable. Unreadable files fail; files needing a newer Finisher are
//!   listed (`newer`). A `segments` row of the database whose file is not listed at all (by
//!   name) and that no present pack replaces fails (`missing`).
//! - `chain_continuity`: per branch, the database's rows are dense from seq 1 (a 4B branch:
//!   from its fork), each
//!   `parent_hash` is the previous row's `tx_hash`, each `tx_hash` is the BLAKE3 of its
//!   `record_bytes`, each record reads as that row (seq, txId, parentHash), and exported rows
//!   are a prefix. The folder, resolved from scratch as a fresh reader would, must hold every
//!   exported row at its seq with the same hash, with no fork and nothing waiting; a folder
//!   ahead of the database (not imported yet) is reported, not a failure. While files under
//!   `segments/` are still arriving or unchecked (placeholders, unsettled damage), what lies
//!   beyond the folder's chain cannot be told: such rows and `waiting` are listed
//!   (`unchecked`), not failures; a different hash at a seq the chain does hold always fails.
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
//!   snapshot, and every segment's `genesisHash` is the hash of that genesis
//!   (`segmentsDisagree`): an edited `genesis.rate` would otherwise silently change a
//!   rebuild. A file without a genesis checks the segments against the database's rev 0.
//! - `record_size`: no stored record is larger than `segment::MAX_RECORD_BYTES` (only a
//!   migrated sub-project 2 record can be; export stops at it, `RECORD_TOO_LARGE`).
//! - `branch_index` (plan 4B Decision 6): the `branches` table equals the index
//!   `crate::derived::compute` derives from the log (`index`), and `record_links` the rows its
//!   records give, inert marks included (`links`); every non-`main` index row has state
//!   (`stateless`: such a branch cannot be read) and every branch with rows an index row
//!   (`unindexed`: it refuses writes). Each branch directory's segments that do not name the
//!   index's `branch` record (Decision 33) name one that is set aside here (an inert row or
//!   a quarantine row; its records not held yet are `pending`, not a failure) or one unknown
//!   here (`waiting`, listed but not a failure while files arrive), never a live record the
//!   index does not hold or a record of another branch (`segments`).
//!
//! Per branch, the folder checks read the segments that make it: all of `main`'s, and a 4B
//! branch's that name its index record, resolved from its fork point
//! (`Known::for_branch`); `chain_continuity` starts a branch at `forkSeq + 1` with
//! `forkHash` as its first parent.

mod branches;
mod folder;
mod replay;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

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

/// Damaged files under `.finisher/` and since when (this store's monotonic clock) each has
/// looked the same (`len`, `mtime`). A file that changes starts again; one that is no longer
/// damaged is forgotten.
#[derive(Debug, Default)]
pub(crate) struct DamageSeen {
    /// `(len, mtime, first seen so, its mtime was already SETTLED old then)`.
    files: HashMap<PathBuf, (u64, Option<SystemTime>, Instant, bool)>,
}

/// How long a damaged file must look unchanged to be corrupt rather than arriving.
pub(crate) const SETTLED: Duration = Duration::from_secs(60);

/// Whether `mtime` is at least [`SETTLED`] old by the wall clock.
fn old_mtime(mtime: Option<SystemTime>) -> bool {
    mtime
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age >= SETTLED)
}

/// Whether a damaged file is corrupt rather than arriving. Off sync folders every finished
/// file was written atomically (temp file + rename), so damage under a canonical name is
/// corrupt at once. On a sync folder a file can be mid-download: it is corrupt once seen
/// unchanged for [`SETTLED`], or, at its first sighting, when its mtime is already that old.
pub(crate) fn settled(
    seen: Option<&DamageSeen>,
    path: &Path,
    mtime: Option<SystemTime>,
    now: Instant,
    kind: StorageKind,
) -> bool {
    if !kind.is_sync() {
        return true;
    }
    match seen.and_then(|s| s.files.get(path)) {
        Some((_, _, since, old)) => *old || now.saturating_duration_since(*since) >= SETTLED,
        None => old_mtime(mtime),
    }
}

/// Whether a file that cannot be read now (`NotArrivedWhy::Locked`: held by another program,
/// pending deletion while another client holds it open, or a permission error that looks
/// transient, as Windows `ACCESS_DENIED`) has looked the same for [`SETTLED`] of this store's
/// monotonic clock, on any kind of folder. Then it is reported (verify: `unreadable`; status:
/// `LOCKED_FILES`) rather than taken as arriving. Never at first sighting and never from an old
/// mtime, so a segment that compaction deleted while another client briefly holds it stays
/// quiet.
pub(crate) fn locked_too_long(seen: Option<&DamageSeen>, path: &Path, now: Instant) -> bool {
    seen.and_then(|s| s.files.get(path))
        .is_some_and(|(_, _, since, _)| now.saturating_duration_since(*since) >= SETTLED)
}

impl DamageSeen {
    /// Records one scan's damaged, incomplete and locked files (`(path, len, mtime)`) at `now`;
    /// forgets the others. A file whose metadata could not be read this time (`(0, None)`: on
    /// Windows a file pending deletion refuses even a stat) keeps its earlier sighting, so one
    /// that flips between incomplete and locked does not restart its time.
    pub(crate) fn observe(&mut self, seen: &[(PathBuf, u64, Option<SystemTime>)], now: Instant) {
        let mut next = HashMap::new();
        for (path, len, mtime) in seen {
            let unknown = *len == 0 && mtime.is_none();
            let entry = match self.files.get(path) {
                Some(&(l, m, since, old)) if unknown || (l == *len && m == *mtime) => {
                    (l, m, since, old)
                }
                _ => (*len, *mtime, now, old_mtime(*mtime)),
            };
            next.insert(path.clone(), entry);
        }
        self.files = next;
    }
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
    /// The store's damage observations and its clock's now; `None` (a rebuild): nothing is
    /// settled.
    pub(crate) damage: Option<(&'a RefCell<DamageSeen>, Instant)>,
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
    checks.push(guarded("branch_index", || branches::check(s, &f)));
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
    let rev0 = db::snapshot_at(s.conn, MAIN, 0)?;
    let mut ok = true;
    let expected = match (&file.genesis, &rev0) {
        (Some(g), _) => {
            let fresh = new_project(s.project_id.to_string(), g.name.clone(), g.rate)?;
            ok &= rev0.as_ref() == Some(&fresh);
            Some(crate::project_file::genesis_hash(g))
        }
        (None, Some(p)) => Some(crate::project_file::genesis_hash(
            &crate::project_file::genesis_of(p),
        )),
        (None, None) => None,
    };
    let disagree = match &expected {
        Some(h) => folder::genesis_disagree(s.project_dir, h),
        None => 0,
    };
    ok &= disagree == 0;
    let details = json!({
        "file": file.genesis.as_ref().map(|g| json!({ "name": g.name, "rate": g.rate.to_string() })),
        "rev0": rev0.map(|p| json!({ "name": p.name, "rate": p.rate.to_string() })),
        "genesisHash": expected,
        "segmentsDisagree": disagree,
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
    ///
    /// Every check reads inside one read transaction, so they see one state of the database.
    pub fn verify(&self) -> VerifyReport {
        let txn = match self.conn.unchecked_transaction() {
            Ok(t) => t,
            Err(e) => {
                let e = fin_core::CoreError::from(StoreError::from(e));
                return VerifyReport {
                    ok: false,
                    checks: vec![Check {
                        name: "integrity_check".into(),
                        ok: false,
                        details: json!({ "error": e }),
                    }],
                };
            }
        };
        let mut live = HashMap::new();
        {
            let mut heads = self.heads.borrow_mut();
            for branch in db::branches(&txn).unwrap_or_default() {
                if crate::refresh(&txn, &mut heads, &branch).is_ok()
                    && let Some(h) = heads.get(&branch)
                {
                    live.insert(branch, (h.rev, h.project.clone()));
                }
            }
        }
        let report = run(&Subject {
            conn: &txn,
            project_dir: &self.project_dir,
            project_id: &self.project_id,
            kind: self.storage_kind(),
            live: Some(live),
            damage: Some((&self.damage, self.ctl.now())),
        });
        drop(txn); // read only: rolled back
        report
    }
}
