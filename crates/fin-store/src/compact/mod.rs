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
//! 1. **Read afresh** ([`view::read`]): an uncached scan, a fresh resolution (no fork, nothing
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

mod finish;
mod manifest;
mod pack;
mod state;
mod view;

use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::names::ManifestName;
use crate::storage_kind::StorageKind;
use crate::{Store, StoreError, db, fsio};
use manifest::Manifest;
pub(crate) use state::CompactState;
pub use state::{CompactReport, CompactionStatus, PendingCompaction};
use state::{Seen, State};

/// A branch is compacted once it has more than this many non-pack segments.
pub const TRIGGER: usize = 200;
/// A step that had to wait is tried again after this long…
pub(crate) const RETRY: Duration = Duration::from_secs(60);
/// …and a branch above the trigger with nothing worth packing is planned again after this.
pub(crate) const REPLAN: Duration = Duration::from_secs(10 * 60);

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
}

pub(super) enum Fence {
    Held(u64),
    Lost(String),
}

/// The full lease check before a folder write or delete; `Lost` also when our epoch is below
/// `min_epoch` (a file labelled above the lease we hold).
pub(super) fn fence(store: &Store, min_epoch: u64) -> Fence {
    match store.export.lease.check() {
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
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
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

/// Non-pack segments of `branch` that no present pack replaces, from the last import's scan.
fn unpacked(store: &Store, branch: &str) -> usize {
    let segs: Vec<_> = store
        .import
        .segrefs()
        .filter(|s| s.header().branch == branch)
        .collect();
    let replaced: HashSet<&str> = segs
        .iter()
        .filter(|s| s.header().pack)
        .flat_map(|s| s.header().replaces.iter().map(|r| r.hash.as_str()))
        .collect();
    segs.iter()
        .filter(|s| !s.header().pack && !replaced.contains(s.hash()))
        .count()
}

/// One compaction run (module docs). `Err` only for a database failure (or an injected
/// fault); folder problems are in the report and status.
pub(crate) fn run(
    store: &mut Store,
    now_ms: i64,
    force: bool,
) -> Result<CompactReport, StoreError> {
    let mut report = CompactReport::default();
    if let Some(why) = cannot_run(store) {
        report.skipped = Some(why);
        return Ok(report);
    }
    let now = store.ctl.now();
    if force {
        store.compact.retry_plan.clear();
        state::set_abandoned(store, &Default::default())?;
        for s in store.compact.seen.values_mut() {
            s.retry_at = None;
            if matches!(s.state, State::Abandoned(_)) {
                s.state = State::Grace;
            }
        }
    }
    let dir = store.project_dir.join(".finisher").join("compaction");
    let names = match manifest::list(&dir) {
        Ok(n) => n,
        Err(e) => {
            store.compact.last_error = Some(format!("cannot list {}: {e}", dir.display()));
            return Ok(report);
        }
    };
    state::observe(store, &dir, &names, now, now_ms)?;
    if phase2(store, &mut report, now)? {
        return Ok(report);
    }
    for branch in db::branches(&store.conn)? {
        if !branch_due(store, &branch, now, force) {
            continue;
        }
        if phase1(store, &mut report, &branch, now, now_ms)? {
            break;
        }
    }
    let worked = !report.manifests.is_empty() || !report.finished.is_empty() || report.deleted > 0;
    if worked {
        store.compact.last_run_ms = Some(now_ms);
    }
    store.compact.last_error = report
        .waiting
        .last()
        .or(report.abandoned.last())
        .map(|(what, why)| format!("{what}: {why}"));
    Ok(report)
}

/// Why compaction cannot run now (read-only, not holding); forgets the manifests seen then.
fn cannot_run(store: &mut Store) -> Option<String> {
    if let Some(r) = store.read_only() {
        store.compact.seen.clear();
        return Some(format!("the project is read-only ({})", r.code()));
    }
    // In memory only: every write and delete takes the full check first.
    if let Err(e) = store.export.lease.quick_check() {
        store.compact.seen.clear();
        return Some(e.message);
    }
    None
}

/// Whether phase 1 should plan `branch` now.
fn branch_due(store: &Store, branch: &str, now: Instant, force: bool) -> bool {
    let pending = store.compact.seen.values().any(|s| s.blocks(branch));
    let due = force
        || (unpacked(store, branch) > TRIGGER
            && store
                .compact
                .retry_plan
                .get(branch)
                .is_none_or(|t| now >= *t));
    !pending && due
}

/// The cheap part of a run (`Store::tick` under `TickOptions { compaction: false }`): observes
/// the manifests (their grace periods start here as in a run) and tells whether a run would
/// do anything: a manifest past its grace, or a branch to plan. Writes nothing to the folder.
pub(crate) fn due(store: &mut Store) -> Result<bool, StoreError> {
    if cannot_run(store).is_some() {
        return Ok(false);
    }
    let now = store.ctl.now();
    let dir = store.project_dir.join(".finisher").join("compaction");
    let names = match manifest::list(&dir) {
        Ok(n) => n,
        Err(e) => {
            store.compact.last_error = Some(format!("cannot list {}: {e}", dir.display()));
            return Ok(false);
        }
    };
    state::observe(store, &dir, &names, now, crate::commit::now_ms())?;
    if !ready(store, now).is_empty() {
        return Ok(true);
    }
    for branch in db::branches(&store.conn)? {
        if branch_due(store, &branch, now, false) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The manifests whose grace period is over (and whose retry time has come).
fn ready(store: &Store, now: Instant) -> Vec<(ManifestName, Manifest)> {
    store
        .compact
        .seen
        .iter()
        .filter_map(|(n, s)| {
            let m = s.manifest.as_ref()?;
            let ready = matches!(s.state, State::Grace | State::Waiting(_))
                && s.observed.is_some_and(|t| {
                    now.saturating_duration_since(t) >= state::grace_of(store, Some(m))
                })
                && s.retry_at.is_none_or(|t| now >= t);
            ready.then(|| (n.clone(), m.clone()))
        })
        .collect()
}

/// Phase 2 of every manifest past its grace period. `true`: stop (the lease is lost).
fn phase2(store: &mut Store, report: &mut CompactReport, now: Instant) -> Result<bool, StoreError> {
    let due = ready(store, now);
    for (name, m) in due {
        let label = name.to_string();
        let path = store
            .project_dir
            .join(".finisher")
            .join("compaction")
            .join(&label);
        let step = match manifest::read(&path) {
            manifest::Read::Ok(again) if again == m => finish::run(store, &name, &m)?,
            manifest::Read::Ok(_) => Step::Abandon("the manifest changed".into()),
            manifest::Read::Gone => {
                // Finished (or removed) by someone else meanwhile.
                store.compact.seen.remove(&name);
                continue;
            }
            manifest::Read::Unreadable(why) => Step::Wait(why),
        };
        match step {
            Step::Done(n) => {
                store.compact.seen.remove(&name);
                report.finished.push(label);
                report.deleted += n;
            }
            Step::Wait(why) => {
                if let Some(s) = store.compact.seen.get_mut(&name) {
                    s.state = State::Waiting(why.clone());
                    s.retry_at = Some(now + RETRY);
                }
                report.waiting.push((label, why));
            }
            Step::Abandon(why) => {
                let mut all = state::abandoned(store)?;
                all.insert(label.clone(), why.clone());
                state::set_abandoned(store, &all)?;
                if let Some(s) = store.compact.seen.get_mut(&name) {
                    s.state = State::Abandoned(why.clone());
                }
                report.abandoned.push((label, why));
            }
            Step::Stopped(why) => {
                report.skipped = Some(why);
                store.compact.seen.clear();
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Phase 1 of `branch`. `true`: stop (the lease is lost).
fn phase1(
    store: &mut Store,
    report: &mut CompactReport,
    branch: &str,
    now: Instant,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let Fence::Held(lease_epoch) = fence(store, 0) else {
        return Ok(true);
    };
    let wait = |store: &mut Store, report: &mut CompactReport, why: String| {
        store
            .compact
            .retry_plan
            .insert(branch.to_string(), now + RETRY);
        report.waiting.push((branch.to_string(), why));
    };
    let v = match view::read(store, branch)? {
        Ok(v) => v,
        Err(why) => {
            wait(store, report, why);
            return Ok(false);
        }
    };
    // What an abandoned compaction of this branch holds is left alone.
    let (mut protected, mut protected_names) = (HashSet::new(), HashSet::new());
    for s in store.compact.seen.values() {
        if let Some(m) = s.manifest.as_ref().filter(|m| m.branch == branch) {
            protected_names.extend(m.replaced.iter().cloned());
            for p in &m.packs {
                if let Some(seg) = v.by_name.get(p) {
                    protected.extend(seg.header().replaces.iter().map(|r| r.hash.clone()));
                }
            }
        }
    }
    let Some(plan) = pack::plan(store, branch, &v, &protected, &protected_names, lease_epoch)?
    else {
        store
            .compact
            .retry_plan
            .insert(branch.to_string(), now + REPLAN);
        return Ok(false);
    };
    let packs = u32::try_from(plan.packs.len()).unwrap_or(u32::MAX);
    let replaced: usize = plan.packs.iter().map(|p| p.replaced.len()).sum();
    match pack::write(store, branch, plan, now_ms)? {
        Step::Done((name, m)) => {
            report.manifests.push(name.to_string());
            report.packs_written += packs;
            report.replaced += u32::try_from(replaced).unwrap_or(u32::MAX);
            let g = state::grace_of(store, Some(&m));
            let ends = now_ms.saturating_add(i64::try_from(g.as_millis()).unwrap_or(i64::MAX));
            store.compact.seen.insert(name, Seen::written(m, now, ends));
            Ok(false)
        }
        Step::Wait(why) | Step::Abandon(why) => {
            wait(store, report, why);
            Ok(false)
        }
        Step::Stopped(why) => {
            report.skipped = Some(why);
            Ok(true)
        }
    }
}
