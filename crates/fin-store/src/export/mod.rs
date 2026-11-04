//! Export (sub-project 4 spec §3.4 "Export cadence"): committed records go to
//! `.finisher/segments/<branch>/` as verified, chained, write-once segments.
//!
//! # One pass ([`export_pending`])
//!
//! 1. Only while this server holds the lease ([`ExportLease::check`]). The lease is checked
//!    again right before every write, and the segment carries the epoch that check returned:
//!    the lease epoch at export, not the epoch the records were committed at. A segment is the
//!    writer's fenced act, so it is labelled with the fence it passed (spec §3.6 step 6: "before
//!    this server has exported anything *at its epoch*"). One segment therefore never mixes
//!    epochs, whatever the commit epochs of its records.
//! 2. Removes our own `.~fin-*.tmp` files older than [`policy::TEMP_MAX_AGE`] (2×TTL) under
//!    `segments/*`, at most every [`SWEEP_EVERY`]. The lease holder is the only writer, so they
//!    are crash leftovers.
//! 3. Debounce: the first pass that sees unexported records starts the clock; they are written
//!    once [`ExportPolicy::delay`] has passed (2 s, or 30 s under churn). `force` (close) skips
//!    the wait. After a failure the clock restarts, so a broken folder is retried once per delay.
//! 4. Per branch, from its lowest unexported seq, one segment at a time:
//!    - the exported rows must be a seq prefix of the branch (else `DB_ERROR`);
//!    - `prevSegmentHash` is the hash of the segment (from the `segments` table) whose last
//!      record is the one before; `null` when nothing precedes;
//!    - a retry of a failed write reuses the exact bytes (and so the name) of that attempt,
//!      never a re-encoding with a new `createdAt`;
//!    - otherwise a file already in the folder that starts at the same seq, continues the same
//!      predecessor and holds exactly our next records byte for byte (left by a crash after
//!      the rename) is adopted instead of writing a duplicate;
//!    - otherwise the next records, up to [`segment::MAX_RECORDS`] or [`segment::MAX_BYTES`]
//!      (one transaction is never split, so one large record may exceed the bytes cap), are
//!      encoded and written with [`fsio::write_once`];
//!    - whatever `write_once` returned, the target is read back uncached and decoded. Our hash
//!      → written (this covers `AlreadyExists` after an NFS link retransmit, `NotFound` after a
//!      Windows rename that succeeded server-side, and a failed directory fsync after the
//!      rename). Another valid file under our name → `SEGMENT_COLLISION`, never overwritten.
//!      Absent or unreadable → the write error, retried later;
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

pub(crate) use policy::{ExportLease, ExportPolicy};
use policy::{SWEEP_EVERY, sweep_temps};
use rows::{Row, broken, mark_exported, pending_branches, pending_rows, prev_segment};

/// Room left for the header and trailer when filling a segment (both are far smaller).
const FRAME_RESERVE: usize = 1024;

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
}

/// Export state held by the `Store`.
#[derive(Debug, Default)]
pub(crate) struct ExportState {
    policy: ExportPolicy,
    pending_since: Option<Instant>,
    last_sweep: Option<Instant>,
    pub(crate) error: Option<CoreError>,
    attempt: Option<Attempt>,
    pub(crate) lease: ExportLease,
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
    if store.export.lease.check().is_err() {
        return Ok(report);
    }
    if store
        .export
        .last_sweep
        .is_none_or(|t| now.saturating_duration_since(t) >= SWEEP_EVERY)
    {
        store.export.last_sweep = Some(now);
        sweep_temps(&segments_root(&store.project_dir));
    }
    let branches = pending_branches(&store.conn)?;
    if branches.is_empty() {
        store.export.pending_since = None;
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
    let Ok(epoch) = store.export.lease.check() else {
        return Ok(Step::Stopped);
    };
    let prev = prev_segment(&store.conn, branch, first)?;
    let dir = segments_root(&store.project_dir).join(branch);
    if let Err(e) = fs::create_dir_all(&dir) {
        return Ok(Step::Failed(folder_error(&dir, branch, &e)));
    }
    let retry = store.export.attempt.take().filter(|a| {
        a.branch == branch
            && a.first_seq == first
            && a.epoch == epoch
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
            prepare(store, branch, &rows, epoch, prev)?
        }
    };
    write(store, &dir, attempt)
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
    // The fence: right before the write, at the epoch the segment carries.
    if store.export.lease.check().ok() != Some(a.epoch) {
        return Ok(Step::Stopped);
    }
    let written = fsio::write_once(dir, &a.name, &a.bytes);
    #[cfg(any(test, feature = "test-hooks"))]
    if fail_at == Some(FailPoint::AfterRename) {
        store.export.attempt = Some(a);
        return Err(fault());
    }
    let target = dir.join(&a.name);
    match read_back(&target, &a.hash) {
        ReadBack::Ours => {}
        ReadBack::Other(why) => {
            // Dropped: the next attempt re-encodes, which gives another name.
            return Ok(Step::Failed(
                CoreError::new(
                    "SEGMENT_COLLISION",
                    format!("{} already holds other content", target.display()),
                )
                .with_details(
                    json!({ "path": target.to_string_lossy(), "branch": a.branch, "reason": why }),
                )
                .with_fix("the file was left untouched; export retries under another name"),
            ));
        }
        ReadBack::Missing(read_err) => {
            let branch = a.branch.clone();
            store.export.attempt = Some(a);
            let e = written.err().unwrap_or(read_err);
            return Ok(Step::Failed(folder_error(&target, &branch, &e)));
        }
    }
    #[cfg(any(test, feature = "test-hooks"))]
    if fail_at == Some(FailPoint::BeforeMarkExported) {
        store.export.attempt = Some(a);
        return Err(fault());
    }
    let n = mark_exported(&mut store.conn, &a.branch, &a)?;
    Ok(Step::Wrote(n))
}

enum ReadBack {
    Ours,
    /// A complete file with other content (or damaged) under our name.
    Other(String),
    /// Absent, unreadable or still incomplete.
    Missing(io::Error),
}

fn read_back(path: &Path, hash: &str) -> ReadBack {
    let bytes = match fsio::read_uncached(path) {
        Ok(b) => b,
        Err(e) => return ReadBack::Missing(e),
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
    epoch: u64,
    prev: Option<String>,
) -> Result<Attempt, StoreError> {
    let mut n = 0;
    let mut body = 0;
    for row in rows {
        let line = row.bytes.len() + 1;
        if n > 0 && (n == segment::MAX_RECORDS || body + line + FRAME_RESERVE > segment::MAX_BYTES)
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
            created_at: crate::commit::now_ms(),
            pack: false,
            replaces: Vec::new(),
        };
        let lines: Vec<&[u8]> = chunk.iter().map(|r| r.bytes.as_slice()).collect();
        let (bytes, hash) = segment::encode(&header, &lines);
        if bytes.len() > segment::MAX_BYTES && n > 1 {
            n -= 1;
            continue;
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
        let epoch = self.export.lease.check()?;
        let prev = prev_segment(&self.conn, branch, first)?;
        let a = prepare(self, branch, &rows, epoch, prev)?;
        let name = a.name.clone();
        self.export.attempt = Some(a);
        Ok(Some(name))
    }
}
