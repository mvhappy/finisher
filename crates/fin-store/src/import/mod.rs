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
//!    Snapshots are not used, except by a rebuild: it reads the folder's snapshots and
//!    passes them in as seeds ([`Seed`], [`apply`] step 3).
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
//!      aside, apply, quarantine;
//!    - an applied line that lost (`AppliedLineLost`) is first rewound when [`rewind`]'s
//!      conditions hold, and the branch resolved once more against the rewound DB (at most
//!      once per branch per pass). Whether it may depends on the lease files too: their
//!      highest epoch is read before `BEGIN` (never under the database lock) whenever the
//!      folder's segments or our lease changed or the last pass left such a fork, and it is
//!      part of the skip key.
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
//! (`CHAIN_FORK`, `CONFLICT_COPY`, `NEWER_FORMAT`, `IMPORT_ERROR`; [`Store::read_only`]
//! combines it with the lease's) is that of the last pass (a pass that cannot list the
//! folder, or finds `.finisher/` missing, changes nothing); commits and export are refused
//! while it holds. `Store::open` runs a pass before returning, and `Store::tick` one per
//! poll, so the state is re-established before any commit after a restart.
//!
//! A foreign file matching a conflict-copy pattern, or a Finisher file under a name that is
//! not its own (`conflict_copy` is `Some`), makes the project `CONFLICT_COPY` (spec §3.7);
//! other foreign files are listed only, and so is a conflict copy of a `lease.probe-*` file
//! (each server writes only its own probe). A conflicted *branch directory* is one foreign
//! entry (scan does not enter it) with `conflict_copy` set: read-only too.

mod apply;
mod hydrate;
mod known;
mod quarantine;
mod rewind;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use fin_core::{CommittedTx, CoreError};
use rusqlite::TransactionBehavior;
use serde::Serialize;
use serde_json::json;

use crate::chain::{ForkInfo, SegRef, resolve};
use crate::conflict_names::Provider;
use crate::counters::Counters;
use crate::scan::{Entry, ForeignContent, NotArrivedWhy, Scanner};
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
    /// Hashes of segments neither imported nor set aside yet (beyond a fork, behind a gap).
    pub pending: Vec<String>,
    /// Files under `.finisher/` that have not arrived yet (placeholders, incomplete, temp).
    pub not_arrived: Vec<(PathBuf, NotArrivedWhy)>,
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
    /// Records taken out of a branch's history this pass (a rewound applied line, a diverged
    /// tail), before `applied` was applied.
    pub rewound: Vec<Rewind>,
}

/// Records taken out of `branch`'s history by an import ([`crate::StoreEvent::Rewound`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewind {
    pub branch: String,
    /// The first rev that no longer holds what it held.
    pub from_rev: u64,
    /// Records set aside.
    pub quarantined: u32,
    /// Some of them were this server's own.
    pub own: bool,
    /// The branch's generation after the rewind.
    pub generation: u64,
}

/// A folder snapshot a rebuild offers to seed a fresh branch from (spec §3.4: it seeds only
/// if its `txHash` lies on the resolved chain).
#[derive(Debug, Clone)]
pub(crate) struct Seed {
    pub(crate) branch: String,
    pub(crate) seq: u64,
    pub(crate) tx_hash: String,
    pub(crate) project: fin_core::Project,
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

/// Why the project is read-only (spec §3.7). Import sets `ChainFork`, `ConflictCopy`,
/// `NewerFormat` and `ImportError`; the lease sets `LeaseHeld`, `LeaseLost` and
/// `CatchingUp` (`Store::read_only` combines them).
///
/// Not in the spec's table (Task 20 adds them):
/// - `ImportError`: a record on the resolved chain cannot be applied
///   ([`ImportReport::error`]). Writing past it would build a line that servers able to
///   replay it see as a fork, so commits and export wait until a later pass no longer
///   reports it.
/// - `CatchingUp`: the lease was just acquired, and no import pass at the new epoch has yet
///   resolved every branch with nothing waiting (after waiting out the directory-cache
///   window when another server wrote before). Until then a commit or an export could extend
///   a line the folder has already moved past (plan Task 11, ordering rule).
///
/// Precedence, highest first: `ChainFork`, `ConflictCopy`, `NewerFormat`, `ImportError`
/// (the folder's content: a person or an update must act, whoever holds the lease), then
/// `LeaseLost`, `LeaseHeld`, `CatchingUp` (the lease: resolved by waiting or a takeover).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReadOnlyReason {
    LeaseHeld,
    LeaseLost,
    ChainFork,
    ConflictCopy,
    NewerFormat,
    ImportError,
    CatchingUp,
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
            ReadOnlyReason::CatchingUp => "CATCHING_UP",
        }
    }

    pub fn fix(self) -> &'static str {
        match self {
            ReadOnlyReason::LeaseHeld => "wait for the other server, or take the project over",
            ReadOnlyReason::LeaseLost => "the lease is re-acquired automatically when possible",
            ReadOnlyReason::ChainFork => {
                "two histories exist in the project folder; keep one and move the other's segment files out of .finisher/segments (fin project status lists them)"
            }
            ReadOnlyReason::ConflictCopy => {
                "a sync client made a conflict copy of a Finisher file (fin project status lists it): another machine may have written at the same time; keep one history and move the copy out of .finisher"
            }
            ReadOnlyReason::CatchingUp => {
                "the lease was just taken: the project is writable once the folder has been read (about 10 s, or once missing segments arrive)"
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
    /// The epoch of the lease this server holds, if it holds one.
    held: Option<u64>,
    /// The highest epoch in `lease.json` and the claims ([`rewind::lease_top`]).
    lease_top: Option<u64>,
    kind: StorageKind,
}

/// The per-poll verdict of one branch, reused while its [`Key`] is unchanged.
#[derive(Debug, Clone, Default)]
struct Verdict {
    fork: Option<ForkInfo>,
    waiting: Vec<String>,
    pending: Vec<String>,
    /// `txHash`es this verdict sets aside.
    quarantined: BTreeSet<String>,
    newer: bool,
    error: Option<CoreError>,
}

/// Import state held by the `Store`.
#[derive(Default)]
pub(crate) struct ImportState {
    pub(crate) scanner: Scanner,
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
    /// What decided the last lease-file read: the segment hashes and the lease held then.
    lease_seen: Option<(BTreeSet<String>, Option<u64>)>,
    lease_top: Option<u64>,
    /// The last pass left an `AppliedLineLost` fork: read the lease files on every pass.
    forked: bool,
    /// Folder snapshots the next pass may seed from (a rebuild's fresh database only).
    pub(crate) seeds: Vec<Seed>,
    /// The highest seq a pass seeded a branch at.
    pub(crate) seeded: Option<u64>,
    /// Test hook: the next rewind fails after taking the rows out.
    #[cfg(any(test, feature = "test-hooks"))]
    fail_rewind: bool,
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

    /// The verified segments the last pass listed (every branch).
    pub(crate) fn segrefs(&self) -> impl Iterator<Item = &SegRef> {
        self.segrefs.values()
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
        let dot = self.project_dir.join(".finisher");
        let listed = if dot.is_dir() {
            self.import.scanner.scan(&self.project_dir)
        } else {
            // The folder vanished (unmounted, renamed): never read that as "empty".
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is missing", dot.display()),
            ))
        };
        let entries = match listed {
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

        // Classify. A segment written under another genesis (`genesisHash`) is another
        // project's history under this id (a hand-made copy, an edited project): it is not
        // imported, and the pass reports `GENESIS_MISMATCH` (read-only `IMPORT_ERROR`) until it
        // is moved out. It is not quarantined: its records belong to no history of ours.
        let genesis = crate::project_file::db_genesis_hash(&self.conn)?;
        let mut foreign_genesis: Vec<String> = vec![];
        let mut by_branch: HashMap<String, Vec<SegRef>> = HashMap::new();
        let mut present = HashMap::new();
        let mut foreign = vec![];
        let mut newer = false;
        for entry in &entries {
            match entry {
                Entry::Segment { branch, seg, name } if seg.header.genesis_hash != genesis => {
                    let path = dot.join("segments").join(branch).join(name.to_string());
                    foreign_genesis.push(path.to_string_lossy().into_owned());
                }
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
                Entry::NotArrived { path, why } => report.not_arrived.push((path.clone(), *why)),
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
        // Damaged and incomplete files, for verify's "unchanged for 60 s" rule.
        let damaged: Vec<_> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Damaged {
                    path,
                    len,
                    modified,
                    ..
                } => Some((path.clone(), *len, *modified)),
                Entry::NotArrived {
                    path,
                    why: NotArrivedWhy::Incomplete,
                } => {
                    let m = std::fs::metadata(path).ok();
                    Some((
                        path.clone(),
                        m.as_ref().map_or(0, |m| m.len()),
                        m.and_then(|m| m.modified().ok()),
                    ))
                }
                _ => None,
            })
            .collect();
        let now_mono = self.ctl.now();
        self.damage.get_mut().observe(&damaged, now_mono);
        for segs in by_branch.values_mut() {
            segs.sort_by(|a, b| a.hash().cmp(b.hash()));
        }

        let held = self.export.lease.check().ok();
        let holds = held.is_some();
        let kind = self.import.kind;
        // Lease files for the rewind rule, before the database lock (module docs).
        let seen = (self.import.segrefs.keys().cloned().collect(), held);
        if self.import.forked || self.import.lease_seen.as_ref() != Some(&seen) {
            self.import.lease_top = rewind::lease_top(&dot);
            self.import.lease_seen = Some(seen);
        }
        let lease_top = self.import.lease_top;
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
        let mut ctx = apply::Ctx::new(&txn, server_id, now, &import.seeds);
        let mut cache = HashMap::new();
        let mut new_heads: Vec<(String, Option<Head>)> = vec![];
        let mut resolves = 0;
        for branch in db::branches(&txn)? {
            let segs = by_branch.remove(&branch).unwrap_or_default();
            let line = known::line(&txn, &branch)?;
            let key = Key {
                segs: segs.iter().map(|s| s.hash().to_string()).collect(),
                line,
                held,
                lease_top,
                kind,
            };
            if let Some((k, v)) = import.cache.get(&branch)
                && *k == key
            {
                cache.insert(branch, (key, v.clone()));
                continue;
            }
            let mut key = key;
            let known = known::build(&txn, &branch, &key.line, ctx.all_tx()?, holds, kind)?;
            let mut res = resolve(&segs, &known);
            resolves += 1;
            let mut rewound = None;
            if let Some(lost) = rewind::eligible(&ctx, &branch, &res, held, lease_top, kind)? {
                rewound = Some(rewind::run(&mut ctx, &branch, lost, &segs)?);
                #[cfg(any(test, feature = "test-hooks"))]
                if std::mem::take(&mut import.fail_rewind) {
                    // Dropping `txn` rolls the rewind back, as a crash before the commit.
                    return Err(CoreError::new("TEST_FAULT", "injected failure in a rewind").into());
                }
                key.line = known::line(&txn, &branch)?;
                let known = known::build(&txn, &branch, &key.line, ctx.all_tx()?, holds, kind)?;
                res = resolve(&segs, &known);
                resolves += 1;
            }
            refresh(&txn, heads, &branch)?;
            let head = heads.get(&branch).cloned();
            let mut out = apply::branch(&mut ctx, &branch, &key.line, &segs, &res, head)?;
            report.rewound.extend(rewound.iter().cloned());
            report.rewound.extend(out.rewound.take());
            report.applied.extend(out.applied);
            if out.changed || rewound.is_some() {
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
        let seeded = ctx.seeded;
        drop(ctx);
        txn.commit()?;
        import.seeded = import.seeded.max(seeded);

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
        let mut pending = BTreeSet::new();
        for b in branches {
            let v = &cache[b].1;
            if report.fork.is_none() {
                report.fork = v.fork.clone();
            }
            waiting.extend(v.waiting.iter().cloned());
            pending.extend(v.pending.iter().cloned());
            newer |= v.newer;
            if report.error.is_none() {
                report.error = v.error.clone();
            }
        }
        import.forked = cache.values().any(|(_, v)| {
            v.fork
                .as_ref()
                .is_some_and(|f| f.reason == crate::chain::ForkReason::AppliedLineLost)
        });
        import.cache = cache;
        report.waiting = waiting.into_iter().collect();
        report.pending = pending.into_iter().collect();
        report.foreign = foreign.files;
        report.newer = newer;
        if !foreign_genesis.is_empty() && report.error.is_none() {
            report.error = Some(crate::project_file::genesis_mismatch(
                &genesis,
                foreign_genesis,
            ));
        }
        let conflict_copy = report.foreign.iter().any(|f| {
            f.conflict_copy.is_some()
                && !f
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(crate::conflict_names::is_probe_copy)
        });
        import.read_only = if report.fork.is_some() {
            Some(ReadOnlyReason::ChainFork)
        } else if conflict_copy {
            Some(ReadOnlyReason::ConflictCopy)
        } else if newer {
            Some(ReadOnlyReason::NewerFormat)
        } else if report.error.is_some() {
            Some(ReadOnlyReason::ImportError)
        } else {
            None
        };
        Ok(report)
    }

    /// Test hook: replaces the detected storage kind the epoch rule uses.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn set_test_storage_kind(&mut self, kind: StorageKind) {
        self.import.kind = kind;
    }

    /// Test hook: the next rewind fails inside its transaction (after taking the rows out).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn fail_next_rewind_for_test(&mut self) {
        self.import.fail_rewind = true;
    }

    /// Test hook: how many times this store has run `chain::resolve`.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn resolve_calls(&self) -> u64 {
        self.import.resolves
    }
}
