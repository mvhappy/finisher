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
//! (or `force`), and no compaction of it is pending:
//! 1. **Read afresh** ([`view::read`]): an uncached scan, a fresh resolution (no fork, nothing
//!    waiting or pending, every file under `segments/<branch>/` settled), and the database's
//!    exported rows exactly the chain's prefix. Otherwise wait (retried after [`RETRY`]).
//! 2. **Eligible**: non-pack segments wholly on the resolved chain and exported in the
//!    database, not set aside (a losing, stale, malformed or quarantined segment never is:
//!    it is what lets a late follower reach the same verdict, and 4B's `recovered-*` branches
//!    need it), not held by an abandoned compaction, and linked to a present predecessor.
//! 3. **Packs**: runs of consecutive eligible segments with non-decreasing epochs, cut at
//!    [`crate::segment::MAX_RECORDS`] / [`crate::segment::MAX_BYTES`] (whole segments, so a transaction is
//!    never split), at least two segments each. A pack holds the records byte for byte,
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
//! folder or the writer's is a sync folder) runs from the first poll that saw it while holding
//! the lease, never from its `createdMs` or a file time. Once it has passed: read afresh again,
//! check the packs, the snapshot and every replaced segment against the chain and the
//! database (records byte for byte), then delete the replaced files (a full lease check before
//! each), then the manifest. A file or check that is not settled waits ([`RETRY`]); anything
//! that contradicts the manifest abandons it: nothing more is deleted for it, it stays in the
//! folder and in status, and its segments are not compacted again (until `force`).
//!
//! Interrupted during phase 2, the next holder finishes it: absent replaced files are skipped.

mod finish;
mod manifest;
mod pack;
mod view;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::names::ManifestName;
use crate::storage_kind::StorageKind;
use crate::{Store, StoreError, db, fsio};
use manifest::Manifest;

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

/// What one compaction run did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompactReport {
    /// Why nothing ran (not holding the lease, read-only).
    pub skipped: Option<String>,
    /// Manifests written (phase 1).
    pub manifests: Vec<String>,
    pub packs_written: u32,
    /// Segments the new manifests replace.
    pub replaced: u32,
    /// Manifests finished (phase 2) and the segment files deleted.
    pub finished: Vec<String>,
    pub deleted: u32,
    /// `(manifest or branch, why)`: steps that wait and are retried.
    pub waiting: Vec<(String, String)>,
    /// `(manifest, why)`: abandoned this run.
    pub abandoned: Vec<(String, String)>,
}

/// One manifest as status shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCompaction {
    pub manifest: String,
    pub branch: Option<String>,
    pub writer_id: Option<String>,
    /// `grace`, `waiting`, `abandoned` or `unreadable`.
    pub state: String,
    pub reason: Option<String>,
    /// Unix ms (this machine's clock, informational) when the grace period ends.
    pub grace_ends_ms: i64,
}

/// Compaction in `fin project status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionStatus {
    /// Manifests seen while holding the lease.
    pub pending: Vec<PendingCompaction>,
    /// Unix ms of the last run that wrote or deleted something.
    pub last_run_ms: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Grace,
    Waiting(String),
    Abandoned(String),
    Unreadable(String),
}

struct Seen {
    manifest: Option<Manifest>,
    observed: Instant,
    grace_ends_ms: i64,
    state: State,
    retry_at: Option<Instant>,
}

/// Compaction state held by the `Store`.
#[derive(Default)]
pub(crate) struct CompactState {
    seen: BTreeMap<ManifestName, Seen>,
    /// Phase 1 of a branch that had to wait is retried after this.
    retry_plan: HashMap<String, Instant>,
    last_run_ms: Option<i64>,
    last_error: Option<String>,
    /// Test hook: a grace period replacing [`grace`].
    pub(crate) grace: Option<Duration>,
    #[cfg(any(test, feature = "test-hooks"))]
    fail_at: Option<CompactFail>,
    #[cfg(any(test, feature = "test-hooks"))]
    #[allow(clippy::type_complexity)]
    on_delete: Option<Box<dyn FnMut(&Path) + Send>>,
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
        .flat_map(|s| s.header().replaces.iter().map(String::as_str))
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
    if let Some(r) = store.read_only() {
        store.compact.seen.clear();
        report.skipped = Some(format!("the project is read-only ({})", r.code()));
        return Ok(report);
    }
    // In memory only: every write and delete below takes the full check first.
    if let Err(e) = store.export.lease.quick_check() {
        store.compact.seen.clear();
        report.skipped = Some(e.message);
        return Ok(report);
    }
    let now = store.ctl.now();
    if force {
        store.compact.retry_plan.clear();
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
    observe(store, &dir, &names, now, now_ms);
    if phase2(store, &mut report, now)? {
        return Ok(report);
    }
    for branch in db::branches(&store.conn)? {
        let pending = store.compact.seen.values().any(|s| {
            s.manifest.as_ref().is_some_and(|m| m.branch == branch)
                && !matches!(s.state, State::Abandoned(_))
        });
        let due = force
            || (unpacked(store, &branch) > TRIGGER
                && store
                    .compact
                    .retry_plan
                    .get(&branch)
                    .is_none_or(|t| now >= *t));
        if pending || !due {
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

/// Notes every manifest in `names` (first sighting starts its grace) and forgets the rest.
fn observe(store: &mut Store, dir: &Path, names: &[ManifestName], now: Instant, now_ms: i64) {
    let kind = store.storage_kind();
    let override_grace = store.compact.grace;
    let seen = &mut store.compact.seen;
    seen.retain(|n, _| names.contains(n));
    for name in names {
        let readable = seen.get(name).is_some_and(|s| s.manifest.is_some());
        if readable {
            continue;
        }
        let (manifest, state) = match manifest::read(&dir.join(name.to_string())) {
            manifest::Read::Ok(m) => (Some(m), State::Grace),
            manifest::Read::Gone => continue,
            manifest::Read::Unreadable(why) => (None, State::Unreadable(why)),
        };
        let g = override_grace.unwrap_or_else(|| {
            let theirs = manifest.as_ref().map_or(kind, Manifest::writer_kind);
            grace(kind).max(grace(theirs))
        });
        // Grace runs from the first sighting that could be read.
        seen.insert(
            name.clone(),
            Seen {
                manifest,
                observed: now,
                grace_ends_ms: now_ms + i64::try_from(g.as_millis()).unwrap_or(i64::MAX),
                state,
                retry_at: None,
            },
        );
    }
}

/// Phase 2 of every manifest past its grace period. `true`: stop (the lease is lost).
fn phase2(store: &mut Store, report: &mut CompactReport, now: Instant) -> Result<bool, StoreError> {
    let kind = store.storage_kind();
    let due: Vec<(ManifestName, Manifest)> = store
        .compact
        .seen
        .iter()
        .filter_map(|(n, s)| {
            let m = s.manifest.as_ref()?;
            let g = store
                .compact
                .grace
                .unwrap_or_else(|| grace(kind).max(grace(m.writer_kind())));
            let ready = matches!(s.state, State::Grace | State::Waiting(_))
                && now.saturating_duration_since(s.observed) >= g
                && s.retry_at.is_none_or(|t| now >= t);
            ready.then(|| (n.clone(), m.clone()))
        })
        .collect();
    for (name, m) in due {
        let label = name.to_string();
        let step = match m.invalid(&store.project_id) {
            Some(why) => Step::Abandon(why),
            None => match manifest::read(
                &store
                    .project_dir
                    .join(".finisher")
                    .join("compaction")
                    .join(&label),
            ) {
                manifest::Read::Ok(again) if again == m => finish::run(store, &name, &m)?,
                manifest::Read::Ok(_) => Step::Abandon("the manifest changed".into()),
                manifest::Read::Gone => {
                    // Finished (or removed) by someone else meanwhile.
                    store.compact.seen.remove(&name);
                    continue;
                }
                manifest::Read::Unreadable(why) => Step::Wait(why),
            },
        };
        let s = store.compact.seen.get_mut(&name);
        match step {
            Step::Done(n) => {
                store.compact.seen.remove(&name);
                report.finished.push(label);
                report.deleted += n;
            }
            Step::Wait(why) => {
                if let Some(s) = s {
                    s.state = State::Waiting(why.clone());
                    s.retry_at = Some(now + RETRY);
                }
                report.waiting.push((label, why));
            }
            Step::Abandon(why) => {
                if let Some(s) = s {
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
                    protected.extend(seg.header().replaces.iter().cloned());
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
            let g = store
                .compact
                .grace
                .unwrap_or_else(|| grace(store.storage_kind()));
            store.compact.seen.insert(
                name,
                Seen {
                    manifest: Some(m),
                    observed: now,
                    grace_ends_ms: now_ms + i64::try_from(g.as_millis()).unwrap_or(i64::MAX),
                    state: State::Grace,
                    retry_at: None,
                },
            );
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

impl CompactState {
    pub(crate) fn status(&self) -> CompactionStatus {
        let pending = self
            .seen
            .iter()
            .map(|(n, s)| {
                let (state, reason) = match &s.state {
                    State::Grace => ("grace", None),
                    State::Waiting(w) => ("waiting", Some(w.clone())),
                    State::Abandoned(w) => ("abandoned", Some(w.clone())),
                    State::Unreadable(w) => ("unreadable", Some(w.clone())),
                };
                PendingCompaction {
                    manifest: n.to_string(),
                    branch: s.manifest.as_ref().map(|m| m.branch.clone()),
                    writer_id: s.manifest.as_ref().map(|m| m.writer_id.clone()),
                    state: state.to_string(),
                    reason,
                    grace_ends_ms: s.grace_ends_ms,
                }
            })
            .collect();
        CompactionStatus {
            pending,
            last_run_ms: self.last_run_ms,
            last_error: self.last_error.clone(),
        }
    }
}

impl Store {
    /// Compacts now (`fin project compact`): phase 1 of every branch with at least two
    /// segments worth packing, whatever their number, and phase 2 of every manifest past its
    /// grace period (`force` never shortens it). Abandoned manifests are checked again. Only
    /// the lease holder compacts; otherwise the report says why nothing ran.
    pub fn compact(&mut self, force: bool) -> Result<CompactReport, StoreError> {
        run(self, crate::commit::now_ms(), force)
    }

    /// Compaction as status shows it.
    pub fn compaction_status(&self) -> CompactionStatus {
        self.compact.status()
    }

    /// Test hook: the next compaction stops at `point` with `TEST_FAULT`.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn fail_next_compaction_at(&mut self, point: CompactFail) {
        self.compact.fail_at = Some(point);
    }

    /// Test hook: the grace period between the phases, replacing the spec's.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn set_compaction_grace_for_test(&mut self, grace: Duration) {
        self.compact.grace = Some(grace);
    }

    /// Test hook: `f` runs after each replaced segment phase 2 deletes.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn on_compaction_delete_for_test(&mut self, f: Box<dyn FnMut(&Path) + Send>) {
        self.compact.on_delete = Some(f);
    }
}
