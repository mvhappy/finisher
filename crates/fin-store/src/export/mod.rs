//! Export (sub-project 4 spec §3.4 "Export cadence"): committed records go to
//! `.finisher/segments/<branch>/` as verified, chained, write-once segments.
//!
//! # One pass ([`export_pending`])
//!
//! 1. Only while this server holds the lease ([`StoreLease::check`]) and the last import
//!    found the project writable (no fork, no newer format). The lease is checked
//!    again right before every write (the fence), but the segment's epoch is **not** taken
//!    from it: a segment is labelled with the highest commit epoch (`txs.epoch`) of its
//!    records, which must not exceed the current lease epoch. Labelling with the epoch at
//!    export would launder records committed under a lost lease into a newer epoch: after B
//!    commits t1 at epoch 2, loses the lease, C exports u at 3 and B re-acquires at 4, a
//!    t1 labelled 4 would beat u, an export other servers already applied. Labelled 2, it
//!    loses, as the spec's epoch rule (§3.6 step 6) intends: the unexported tail competes at
//!    its commit epoch (Task 20 amends the spec's wording).
//! 2. Removes our own `.~fin-*.tmp` files under `segments/`, `segments/*`, `snapshots/*` and
//!    `compaction/` once each has been seen unchanged (same length and mtime) for
//!    [`policy::TEMP_MAX_AGE`] (2×TTL) by the monotonic clock,
//!    checked at most every [`SWEEP_EVERY`]. The mtime is the server's clock and is never
//!    compared with ours. Only the holder writes them (and any server's listing refresh,
//!    which removes its file at once), so these are crash leftovers.
//! 3. Debounce: the first pass that sees unexported records starts the clock; they are written
//!    once [`ExportPolicy::delay`] has passed (2 s; under churn 5 s, or 30 s on a sync
//!    folder). `force` (close) skips the wait. After a failure the clock restarts, so a broken folder is retried once per delay.
//! 4. Per branch, from its lowest unexported seq, one segment at a time:
//!    - the exported rows must be a seq prefix of the branch (else `DB_ERROR`);
//!    - `prevSegmentHash` is the hash of the segment (from the `segments` table) whose last
//!      record is the one before; `null` when nothing precedes;
//!    - a retry of a failed write reuses the exact bytes (and so the name) of that attempt
//!      while the lease is held, even across a re-acquisition. Encoding is deterministic
//!      anyway (`createdAt` is the records' latest `committedAt`), so a re-export after a
//!      crash is byte-identical;
//!    - otherwise a file already in the folder that starts at the same seq, continues the same
//!      predecessor and holds exactly our next records byte for byte (left by a crash after
//!      the rename) is adopted instead of writing a duplicate;
//!    - otherwise the next records, up to [`segment::MAX_RECORDS`] or [`segment::MAX_BYTES`]
//!      (a transaction is never split; commit refuses a record above
//!      [`segment::MAX_RECORD_BYTES`] with `TX_TOO_LARGE`, so every segment fits), are encoded
//!      and written with [`fsio::write_once`];
//!    - whatever `write_once` returned, the target is read back uncached and decoded. Our hash
//!      → written (this covers `AlreadyExists` after an NFS link retransmit, `NotFound` after a
//!      Windows rename that succeeded server-side, and a failed directory fsync after the
//!      rename). Another valid file under our name → `SEGMENT_COLLISION`, never overwritten;
//!      the next attempt adds 1 ms to `createdAt` for another name. Absent → retried briefly
//!      in place ([`READBACK_RETRIES`], for SMB reporting a fresh rename missing), then
//!      reported and retried on a later pass with the same bytes. Present but not verifiable
//!      (incomplete, unreadable) [`MAX_FAILED_READS`] times → treated as a collision;
//!    - only then, in one DB transaction: the segment's rows are marked `exported = 1` and its
//!      `segments` row inserted.
//!
//! Folder failures (unwritable, disk full, collision) do not fail the pass: they are
//! [`ExportReport::error`] and the store's export error, and the records stay unexported.
//!
//! # `segments.state`
//!
//! - `exported`: written by this server's export, or found in the folder after a crash and
//!   adopted, and verified by read-back in both cases.
//! - `imported` (Task 8): taken from the folder by import.
//!
//! Both count as "in the folder" when chaining `prevSegmentHash`.

mod policy;
mod prepare;
mod rows;
mod step;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use fin_core::CoreError;
use serde_json::json;

use crate::reserved::no_links;
use crate::segment;
use crate::tick::StoreAccess;
use crate::{Store, StoreError};

pub(crate) use policy::{ExportPolicy, Fence, StoreLease};
use policy::{SWEEP_EVERY, TempSweep};
use prepare::Prep;
use rows::{
    Row, last_pending, mark_exported, pending_branches, pending_rows, prev_segment, still_pending,
};
use step::{Outcome, StepIo};

/// A read-back that finds nothing is retried this many times, [`READBACK_DELAY`] apart.
pub(crate) const READBACK_RETRIES: u32 = 4;
pub(crate) const READBACK_DELAY: std::time::Duration = std::time::Duration::from_millis(50);
/// A file under our name that will not verify this many times is given up (another name).
pub(crate) const MAX_FAILED_READS: u32 = 3;

pub(crate) fn segments_root(project_dir: &Path) -> PathBuf {
    project_dir.join(".finisher").join("segments")
}

/// The directories temp files are written in: `segments/*`, `snapshots/*` and
/// `compaction/` by the holder, and `segments/` itself by a listing refresh
/// (`scan::refresh_listing`, macOS network shares). None reached through a link.
fn temp_dirs(project_dir: &Path) -> Vec<PathBuf> {
    let dot = project_dir.join(".finisher");
    let mut dirs = vec![dot.join("compaction"), dot.join("segments")];
    for sub in ["segments", "snapshots"] {
        if no_links(&dot.join(sub)).is_err() {
            continue;
        }
        for d in fs::read_dir(dot.join(sub)).into_iter().flatten().flatten() {
            if d.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(d.path());
            }
        }
    }
    dirs.retain(|d| no_links(d).is_ok());
    dirs
}

/// What one export pass did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportReport {
    /// Segment files written (adopted files are not counted).
    pub segments_written: u32,
    /// Records marked exported, adopted ones included.
    pub records: u32,
    /// A folder failure: the records stay unexported and are retried after the delay.
    pub error: Option<CoreError>,
}

/// Test hook: where the next export "crashes" (returns `TEST_FAULT` and stops, as if the
/// process died there).
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailPoint {
    /// The temp file is written; no rename.
    AfterTempWrite,
    /// The segment is in place; not read back, not marked.
    AfterRename,
    /// Read back and verified; not marked.
    BeforeMarkExported,
}

/// A prepared segment, kept across a failed write so the retry uses the same bytes and name.
#[derive(Debug)]
struct Attempt {
    branch: String,
    first_seq: u64,
    last_seq: u64,
    epoch: u64,
    prev: Option<String>,
    name: String,
    bytes: Vec<u8>,
    hash: String,
    /// `txHash` of each record, to check the rows are still the same.
    tx_hashes: Vec<String>,
    /// Read-backs that found the file but could not verify it.
    failed_reads: u32,
}

/// Export state held by the `Store`.
#[derive(Debug, Default)]
pub(crate) struct ExportState {
    policy: ExportPolicy,
    pending_since: Option<Instant>,
    last_sweep: Option<Instant>,
    temps: TempSweep,
    /// `(branch, first_seq, ms)`: added to `createdAt` after collisions at that start.
    salt: Option<(String, u64, i64)>,
    pub(crate) error: Option<CoreError>,
    attempt: Option<Attempt>,
    pub(crate) lease: StoreLease,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fail_at: Option<FailPoint>,
}

/// [`Store::close`]: a forced export pass, then the lease is released
/// ([`crate::lease::Lease::release`]) if this pass left nothing unexported. With records
/// still unexported (a folder failure, read-only, not caught up) it is not: they stay this
/// server's to export on its next open (the restart shortcut), and others wait out staleness
/// as before.
pub(crate) fn close(store: &mut Store, now: Instant) -> Result<ExportReport, StoreError> {
    let report = export_pending(store, now, true)?;
    let flushed =
        report.error.is_none() && pending_branches(&store.conn).is_ok_and(|b| b.is_empty());
    if flushed && let StoreLease::Lease(lease) = std::mem::take(&mut store.export.lease) {
        lease.release();
    }
    Ok(report)
}

/// One export pass (module docs) on a store the caller has. `Err` only for DB failures, a
/// broken DB invariant, or an injected `FailPoint`.
pub(crate) fn export_pending(
    store: &mut Store,
    now: Instant,
    force: bool,
) -> Result<ExportReport, StoreError> {
    let mut direct = crate::tick::Direct(store);
    Ok(export_with(&mut direct, now, force)?.unwrap_or_default())
}

/// One export pass through `access`: the store lock is taken only around reading the rows
/// and marking a written segment, never across folder I/O (the sweep, the fence, the write and
/// its read-back). `None` when the store went away during the pass.
pub(crate) fn export_with<A: StoreAccess>(
    access: &mut A,
    now: Instant,
    force: bool,
) -> Result<Option<ExportReport>, StoreError> {
    let mut report = ExportReport::default();
    // Read-only (a fork, a newer format, a conflict copy, not caught up after acquiring the
    // lease, not holding it): records stay unexported. The fence itself comes below.
    let Some(begin) = access.with(|s| begin(s, now)) else {
        return Ok(None);
    };
    let Some((fence, sweep, dirs_of)) = begin else {
        return Ok(Some(report));
    };
    if fence.check().is_err() {
        if let Some(t) = sweep {
            access.with(|s| s.export.temps = t);
        }
        return Ok(Some(report));
    }
    let swept = sweep.map(|mut t| {
        t.sweep(&temp_dirs(&dirs_of), now, &|| fence.quick_check().is_ok());
        t
    });
    let Some(branches) = access.with(|s| after_sweep(s, now, force, swept)) else {
        return Ok(None);
    };
    // Nothing pending, or not yet (the delay): the pass ends here.
    let Some(branches) = branches? else {
        return Ok(Some(report));
    };
    'branches: for (branch, last) in branches {
        let mut stale = false;
        loop {
            let Some(planned) = access.with(|s| plan_step(s, &branch, last)) else {
                return Ok(None);
            };
            let io = match planned? {
                Planned::Done => break,
                Planned::Failed(e) => {
                    report.error = Some(e);
                    break 'branches;
                }
                Planned::Io(io) => io,
            };
            let ran = io.run();
            let Some(step) = access.with(|s| apply_step(s, &branch, ran, now)) else {
                return Ok(None);
            };
            match step? {
                Step::Stopped => break,
                Step::Wrote(n) => {
                    report.segments_written += 1;
                    report.records += n;
                }
                Step::Adopted(n) => report.records += n,
                // Rows changed while the segment was written: plan again, once.
                Step::Stale if !stale => stale = true,
                Step::Stale => break,
                Step::Failed(e) => {
                    report.error = Some(e);
                    break 'branches;
                }
            }
        }
    }
    let error = report.error.clone();
    let Some(()) = access.with(|s| {
        s.export.pending_since = error.as_ref().map(|_| now);
        s.export.error = error;
    }) else {
        return Ok(None);
    };
    Ok(Some(report))
}

/// Under the lock: whether this server may export now (in memory), and the temp sweep if due.
fn begin(store: &mut Store, now: Instant) -> Option<(Fence, Option<TempSweep>, PathBuf)> {
    if store.read_only().is_some() || store.export.lease.quick_check().is_err() {
        return None;
    }
    let due = store
        .export
        .last_sweep
        .is_none_or(|t| now.saturating_duration_since(t) >= SWEEP_EVERY);
    let sweep = due.then(|| std::mem::take(&mut store.export.temps));
    Some((store.export.lease.fence(), sweep, store.project_dir.clone()))
}

/// Under the lock, after the sweep: the branches to export now, each with its last pending
/// seq (records committed during the pass wait for the next one, and its delay); `None` when
/// nothing is pending or the delay has not passed.
fn after_sweep(
    store: &mut Store,
    now: Instant,
    force: bool,
    swept: Option<TempSweep>,
) -> Result<Option<Vec<(String, u64)>>, StoreError> {
    if let Some(t) = swept {
        store.export.temps = t;
        store.export.last_sweep = Some(now);
    }
    let branches = pending_branches(&store.conn)?;
    if branches.is_empty() {
        store.export.pending_since = None;
        store.export.error = None;
        return Ok(None);
    }
    let since = *store.export.pending_since.get_or_insert(now);
    if !force
        && now.saturating_duration_since(since)
            < store.export.policy.delay(now, store.storage_kind())
    {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(branches.len());
    for b in branches {
        if let Some(last) = last_pending(&store.conn, &b)? {
            out.push((b, last));
        }
    }
    Ok(Some(out))
}

enum Planned {
    /// Nothing left on this branch.
    Done,
    Failed(CoreError),
    Io(Box<StepIo>),
}

enum Step {
    /// The lease does not allow writing.
    Stopped,
    Wrote(u32),
    Adopted(u32),
    /// The rows changed while the segment was written: nothing marked.
    Stale,
    Failed(CoreError),
}

/// Under the lock: the next segment of `branch` to write (or adopt), from its unexported rows
/// up to seq `last`.
fn plan_step(store: &mut Store, branch: &str, last: u64) -> Result<Planned, StoreError> {
    let mut rows = pending_rows(&store.conn, branch)?;
    rows.retain(|r| r.seq <= last);
    let Some(first) = rows.first().map(|r| r.seq) else {
        return Ok(Planned::Done);
    };
    if let Some(e) = too_large(branch, &rows[0]) {
        return Ok(Planned::Failed(e));
    }
    let prev = prev_segment(&store.conn, branch, first)?;
    let retry = store.export.attempt.take().filter(|a| {
        a.branch == branch
            && a.first_seq == first
            && a.prev == prev
            && a.tx_hashes.len() <= rows.len()
            && a.tx_hashes.iter().zip(&rows).all(|(h, r)| *h == r.tx_hash)
    });
    Ok(Planned::Io(Box::new(StepIo {
        branch: branch.to_string(),
        prep: Prep::of(store, branch, first)?,
        rows,
        prev,
        dir: segments_root(&store.project_dir).join(branch),
        retry,
        fence: store.export.lease.fence(),
        #[cfg(any(test, feature = "test-hooks"))]
        fail_at: store.export.fail_at,
        #[cfg(any(test, feature = "test-hooks"))]
        hooks: store.phase.hooks.clone(),
    })))
}

/// Under the lock: what a step's folder I/O did, applied. A written (or adopted) segment is
/// marked only if its rows are still the next unexported ones ([`still_pending`]).
fn apply_step(
    store: &mut Store,
    branch: &str,
    (outcome, wrote): (Outcome, bool),
    now: Instant,
) -> Result<Step, StoreError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if wrote {
        store.export.fail_at = None;
    }
    let _ = wrote;
    match outcome {
        Outcome::Stopped { keep } => {
            store.export.attempt = keep;
            Ok(Step::Stopped)
        }
        Outcome::Failed { error, keep } => {
            store.export.attempt = keep;
            Ok(Step::Failed(error))
        }
        Outcome::Collision {
            target,
            attempt,
            why,
        } => Ok(collision(store, &target, attempt, why)),
        Outcome::Broken(e) => Err(e),
        #[cfg(any(test, feature = "test-hooks"))]
        Outcome::Fault { keep } => {
            store.export.attempt = Some(keep);
            Err(fault())
        }
        Outcome::Wrote(a) | Outcome::Adopted(a) if !still_pending(&store.conn, branch, &a)? => {
            Ok(Step::Stale)
        }
        Outcome::Adopted(a) => {
            let n = mark_exported(&mut store.conn, branch, &a)?;
            store.phase.io_gen += 1;
            Ok(Step::Adopted(n))
        }
        Outcome::Wrote(a) => {
            let n = mark_exported(&mut store.conn, branch, &a)?;
            store.phase.io_gen += 1;
            store.export.salt = None;
            store.export.policy.wrote(now);
            Ok(Step::Wrote(n))
        }
    }
}

/// A record that cannot fit a segment every reader accepts. Commit refuses them
/// (`TX_TOO_LARGE`), but migration 0001 brings sub-project 2 history in unchecked: export
/// stops at such a record rather than write a file followers would reject.
fn too_large(branch: &str, row: &Row) -> Option<CoreError> {
    let limit = segment::MAX_RECORD_BYTES;
    (row.bytes.len() > limit).then(|| {
        CoreError::new(
            "RECORD_TOO_LARGE",
            format!(
                "record {} (seq {}) is {} bytes; a segment holds records of at most {limit}",
                row.tx_id,
                row.seq,
                row.bytes.len()
            ),
        )
        .with_details(json!({
            "branch": branch, "txId": row.tx_id, "seq": row.seq,
            "bytes": row.bytes.len(), "limit": limit,
        }))
        .with_fix("export stops at this record; nothing after it reaches the folder until it is removed or split (fin project verify reports it)")
    })
}

/// Our name holds something else: leave it, and use another name next time.
fn collision(store: &mut Store, target: &Path, a: Attempt, why: String) -> Step {
    let salt = match &store.export.salt {
        Some((b, f, ms)) if *b == a.branch && *f == a.first_seq => ms + 1,
        _ => 1,
    };
    store.export.salt = Some((a.branch.clone(), a.first_seq, salt));
    Step::Failed(
        CoreError::new(
            "SEGMENT_COLLISION",
            format!("{} already holds other content", target.display()),
        )
        .with_details(
            json!({ "path": target.to_string_lossy(), "branch": a.branch, "reason": why }),
        )
        .with_fix("the file was left untouched; export retries under another name"),
    )
}

#[cfg(any(test, feature = "test-hooks"))]
fn fault() -> StoreError {
    CoreError::new("TEST_FAULT", "injected export failure").into()
}

impl Store {
    /// Test hook: the name the next export of `branch` will write, prepared now so the export
    /// uses exactly these bytes. `None` when nothing is pending.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn planned_segment_name(&mut self, branch: &str) -> Result<Option<String>, StoreError> {
        let rows = pending_rows(&self.conn, branch)?;
        let Some(first) = rows.first().map(|r| r.seq) else {
            return Ok(None);
        };
        let lease_epoch = self.export.lease.check()?;
        let prev = prev_segment(&self.conn, branch, first)?;
        let a = Prep::of(self, branch, first)?.encode(branch, &rows, lease_epoch, prev)?;
        let name = a.name.clone();
        self.export.attempt = Some(a);
        Ok(Some(name))
    }
}
