//! Compaction (sub-project 4 spec §3.9): many small segments of a branch become a few packs
//! that hold the same records byte for byte, so history is kept whole. Compaction is the only
//! code that deletes files from the project folder, and what it deletes may be another
//! machine's only copy of history, so every step re-checks, from a fresh read, what it acts on.
//!
//! # When ([`run`]; `tick` calls it on every poll, [`Store::compact`] on demand)
//!
//! Only while this server holds the lease, the project is writable (`read_only()` is `None`:
//! not `CATCHING_UP` after an acquisition, no fork, no conflict copy, no newer format, no
//! import error), and a full lease check passes. Followers never compact. Losing the lease
//! forgets every grace period observed.
//!
//! # Phase 1 (per branch, [`pack`])
//!
//! When the branch has more than [`TRIGGER`] non-pack segments that no present pack replaces
//! (or `force`), and no compaction of it is pending (nor any manifest that cannot be read,
//! whose branch is unknown):
//! 1. **Read afresh** ([`view::ViewPlan::scan`]): an uncached scan, a fresh resolution (no fork, nothing
//!    waiting or pending, every file under `segments/<branch>/` settled), and the database's
//!    exported rows exactly the chain's prefix. Otherwise wait (retried after [`RETRY`]). A
//!    4B branch is the segments naming its index record, resolved from its fork point
//!    (plan 4B Decision 33); the directory's other segments are never packed or deleted, and
//!    a manifest naming one is abandoned.
//! 2. **Eligible**: non-pack segments wholly on the resolved chain and exported in the
//!    database, not set aside (a losing, stale, malformed or quarantined segment never is:
//!    it is what lets a late follower reach the same verdict, and 4B's `recovered-*` branches
//!    need it), not held by an abandoned compaction, not named as predecessor by a malformed
//!    file (once gone, the file could link through the pack), and linked to a present
//!    predecessor.
//! 3. **Packs**: runs of consecutive eligible segments with non-decreasing epochs, cut at
//!    [`crate::segment::MAX_RECORDS`] / [`crate::segment::MAX_BYTES`] (whole segments, so a transaction is
//!    never split), at least two segments each. A pack holds the records byte for byte,
//!    lists each replaced segment as `{"hash", "lastTxHash"}` (its last record: a pack
//!    stands in for a deleted segment only as that record, `crate::chain`),
//!    `prevSegmentHash` = its first segment's, epoch = the highest it replaces (its last
//!    segment's; duplicates inside its range at a higher epoch are left alone), `createdAt`
//!    = the latest it replaces (so a re-planned pack is the same file), our `writerId` and
//!    the genesis hash. Packs are never re-packed.
//! 4. **Write**, each step behind a full lease check, write-once (temp → fsync → no-replace
//!    rename → dir fsync) and read back uncached: the snapshot at the branch's exported head
//!    (or a verified one already there), each pack (then its `segments` row, state
//!    `imported`: a pack carries other writers' records), then the manifest
//!    (`compaction/<ulid>.json`, [`manifest`]). Its grace period starts now.
//!
//! Interrupted before the manifest, the packs are duplicates that readers skip by `txHash`;
//! the next phase 1 plans the same packs again (deterministic bytes, so they are adopted).
//!
//! # Phase 2 (per manifest, [`finish`])
//!
//! Each manifest in `compaction/`, ours or another server's, is observed on this server's
//! monotonic, suspend-aware lease clock: its grace period ([`grace`]: 10 min, 24 h when this
//! folder or the writer's is a sync folder) runs from the first poll that, holding the lease,
//! found all its packs and its snapshot present (a sync folder may deliver the manifest
//! first), never from its `createdMs` or a file time. Once it has passed: fence at the
//! manifest's epoch, read afresh again, check the packs, the snapshot and every replaced
//! segment against the chain and the database (records byte for byte; no replaced segment
//! above its pack's epoch), then resolve the branch once more **without** the files to be
//! deleted, for a fresh reader and for this database: the verdicts (chain, what is set aside
//! and why, fork, waiting, pending) must be the same as with them. Only then delete the
//! replaced files (a full lease check before each), then the manifest. A file or check that
//! is not settled waits ([`RETRY`]); anything that contradicts the manifest abandons it:
//! nothing more is deleted for it, it stays in the folder and in status, the database
//! remembers it (`meta` `compactionAbandoned`, across restarts), and its segments are not
//! compacted again, until `force` checks it again. Abandoning is per server: the manifest
//! stays in the folder, and another server that later holds the lease checks it afresh
//! after its own grace period.
//!
//! Export and import chain a new segment from an original segment rather than a pack ending
//! at the same seq, and import never records a pack the resolution sets aside.
//!
//! Interrupted during phase 2, the next holder finishes it: absent replaced files are skipped.
//!
//! # Locking
//!
//! A run goes through a [`crate::StoreAccess`] step by step ([`run_with`]: the phased tick and
//! fin-node's automatic step, `Store::run_due_compaction_step_phased`; `fin project compact`
//! has the store throughout), taking the store only for database work and in-memory state:
//! - **Without the store:** the `compaction/` listing and the manifests' reads (also the light
//!   tick's due check), every full lease check (through the fence of the lease in force when
//!   the run started: one the store let go fails at once), the fresh read of a branch (scan,
//!   verification, resolution), reading a snapshot, writing the snapshot, the packs and the
//!   manifest, the "would a verdict change" resolutions, directory fsyncs.
//! - **With the store:** planning (what the read needs from the database), checking each read
//!   against the database as it is then (exported rows, the branch's record and fork point,
//!   the snapshot's state), the pack plan, the `segments` rows, the abandoned list, the
//!   grace periods, and **every delete** (`remove_file`, with the in-memory lease check).
//! - **Overtaken:** a run notes `Phase::io_gen` and the lease when it starts (bumping it:
//!   an import read before is stale). Each step with the store first checks both are
//!   unchanged; otherwise the run ends there (`skipped`, nothing abandoned, no retry delay)
//!   and the next one reads afresh. `io_gen` moves with everything that changes what the
//!   checks rest on: an applied import, a marked export, another compaction run (`fin
//!   project compact`), a rebuild, a recovery, a lease change.
//! - **Why a delete is safe:** each delete runs with the store, after the same `io_gen` and
//!   lease check, so it rests on a fresh read checked against the database as it is at the
//!   delete. Whatever ran with the store since the check (a short call, or a long call that
//!   took it, each atomic with respect to the delete) either moved `io_gen` or left the
//!   folder files and everything the check rests on as they were (a commit adds only an
//!   unexported row above the branch's exported head; status, verify and other reads change
//!   nothing), so it commutes with the delete as it would with one made under the lock
//!   throughout. A tick's import and export, the only other folder I/O without the store,
//!   never run meanwhile (one at a time). Other servers are kept out by the lease as before:
//!   a full check without the store right before every delete, the in-memory check with it. Writes need no such care: a pack or
//!   snapshot is a duplicate until a manifest names it, and a manifest only starts a grace
//!   period after which phase 2 checks everything afresh.
//! - One step at a time: the automatic step takes the phased tick's guard (no tick's import
//!   reads the folder meanwhile); a panic in the folder I/O unwinds to the caller (fin-node:
//!   the engine is `Panicked`).

/// Runs `$f` on the store through `$access`; returns `Ok(None)` from the caller when the store
/// is gone (closed, taken by a long call).
macro_rules! with {
    ($access:expr, $f:expr) => {
        match $access.with($f) {
            Some(r) => r,
            None => return Ok(None),
        }
    };
}

mod finish;
mod manifest;
mod pack;
mod phased;
mod planned;
mod state;
mod view;

use std::io;
use std::path::Path;
use std::time::Duration;

use crate::storage_kind::StorageKind;
use crate::tick::Direct;
use crate::{Store, StoreError, fsio};
pub(crate) use phased::{due_with, run_with};
pub(crate) use state::CompactState;
pub use state::{CompactReport, CompactionStatus, PendingCompaction};

/// A branch is compacted once it has more than this many non-pack segments.
pub const TRIGGER: usize = 200;
/// A step that had to wait is tried again after this long…
pub(crate) const RETRY: Duration = Duration::from_secs(60);
/// …and a branch above the trigger with nothing worth packing is planned again after this.
pub(crate) const REPLAN: Duration = Duration::from_secs(10 * 60);

/// Why a run ended at a step that something else overtook (module docs, "Locking").
pub(crate) const OVERTAKEN: &str =
    "the folder or the database changed during the compaction step; it runs again afresh";

/// The grace period between phase 1 and phase 2 on `kind` (spec §3.9).
pub fn grace(kind: StorageKind) -> Duration {
    if kind.is_sync() {
        Duration::from_secs(24 * 60 * 60)
    } else {
        Duration::from_secs(10 * 60)
    }
}

/// Test hook: where compaction "crashes" (returns `TEST_FAULT` and stops,
/// `Store::fail_next_compaction_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactFail {
    /// The snapshot is in place; no pack yet.
    AfterSnapshot,
    /// The first pack is in place (and recorded); no manifest.
    AfterPack,
    /// The manifest is in place.
    AfterManifest,
    /// The first replaced segment is deleted.
    AfterDelete,
    /// Every replaced segment is deleted; the manifest is not.
    BeforeManifestDelete,
}

/// The outcome of a step.
pub(super) enum Step<T> {
    Done(T),
    /// Not settled: retried later.
    Wait(String),
    /// Contradicts the manifest: never acted on.
    Abandon(String),
    /// The lease is not held.
    Stopped(String),
    /// Something changed what the step read against ([`OVERTAKEN`]): nothing more is done
    /// and nothing is noted; the next run reads afresh.
    Overtaken,
    /// The manifest is gone (finished or removed by someone else meanwhile).
    Gone,
}

pub(super) enum Fence {
    Held(u64),
    Lost(String),
}

/// A lease check's verdict at `min_epoch`; `Lost` also when our epoch is below it (a file
/// labelled above the lease we hold).
pub(super) fn fence_at(checked: Result<u64, fin_core::CoreError>, min_epoch: u64) -> Fence {
    match checked {
        Ok(e) if e >= min_epoch => Fence::Held(e),
        Ok(e) => Fence::Lost(format!(
            "a file at epoch {min_epoch} is above our lease ({e})"
        )),
        Err(e) => Fence::Lost(e.message),
    }
}

/// `write_once` of `dir/name`, then an uncached read-back that `check` must accept (also
/// when the name already existed: identical content is adopted, anything else is refused).
pub(super) fn fsio_write(
    dir: &Path,
    name: &str,
    bytes: &[u8],
    check: &dyn Fn(&[u8]) -> bool,
) -> Result<(), String> {
    let no_links = || crate::reserved::no_links(dir).map_err(|e| e.to_string());
    no_links()?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    no_links()?;
    let written = fsio::write_once(dir, name, bytes);
    let path = dir.join(name);
    let mut left = crate::export::READBACK_RETRIES;
    let read = loop {
        match fsio::read_uncached(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound && left > 0 => {
                left -= 1;
                std::thread::sleep(crate::export::READBACK_DELAY);
            }
            r => break r,
        }
    };
    match read {
        Ok(b) if check(&b) => Ok(()),
        Ok(_) => Err(format!("{} holds other content", path.display())),
        Err(e) => Err(format!(
            "cannot write {}: {}",
            path.display(),
            written.err().unwrap_or(e)
        )),
    }
}

/// Test hook: an injected crash at `point`.
pub(super) fn hit(store: &mut Store, point: CompactFail) -> Result<(), StoreError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if store.compact.fail_at == Some(point) {
        store.compact.fail_at = None;
        return Err(fin_core::CoreError::new(
            "TEST_FAULT",
            format!("injected compaction failure at {point:?}"),
        )
        .into());
    }
    let _ = (store, point);
    Ok(())
}

/// One compaction run on a store the caller has (module docs). `Err` only for a database
/// failure (or an injected fault); folder problems are in the report and status.
pub(crate) fn run(
    store: &mut Store,
    now_ms: i64,
    force: bool,
) -> Result<CompactReport, StoreError> {
    run_bounded(store, now_ms, force, usize::MAX)
}

/// [`run`], doing at most `units` steps (a manifest's phase 2 or a branch's phase 1); what
/// is left stays due for the next run.
pub(crate) fn run_bounded(
    store: &mut Store,
    now_ms: i64,
    force: bool,
    units: usize,
) -> Result<CompactReport, StoreError> {
    // `Direct` always has the store: the run goes to its end.
    Ok(run_with(&mut Direct(store), now_ms, force, units)?.unwrap_or_default())
}
