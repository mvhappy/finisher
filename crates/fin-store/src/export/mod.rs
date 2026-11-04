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
//! 2. Removes our own `.~fin-*.tmp` files under `segments/*` once each has been seen unchanged
//!    (same length and mtime) for [`policy::TEMP_MAX_AGE`] (2×TTL) by the monotonic clock,
//!    checked at most every [`SWEEP_EVERY`]. The mtime is the server's clock and is never
//!    compared with ours. The lease holder is the only writer, so these are crash leftovers.
//! 3. Debounce: the first pass that sees unexported records starts the clock; they are written
//!    once [`ExportPolicy::delay`] has passed (2 s, or 30 s under churn). `force` (close) skips
//!    the wait. After a failure the clock restarts, so a broken folder is retried once per delay.
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
mod rows;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use fin_core::CoreError;
use serde_json::json;

use crate::names::{self, SegmentName};
use crate::segment::{self, Header};
use crate::{Store, StoreError, fsio};

pub(crate) use policy::{ExportPolicy, StoreLease};
use policy::{SWEEP_EVERY, TempSweep};
use rows::{Row, broken, mark_exported, pending_branches, pending_rows, prev_segment};

/// A read-back that finds nothing is retried this many times, [`READBACK_DELAY`] apart.
pub(crate) const READBACK_RETRIES: u32 = 4;
pub(crate) const READBACK_DELAY: std::time::Duration = std::time::Duration::from_millis(50);
/// A file under our name that will not verify this many times is given up (another name).
pub(crate) const MAX_FAILED_READS: u32 = 3;

pub(crate) fn segments_root(project_dir: &Path) -> PathBuf {
    project_dir.join(".finisher").join("segments")
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

/// One export pass (module docs). `Err` only for DB failures, a broken DB invariant, or an
/// injected `FailPoint`.
pub(crate) fn export_pending(
    store: &mut Store,
    now: Instant,
    force: bool,
) -> Result<ExportReport, StoreError> {
    let mut report = ExportReport::default();
    // Read-only (a fork, a newer format, a conflict copy, not caught up after acquiring the
    // lease, not holding it): records stay unexported. The fence itself comes below.
    if store.read_only().is_some() || store.export.lease.check().is_err() {
        return Ok(report);
    }
    if store
        .export
        .last_sweep
        .is_none_or(|t| now.saturating_duration_since(t) >= SWEEP_EVERY)
    {
        store.export.last_sweep = Some(now);
        let root = segments_root(&store.project_dir);
        let ExportState { temps, lease, .. } = &mut store.export;
        temps.sweep(&root, now, &|| lease.quick_check().is_ok());
    }
    let branches = pending_branches(&store.conn)?;
    if branches.is_empty() {
        store.export.pending_since = None;
        store.export.error = None;
        return Ok(report);
    }
    let since = *store.export.pending_since.get_or_insert(now);
    if !force && now.saturating_duration_since(since) < store.export.policy.delay(now) {
        return Ok(report);
    }
    'branches: for branch in branches {
        loop {
            match export_one(store, &branch)? {
                Step::Done | Step::Stopped => break,
                Step::Wrote(n) => {
                    report.segments_written += 1;
                    report.records += n;
                    store.export.policy.wrote(now);
                }
                Step::Adopted(n) => report.records += n,
                Step::Failed(e) => {
                    report.error = Some(e);
                    break 'branches;
                }
            }
        }
    }
    store.export.error = report.error.clone();
    store.export.pending_since = report.error.as_ref().map(|_| now);
    Ok(report)
}

enum Step {
    /// Nothing left on this branch.
    Done,
    /// The lease does not allow writing.
    Stopped,
    Wrote(u32),
    Adopted(u32),
    Failed(CoreError),
}

/// Writes (or adopts) the next segment of `branch`.
fn export_one(store: &mut Store, branch: &str) -> Result<Step, StoreError> {
    let rows = pending_rows(&store.conn, branch)?;
    let Some(first) = rows.first().map(|r| r.seq) else {
        return Ok(Step::Done);
    };
    let Ok(lease_epoch) = store.export.lease.check() else {
        return Ok(Step::Stopped);
    };
    if let Some(e) = too_large(branch, &rows[0]) {
        return Ok(Step::Failed(e));
    }
    let prev = prev_segment(&store.conn, branch, first)?;
    let dir = segments_root(&store.project_dir).join(branch);
    if let Err(e) = fs::create_dir_all(&dir) {
        return Ok(Step::Failed(folder_error(&dir, branch, &e)));
    }
    let retry = store.export.attempt.take().filter(|a| {
        a.branch == branch
            && a.first_seq == first
            && a.prev == prev
            && a.tx_hashes.len() <= rows.len()
            && a.tx_hashes.iter().zip(&rows).all(|(h, r)| *h == r.tx_hash)
    });
    let attempt = match retry {
        Some(a) => a,
        None => {
            if let Some(found) = adoptable(store, branch, &dir, &rows, prev.as_deref()) {
                let n = mark_exported(&mut store.conn, branch, &found)?;
                return Ok(Step::Adopted(n));
            }
            prepare(store, branch, &rows, lease_epoch, prev)?
        }
    };
    write(store, &dir, attempt)
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

/// Writes `a`, verifies it by read-back and marks it.
fn write(store: &mut Store, dir: &Path, a: Attempt) -> Result<Step, StoreError> {
    #[cfg(any(test, feature = "test-hooks"))]
    let fail_at = store.export.fail_at.take();
    #[cfg(any(test, feature = "test-hooks"))]
    if fail_at == Some(FailPoint::AfterTempWrite) {
        let _ = fs::write(dir.join(names::temp_name()), &a.bytes);
        store.export.attempt = Some(a);
        return Err(fault());
    }
    // The fence, right before the write.
    if !store.export.lease.check().is_ok_and(|e| a.epoch <= e) {
        store.export.attempt = Some(a);
        return Ok(Step::Stopped);
    }
    let written = fsio::write_once(dir, &a.name, &a.bytes);
    #[cfg(any(test, feature = "test-hooks"))]
    if fail_at == Some(FailPoint::AfterRename) {
        store.export.attempt = Some(a);
        return Err(fault());
    }
    let target = dir.join(&a.name);
    // Absent after a write that did not plainly fail: SMB may not show a fresh rename yet.
    let plain_failure = written.as_ref().is_err_and(|e| {
        !matches!(
            e.kind(),
            io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
        )
    });
    // Only a write that put a file in place (or found one there) makes an unverifiable
    // read-back evidence of a foreign file; otherwise it is the folder failing.
    let file_in_place = written
        .as_ref()
        .map_or_else(|e| e.kind() == io::ErrorKind::AlreadyExists, |()| true);
    let retries = if plain_failure { 0 } else { READBACK_RETRIES };
    let mut a = a;
    match read_back(&target, &a.hash, retries) {
        ReadBack::Ours => {}
        ReadBack::Other(why) => return Ok(collision(store, &target, a, why)),
        ReadBack::Missing(read_err) if read_err.kind() == io::ErrorKind::NotFound => {
            let branch = a.branch.clone();
            store.export.attempt = Some(a);
            let e = written.err().unwrap_or(read_err);
            return Ok(Step::Failed(folder_error(&target, &branch, &e)));
        }
        ReadBack::Missing(read_err) if !file_in_place => {
            let branch = a.branch.clone();
            store.export.attempt = Some(a);
            let e = written.err().unwrap_or(read_err);
            return Ok(Step::Failed(folder_error(&target, &branch, &e)));
        }
        ReadBack::Missing(read_err) => {
            a.failed_reads += 1;
            if a.failed_reads >= MAX_FAILED_READS {
                let why = format!("{} read-backs failed: {read_err}", a.failed_reads);
                return Ok(collision(store, &target, a, why));
            }
            let branch = a.branch.clone();
            store.export.attempt = Some(a);
            return Ok(Step::Failed(folder_error(&target, &branch, &read_err)));
        }
    }
    #[cfg(any(test, feature = "test-hooks"))]
    if fail_at == Some(FailPoint::BeforeMarkExported) {
        store.export.attempt = Some(a);
        return Err(fault());
    }
    let n = mark_exported(&mut store.conn, &a.branch, &a)?;
    store.export.salt = None;
    Ok(Step::Wrote(n))
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

enum ReadBack {
    Ours,
    /// A complete file with other content (or damaged) under our name.
    Other(String),
    /// Absent, unreadable or still incomplete.
    Missing(io::Error),
}

/// Reads `path` back uncached; `NotFound` is retried `retries` times.
fn read_back(path: &Path, hash: &str, retries: u32) -> ReadBack {
    let mut left = retries;
    let bytes = loop {
        match fsio::read_uncached(path) {
            Ok(b) => break b,
            Err(e) if e.kind() == io::ErrorKind::NotFound && left > 0 => {
                left -= 1;
                std::thread::sleep(READBACK_DELAY);
            }
            Err(e) => return ReadBack::Missing(e),
        }
    };
    match segment::decode(&bytes) {
        Ok(seg) if seg.hash == hash => ReadBack::Ours,
        Ok(seg) => ReadBack::Other(format!("hash {} is not ours", seg.hash)),
        Err(segment::SegError::Incomplete) => ReadBack::Missing(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the written segment reads back incomplete",
        )),
        Err(e) => ReadBack::Other(e.to_string()),
    }
}

/// A folder failure as reported in status: the records stay unexported.
fn folder_error(path: &Path, branch: &str, e: &io::Error) -> CoreError {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    CoreError::new(
        "PROJECT_DIR_UNWRITABLE",
        format!("cannot export to {}: {e}", path.display()),
    )
    .with_details(json!({ "path": path.to_string_lossy(), "branch": branch, "reason": e.to_string() }))
    .with_fix("make the project folder writable or free space; export retries automatically and nothing is lost")
}

#[cfg(any(test, feature = "test-hooks"))]
fn fault() -> StoreError {
    CoreError::new("TEST_FAULT", "injected export failure").into()
}

/// Encodes the next segment from `rows`: as many as fit the caps, at least one.
fn prepare(
    store: &Store,
    branch: &str,
    rows: &[Row],
    lease_epoch: u64,
    prev: Option<String>,
) -> Result<Attempt, StoreError> {
    let salt = match &store.export.salt {
        Some((b, f, ms)) if b == branch && Some(*f) == rows.first().map(|r| r.seq) => *ms,
        _ => 0,
    };
    let mut n = 0;
    let mut body = 0;
    for row in rows {
        let line = row.bytes.len() + 1;
        if n > 0
            && (n == segment::MAX_RECORDS
                || body + line + segment::FRAME_RESERVE > segment::MAX_BYTES)
        {
            break;
        }
        n += 1;
        body += line;
    }
    loop {
        let chunk = &rows[..n];
        let first = chunk[0].seq;
        let last = chunk[n - 1].seq;
        let versions = chunk.iter().map(|r| r.schema_version);
        // The highest commit epoch, never the epoch at export (module docs).
        let epoch = chunk.iter().map(|r| r.epoch).max().unwrap_or(0);
        if epoch > lease_epoch {
            return Err(broken(
                branch,
                format!("records committed at epoch {epoch}, above the lease epoch {lease_epoch}"),
            ));
        }
        let created_at = chunk.iter().map(|r| r.committed_at).max().unwrap_or(0) + salt;
        let header = Header {
            format_version: segment::FORMAT_VERSION,
            min_reader_version: segment::MIN_READER_VERSION,
            project_id: store.project_id.clone(),
            branch: branch.to_string(),
            writer_id: store.server_id.clone(),
            epoch,
            first_seq: first,
            last_seq: last,
            prev_segment_hash: prev.clone(),
            op_schema_versions: [
                versions.clone().min().unwrap_or(0),
                versions.max().unwrap_or(0),
            ],
            created_at,
            pack: false,
            replaces: Vec::new(),
        };
        let lines: Vec<&[u8]> = chunk.iter().map(|r| r.bytes.as_slice()).collect();
        let (bytes, hash) = segment::encode(&header, &lines);
        if bytes.len() > segment::MAX_BYTES {
            if n > 1 {
                n -= 1;
                continue;
            }
            // `too_large` stops export before such a record; never write it.
            return Err(broken(
                branch,
                format!("seq {first} does not fit a segment"),
            ));
        }
        let name = names::hash12(&hash)
            .and_then(|h| SegmentName::new(epoch, first, last, h))
            .ok_or_else(|| {
                broken(
                    branch,
                    format!("epoch {epoch} or seqs {first}..={last} do not fit a segment name"),
                )
            })?;
        return Ok(Attempt {
            branch: branch.to_string(),
            first_seq: first,
            last_seq: last,
            epoch,
            prev,
            name: name.to_string(),
            bytes,
            hash,
            tx_hashes: chunk.iter().map(|r| r.tx_hash.clone()).collect(),
            failed_reads: 0,
        });
    }
}

/// A segment already in `dir` that holds exactly our next records from the same predecessor:
/// left by a crash between the rename and the marking. The longest one wins.
fn adoptable(
    store: &Store,
    branch: &str,
    dir: &Path,
    rows: &[Row],
    prev: Option<&str>,
) -> Option<Attempt> {
    let first = rows[0].seq;
    let last = rows[rows.len() - 1].seq;
    let mut candidates: Vec<SegmentName> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| SegmentName::parse(e.ok()?.file_name().to_str()?))
        .filter(|n| n.first_seq() == first && n.last_seq() <= last)
        .collect();
    candidates.sort_by_key(|n| std::cmp::Reverse(n.last_seq()));
    candidates.into_iter().find_map(|name| {
        let bytes = fsio::read_uncached(&dir.join(name.to_string())).ok()?;
        let seg = segment::decode(&bytes).ok()?;
        let h = &seg.header;
        let same = h.branch == branch
            && h.project_id == store.project_id
            && h.prev_segment_hash.as_deref() == prev
            && !h.pack
            && h.epoch == name.epoch()
            && names::hash12(&seg.hash) == Some(name.hash12())
            && seg.records.len() as u64 == name.last_seq() - first + 1
            && seg.records.iter().zip(rows).all(|(r, row)| *r == row.bytes);
        same.then(|| Attempt {
            branch: branch.to_string(),
            first_seq: first,
            last_seq: name.last_seq(),
            epoch: h.epoch,
            prev: prev.map(str::to_string),
            name: name.to_string(),
            bytes: Vec::new(),
            hash: seg.hash.clone(),
            tx_hashes: rows[..seg.records.len()]
                .iter()
                .map(|r| r.tx_hash.clone())
                .collect(),
            failed_reads: 0,
        })
    })
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
        let a = prepare(self, branch, &rows, lease_epoch, prev)?;
        let name = a.name.clone();
        self.export.attempt = Some(a);
        Ok(Some(name))
    }
}
