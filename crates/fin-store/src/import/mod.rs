//! Import (sub-project 4 spec §3.3, §3.6, §3.7): records in the project folder enter the DB.
//!
//! # One pass ([`Store::import`])
//!
//! 1. **Scan** `.finisher/` with the store's [`Scanner`] (verified files are cached by path).
//! 2. **Hydrate** cloud-sync placeholders under `segments/` by reading each whole file once,
//!    at most [`hydrate::BUDGET`] (16 MiB) per pass; the *next* pass's scan sees the
//!    hydrated file. A hydration read is not a decode and is not counted as one.
//! 3. **Classify:** verified segments are grouped per branch as [`SegRef`]s, cached by
//!    segment hash (segments are write-once, so a `SegRef`, which hashes every record, is
//!    built once). `Newer` and `UnsupportedHash` files set [`ImportReport::newer`]. Foreign
//!    files are listed; those that decode as segments are imported to quarantine (step 6).
//!    Snapshots are not used: seeding a fresh DB from one is rebuild's (Task 12), and an
//!    import always has a DB to continue.
//! 4. **One DB transaction** (`BEGIN IMMEDIATE`) for the whole pass, so a pass lands
//!    completely or not at all and nothing else commits between building [`Known`] and
//!    acting on the [`Resolution`]. Per branch the DB knows (folder branches it does not know
//!    are 4B's):
//!    - the DB line ([`known`]): `head` = the highest exported row, `tail` = the unexported
//!      rows, which must follow every exported row (else `DB_ERROR`);
//!    - **skip:** when the branch's segment set, `head`, `tail`, lease and storage kind are
//!      what the last resolution saw, `resolve` is pure, so its verdict is reused without
//!      running it again. The pass right after a change resolves once more against the new
//!      DB state;
//!    - otherwise `resolve` and act on it ([`apply`]): mark exported, set a diverged tail
//!      aside, apply, quarantine.
//! 5. **Id counters** are raised past every id in the log ([`Counters::raise_to_log`])
//!    whenever rows were added or removed, and on the store's first pass (which repairs a DB
//!    whose counters fell behind for any reason).
//! 6. **Quarantine** ([`quarantine`]): rows are keyed by `txHash`, so a record reported on
//!    every poll (a stale line) or by two sources (a stale line and a conflict copy of it) is
//!    one row. `current` says whether this pass's verdicts still set it aside; status counts
//!    only current rows ([`ImportReport::quarantined`]). Bytes are kept forever.
//!
//! # Verdicts are per poll
//!
//! Every verdict (quarantine, a diverged tail, a fork, waiting, newer files) is recomputed on
//! each pass and never persisted as final. The read-only state this module sets
//! ([`Store::read_only`]: `CHAIN_FORK`, `NEWER_FORMAT`, `IMPORT_ERROR`) is that of the last
//! pass (a pass that cannot list the folder changes nothing); commits and
//! export are refused while it holds. The full read-only state machine (lease states,
//! `CONFLICT_COPY`, status events) is Task 11, which also runs `import` from `open` and
//! `tick`.

mod apply;
mod hydrate;
mod known;
mod quarantine;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use fin_core::{CommittedTx, CoreError};
use rusqlite::TransactionBehavior;
use serde::Serialize;
use serde_json::json;

use crate::chain::{ForkInfo, SegRef, resolve};
use crate::conflict_names::Provider;
use crate::counters::Counters;
use crate::scan::{Entry, ForeignContent, Scanner};
use crate::storage_kind::StorageKind;
use crate::{Head, Store, StoreError, db, refresh};

/// What one import pass found and did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    /// Records applied, in order (each `CommittedTx`'s branch is in its `tx`).
    pub applied: Vec<CommittedTx>,
    /// Records currently set aside (quarantine rows that this pass's verdicts still report,
    /// plus this server's diverged tails, which nothing else holds).
    pub quarantined: u32,
    /// Missing predecessor segment hashes (gaps); retried on the next pass.
    pub waiting: Vec<String>,
    /// The first branch that cannot continue without a person choosing (`CHAIN_FORK`).
    pub fork: Option<ForkInfo>,
    /// Files under `.finisher/` that are not ours, conflict copies among them.
    pub foreign: Vec<ForeignFile>,
    /// A file (or a record) needs a newer Finisher (`NEWER_FORMAT`).
    pub newer: bool,
    /// Placeholders read this pass to hydrate them; the next pass imports them.
    pub hydrated: Vec<PathBuf>,
    /// A record on the resolved chain that cannot be applied (`REPLAY_FAILED`,
    /// `INVALID_FORMAT`, `SCHEMA_VERSION`): its branch stops before it; retried every pass.
    pub error: Option<CoreError>,
}

/// A foreign file (spec §3.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignFile {
    pub path: PathBuf,
    /// The conflict-copy pattern it matches; `Some(Unknown)` for a Finisher file under a name
    /// that is not its own.
    pub conflict_copy: Option<Provider>,
    /// Records it holds that the DB already has: harmless duplicates.
    pub known: u32,
    /// Records it holds that the DB does not have: set aside as `CONFLICT_COPY`.
    pub quarantined: u32,
}

/// Why the project is read-only (spec §3.7). Import sets `ChainFork`, `NewerFormat` and
/// `ImportError`; the others arrive with the lease and status work (Tasks 9 and 11).
///
/// `ImportError` (not in the spec's table; Task 20 adds it): a record on the resolved chain
/// cannot be applied ([`ImportReport::error`]). Writing past it would build a line that
/// servers able to replay it see as a fork, so commits and export wait until a later pass
/// no longer reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReadOnlyReason {
    LeaseHeld,
    LeaseLost,
    ChainFork,
    ConflictCopy,
    NewerFormat,
    ImportError,
}

impl ReadOnlyReason {
    pub fn code(self) -> &'static str {
        match self {
            ReadOnlyReason::LeaseHeld => "LEASE_HELD",
            ReadOnlyReason::LeaseLost => "LEASE_LOST",
            ReadOnlyReason::ChainFork => "CHAIN_FORK",
            ReadOnlyReason::ConflictCopy => "CONFLICT_COPY",
            ReadOnlyReason::NewerFormat => "NEWER_FORMAT",
            ReadOnlyReason::ImportError => "IMPORT_ERROR",
        }
    }

    fn fix(self) -> &'static str {
        match self {
            ReadOnlyReason::LeaseHeld => "wait for the other server, or take the project over",
            ReadOnlyReason::LeaseLost => "the lease is re-acquired automatically when possible",
            ReadOnlyReason::ChainFork | ReadOnlyReason::ConflictCopy => {
                "two histories exist in the project folder; keep one and move the other's segment files out of .finisher/segments (fin project status lists them)"
            }
            ReadOnlyReason::NewerFormat => "update Finisher to edit this project",
            ReadOnlyReason::ImportError => {
                "a record in the project folder cannot be applied (details in fin project status): update Finisher, or move the damaged segment out of .finisher/segments"
            }
        }
    }
}

/// What the skip check compares (step 4).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    segs: Vec<String>,
    line: known::Line,
    holds: bool,
    kind: StorageKind,
}

/// The per-poll verdict of one branch, reused while its [`Key`] is unchanged.
#[derive(Debug, Clone, Default)]
struct Verdict {
    fork: Option<ForkInfo>,
    waiting: Vec<String>,
    /// `txHash`es this verdict sets aside.
    quarantined: BTreeSet<String>,
    newer: bool,
    error: Option<CoreError>,
}

/// Import state held by the `Store`.
#[derive(Default)]
pub(crate) struct ImportState {
    scanner: Scanner,
    segrefs: HashMap<String, SegRef>,
    cache: HashMap<String, (Key, Verdict)>,
    /// The `current` set last written to `quarantine`; `None` before the first pass.
    flagged: Option<BTreeSet<String>>,
    read_only: Option<ReadOnlyReason>,
    /// The last placeholder read: the next pass starts after it (hydration takes turns).
    hydrate_from: Option<PathBuf>,
    /// The counters were raised past the log by this store at least once.
    ids_checked: bool,
    /// Storage kind for the epoch rule (detected or overridden at open).
    pub(crate) kind: StorageKind,
    resolves: u64,
}

impl ImportState {
    pub(crate) fn with_kind(kind: StorageKind) -> Self {
        ImportState {
            kind,
            ..Default::default()
        }
    }

    pub(crate) fn read_only(&self) -> Option<ReadOnlyReason> {
        self.read_only
    }
}

/// `PROJECT_READ_ONLY` for `reason`.
pub(crate) fn read_only_error(project_id: &str, reason: ReadOnlyReason) -> StoreError {
    CoreError::new(
        "PROJECT_READ_ONLY",
        format!("the project is read-only ({})", reason.code()),
    )
    .with_details(json!({ "reason": reason, "projectId": project_id }))
    .with_fix(reason.fix())
    .into()
}

/// `PROJECT_READ_ONLY` (`LEASE_LOST`) for a failed lease check `cause`.
pub(crate) fn lease_lost_error(project_id: &str, cause: &CoreError) -> StoreError {
    let reason = ReadOnlyReason::LeaseLost;
    CoreError::new(
        "PROJECT_READ_ONLY",
        format!(
            "the project is read-only ({}): {}",
            reason.code(),
            cause.message
        ),
    )
    .with_details(
        json!({ "reason": reason, "projectId": project_id, "cause": cause.details.get("cause") }),
    )
    .with_fix(reason.fix())
    .into()
}

impl Store {
    /// One import pass (module docs). `Err` only for a DB failure or a broken DB invariant;
    /// folder problems are in the report.
    pub fn import(&mut self) -> Result<ImportReport, StoreError> {
        let mut report = ImportReport::default();
        let entries = match self.import.scanner.scan(&self.project_dir) {
            Ok(e) => e,
            Err(e) => {
                report.error = Some(
                    CoreError::new(
                        "IO_ERROR",
                        format!("cannot list {}: {e}", self.project_dir.display()),
                    )
                    .with_details(json!({ "path": self.project_dir.to_string_lossy() })),
                );
                return Ok(report);
            }
        };
        report.hydrated = hydrate::run(&self.project_dir, &entries, &mut self.import.hydrate_from);

        // Classify.
        let mut by_branch: HashMap<String, Vec<SegRef>> = HashMap::new();
        let mut present = HashMap::new();
        let mut foreign = vec![];
        let mut newer = false;
        for entry in &entries {
            match entry {
                Entry::Segment { branch, seg, .. } => {
                    let r = self
                        .import
                        .segrefs
                        .remove(&seg.hash)
                        .unwrap_or_else(|| SegRef::new(seg.clone()));
                    by_branch.entry(branch.clone()).or_default().push(r.clone());
                    present.insert(seg.hash.clone(), r);
                }
                Entry::Newer { .. } | Entry::UnsupportedHash { .. } => newer = true,
                Entry::Foreign {
                    path,
                    conflict_copy,
                    content,
                } => {
                    if matches!(
                        content,
                        Some(ForeignContent::Newer { .. } | ForeignContent::UnsupportedHash)
                    ) {
                        newer = true;
                    }
                    let seg = match content {
                        Some(ForeignContent::Segment(s)) => Some(s.clone()),
                        _ => None,
                    };
                    foreign.push((path.clone(), *conflict_copy, seg));
                }
                _ => {}
            }
        }
        self.import.segrefs = present;
        for segs in by_branch.values_mut() {
            segs.sort_by(|a, b| a.hash().cmp(b.hash()));
        }

        let holds = self.export.lease.check().is_ok();
        let kind = self.import.kind;
        let Store {
            conn,
            heads,
            import,
            server_id,
            project_dir,
            ..
        } = self;
        let heads = heads.get_mut();
        let txn = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = crate::commit::now_ms();
        let mut ctx = apply::Ctx::new(&txn, server_id, now);
        let mut cache = HashMap::new();
        let mut new_heads: Vec<(String, Option<Head>)> = vec![];
        let mut resolves = 0;
        for branch in db::branches(&txn)? {
            let segs = by_branch.remove(&branch).unwrap_or_default();
            let line = known::line(&txn, &branch)?;
            let key = Key {
                segs: segs.iter().map(|s| s.hash().to_string()).collect(),
                line,
                holds,
                kind,
            };
            if let Some((k, v)) = import.cache.get(&branch)
                && *k == key
            {
                cache.insert(branch, (key, v.clone()));
                continue;
            }
            let known = known::build(&txn, &branch, &key.line, ctx.all_tx()?, holds, kind)?;
            let res = resolve(&segs, &known);
            resolves += 1;
            refresh(&txn, heads, &branch)?;
            let head = heads.get(&branch).cloned();
            let out = apply::branch(&mut ctx, &branch, &key.line, &segs, &res, head)?;
            report.applied.extend(out.applied);
            if out.changed {
                new_heads.push((branch.clone(), out.head));
            }
            cache.insert(branch, (key, out.verdict));
        }
        let foreign = quarantine::foreign(&mut ctx, project_dir, foreign)?;
        if !import.ids_checked {
            // First pass: every branch head, so ids without a binding count too.
            let mut all = HashMap::new();
            for b in db::branches(&txn)? {
                refresh(&txn, &mut all, &b)?;
            }
            let projects: Vec<_> = all.values().map(|h| &h.project).collect();
            Counters::raise_to_log(&txn, &projects)?;
        } else if ctx.changed {
            let projects: Vec<_> = new_heads
                .iter()
                .filter_map(|(_, h)| h.as_ref().map(|h| &h.project))
                .collect();
            Counters::raise_to_log(&txn, &projects)?;
        }
        let mut current: BTreeSet<String> = foreign.hashes;
        for (_, v) in cache.values() {
            current.extend(v.quarantined.iter().cloned());
        }
        if ctx.changed || import.flagged.as_ref() != Some(&current) {
            quarantine::flag_current(&txn, &current)?;
        }
        report.quarantined = quarantine::count(&txn)?;
        txn.commit()?;

        // Committed: adopt the new state.
        for (branch, head) in new_heads {
            match head {
                Some(h) => heads.insert(branch, h),
                None => heads.remove(&branch),
            };
        }
        import.flagged = Some(current);
        import.ids_checked = true;
        import.resolves += resolves;
        let mut branches: Vec<&String> = cache.keys().collect();
        branches.sort();
        let mut waiting = BTreeSet::new();
        for b in branches {
            let v = &cache[b].1;
            if report.fork.is_none() {
                report.fork = v.fork.clone();
            }
            waiting.extend(v.waiting.iter().cloned());
            newer |= v.newer;
            if report.error.is_none() {
                report.error = v.error.clone();
            }
        }
        import.cache = cache;
        report.waiting = waiting.into_iter().collect();
        report.foreign = foreign.files;
        report.newer = newer;
        import.read_only = if report.fork.is_some() {
            Some(ReadOnlyReason::ChainFork)
        } else if newer {
            Some(ReadOnlyReason::NewerFormat)
        } else if report.error.is_some() {
            Some(ReadOnlyReason::ImportError)
        } else {
            None
        };
        Ok(report)
    }

    /// Why the project is read-only after the last import pass, if it is (module docs).
    pub fn read_only(&self) -> Option<ReadOnlyReason> {
        self.import.read_only
    }

    /// Test hook: replaces the detected storage kind the epoch rule uses.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn set_test_storage_kind(&mut self, kind: StorageKind) {
        self.import.kind = kind;
    }

    /// Test hook: how many times this store has run `chain::resolve`.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn resolve_calls(&self) -> u64 {
        self.import.resolves
    }
}
