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
//!    acting on the [`Resolution`]. Per branch the DB knows, parents before children (a
//!    worklist, [`pass`]: a `branch` record applied on the way adds its child, so a child
//!    whose segments came with its parent's record is followed in the same pass; plan 4B
//!    Decision 15):
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
//!      once per branch per pass); a child whose `branch` record was among the lost rows (of
//!      a rewind or a diverged tail) is rewound whole with it, recursively, and the derived
//!      tables are recomputed ([`cascade`], plan 4B Decisions 6–7). Whether it may depends
//!      on the lease files too: their highest epoch is read before `BEGIN` (never under the
//!      database lock) whenever the folder's segments or our lease changed or the last pass
//!      left such a fork, and it is part of the skip key.
//!    - a branch's segments must start at its fork point (`Known::fork`, from the index);
//!    - a 4B branch is resolved from the segments that name its `branch` record
//!      (`branchRecord`); `branch` and `branchSet` records fold into the index as they are
//!      stored ([`branches`]: an id two servers made goes to the higher epoch, then the lower
//!      `txHash`; a losing or unfit record is an inert row);
//!    - every other branch segment ([`aside`]): one whose record is set aside or inert goes
//!      to quarantine with that record's reason (recursively); one whose record is unknown
//!      waits ([`ImportReport::unknown_branches`], never read-only).
//! 5. **Id counters** are raised past every id in the log ([`Counters::raise_to_log`])
//!    whenever rows were added or removed, and on the store's first pass (which repairs a DB
//!    whose counters fell behind for any reason); and on every pass past the ids of
//!    quarantine rows added since the last ([`Counters::raise_to_quarantine`], plan 4B
//!    Decision 28), and the recovered marks read the kind of those rows once (`recover::marks`).
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
mod aside;
mod branches;
mod cascade;
mod ctx;
mod hydrate;
mod known;
mod pass;
pub(crate) mod quarantine;
mod read;
mod records;
mod report;
mod rewind;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use fin_core::CoreError;
use rusqlite::TransactionBehavior;
use serde_json::json;

use crate::chain::{ForkInfo, SegRef};
use crate::counters::Counters;
use crate::scan::Scanner;
use crate::storage_kind::StorageKind;
use crate::{Store, StoreError, db, refresh};
pub(crate) use known::{fork_point, known_now};
use pass::PassOut;
pub(crate) use read::{ImportPlan, ImportRead};

pub(crate) use report::Seed;
pub use report::{ForeignFile, ImportReport, ReadOnlyReason, Rewind};

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
    /// Where the branch starts (the graph's start check).
    fork: Option<crate::chain::ForkPoint>,
    /// The `branch` record the index holds for the branch (its segments name it).
    record: Option<String>,
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
    /// The last pass found a file or record that needs a newer Finisher.
    newer: bool,
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

    /// Whether the last pass found a file or record that needs a newer Finisher (whatever
    /// outranks it in [`ImportState::read_only`]).
    pub(crate) fn newer(&self) -> bool {
        self.newer
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
        let read = self.import_plan()?.read();
        // Nothing can change the store between the two halves here.
        self.import_apply(read)?.ok_or_else(|| {
            CoreError::new("INTERNAL", "the import pass was overtaken by another").into()
        })
    }

    /// Step 4 on (the database transaction) for what [`ImportPlan::read`] found, under the
    /// store lock. `None` when something changed what the pass read against since it was
    /// planned (another pass applied, an export marked rows, a rebuild, an acquisition,
    /// [`crate::tick::Phase::io_gen`]): nothing is applied, and the next poll reads afresh.
    pub(crate) fn import_apply(
        &mut self,
        read: ImportRead,
    ) -> Result<Option<ImportReport>, StoreError> {
        let ImportRead {
            scanner,
            hydrate_from,
            genesis,
            fence,
            io_gen,
            folder,
            ..
        } = read;
        // Its cache holds verified write-once files: still valid whatever happened meanwhile.
        self.import.scanner = scanner;
        self.import.hydrate_from = hydrate_from;
        if io_gen != self.phase.io_gen {
            return Ok(None);
        }
        let folder = match folder {
            Ok(f) => f,
            Err(e) => {
                return Ok(Some(ImportReport {
                    error: Some(e),
                    ..ImportReport::default()
                }));
            }
        };
        let kind = self.import.kind;
        let dot = self.project_dir.join(".finisher");
        let read::Folder {
            mut report,
            mut by_branch,
            present,
            foreign,
            mut newer,
            foreign_genesis,
            damaged,
            seen_at,
            held,
            mut lease_seen,
            mut lease_top,
        } = folder;
        self.import.segrefs = present;
        self.damage.get_mut().observe(&damaged, seen_at);
        // Held as the full check after the scan found, if the lease it checked is still the
        // one in force and has not been lost since (in memory; no folder I/O here).
        let held = held.filter(|e| {
            fence.is_current(&self.export.lease) && self.export.lease.quick_check() == Ok(*e)
        });
        if held != lease_seen.1 {
            // Lost since the read: the lease files once more (small, rare).
            lease_seen.1 = held;
            lease_top = rewind::lease_top(&dot);
        }
        let holds = held.is_some();
        self.import.lease_top = lease_top;
        self.import.lease_seen = Some(lease_seen);
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
        let mut ctx = ctx::Ctx::new(&txn, server_id, now, &import.seeds);
        let PassOut {
            cache,
            new_heads,
            resolves,
            applied,
            rewound,
            dirs,
        } = pass::resolve_branches(
            &txn,
            &mut ctx,
            &import.cache,
            #[cfg(any(test, feature = "test-hooks"))]
            &mut import.fail_rewind,
            heads,
            &mut by_branch,
            held,
            holds,
            lease_top,
            kind,
        )?;
        report.rewound.extend(rewound);
        // `merged` flags the applied records change (Decision 21), before the marks below
        // compare the index with the log's.
        let mut touched = crate::derived::Touched::default();
        for c in &applied {
            touched.imported(&c.tx.branch, c.link.as_deref());
        }
        touched.settle(&txn)?;
        report.applied.extend(applied);
        if ctx.folded {
            // Inert marks from the log alone, before the leftovers read them.
            crate::derived::remark(&txn)?;
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
        // Folder branch directories no known branch took (Decision 8).
        let (taken, unknown) = aside::leftovers(&mut ctx, &dirs, &current)?;
        current.extend(taken);
        report.unknown_branches = unknown;
        if ctx.changed || import.flagged.as_ref() != Some(&current) {
            quarantine::flag_current(&txn, &current)?;
        }
        Counters::raise_to_quarantine(&txn)?;
        crate::recover::fill_marks(&txn)?;
        report.quarantined = quarantine::count(&txn)?;
        let seeded = ctx.seeded;
        drop(ctx);
        txn.commit()?;
        self.phase.io_gen += 1;
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
        import.newer = newer;
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
        Ok(Some(report))
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
