//! What `fin project status` shows (spec §3.7, §3.10) and the events `tick` emits.
//!
//! [`Store::read_only`] combines the import's verdict with the lease (precedence in
//! [`ReadOnlyReason`]). [`Store::status`] adds the branches, export and import state,
//! quarantine and warnings:
//! - `STORAGE_OVERRIDE`: the `storageOverride` was ignored or weakens a sync folder;
//! - `CLOCK_IGNORES_SLEEP`: this machine's monotonic clock does not count time asleep, so a
//!   machine that slept wakes with its self-fence deadline not yet passed (the record compare
//!   and claims still fence it);
//! - `QUARANTINE`: records set aside, by reason. `DIVERGED_TAIL` rows are this server's own
//!   edits that lost to the folder; their bytes exist nowhere else, so they stay counted
//!   until recovered (4B: a `recovered-*` branch);
//! - `REWOUND`: an applied line lost to a newer lease epoch and was rewound (`import::rewind`):
//!   the records, their writers and epochs; one per rewind, for 24 h or until
//!   [`Store::acknowledge_rewind`]. `SPLIT_BRAIN_SUSPECTED` instead when the lost records
//!   span more than one segment or more than 30 s of commits (not one late rename, but two
//!   servers writing at once): shown until acknowledged. A descendant branch taken out
//!   whole with it (a cascade, plan 4B Decision 7) is reported too, as taken out, and as
//!   an earlier branch once another branch holds its id;
//! - `BRANCH_RECORDS_IGNORED`: `branch` or `branchSet` records in the log that took no effect
//!   (an id collision lost to another server's record, or a record the branch index
//!   refuses; plan 4B Decision 33);
//! - `FOREIGN_FILES`: files under `.finisher/` that are not ours;
//! - `INCOMPLETE_FILES`: a file seen incomplete for more than [`INCOMPLETE_POLLS`] polls on a
//!   local or network root, where every write is atomic, so it cannot be in transit: a crashed
//!   foreign writer or damage;
//! - `LOCKED_FILES`: a file under `.finisher/` that could not be read (held open by another
//!   program, pending deletion while another client holds it, or no permission: a transient
//!   error) and has looked the same for 60 s of this store's clock (`verify::locked_too_long`),
//!   on any kind of folder. A segment that compaction deleted while another client briefly
//!   held it stays quiet;
//! - `SERVER_ID_RENEWED`: the server data was found to be a copy (another directory or
//!   machine than its id was made in), so this server took a new id at open;
//! - `LEASE_CHECK`: the last fencing check could not list `.finisher/` (the lease stands);
//! - `SYNC_ARRIVING`: on a sync folder, segment files are placeholders, incomplete or locked
//!   (they also hold `CATCHING_UP` after an acquisition);
//! - `COMPACTION_ABANDONED`: a compaction manifest whose packs or segments do not check out
//!   against the folder: nothing it names is deleted;
//! - `PROJECT_FILE_CONFLICT`: a sync conflict copy of `finisher.project.json` sits in the
//!   project folder (warning only: the copy is never read).

mod warnings;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use fin_core::{CommittedTx, CoreError};
use rusqlite::params;
use serde::Serialize;

use crate::import::{ImportReport, ReadOnlyReason};
use crate::lease::LeaseState;
use crate::scan::NotArrivedWhy;
use crate::storage_kind::StorageKind;
use crate::tick::Gate;
use crate::{Store, StoreError, db};
use warnings::quarantine_summary;

/// Polls a file may stay incomplete on a non-sync root before it is a warning.
pub const INCOMPLETE_POLLS: u32 = 5;

/// What [`Store::tick`] reports.
#[derive(Debug, Clone, PartialEq)]
pub enum StoreEvent {
    /// A record imported from the folder (its branch is in the tx).
    Applied(CommittedTx),
    /// The status changed.
    Status(Box<ProjectStatus>),
    /// An import took records out of `branch`'s history: an applied line that lost to a
    /// newer lease epoch, or this server's diverged tail. Revs from `from_rev` on no longer
    /// hold what was reported before; `quarantined` records were set aside (`own`: some were
    /// this server's), and the branch is at `generation` now. Reported before the records
    /// that replace them (`Applied`).
    Rewound {
        branch: String,
        from_rev: u64,
        quarantined: u32,
        own: bool,
        generation: u64,
    },
    /// The database was rebuilt (at open or by [`Store::rebuild`]): it is a new
    /// `incarnation`, and every rev read before may now hold something else. Reported before
    /// the records imported after it.
    Rebuilt { incarnation: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOnly {
    pub reason: ReadOnlyReason,
    pub fix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchStatus {
    /// The branch id.
    pub branch: String,
    /// Its display name (`main` for `main`; a branch without an index row: its id).
    pub name: String,
    /// The parent's id (`None` for `main`).
    pub parent: Option<String>,
    /// The parent's seq it forks at (0 for `main`).
    pub fork_seq: u64,
    pub state: fin_core::branch::BranchState,
    pub head: u64,
    /// The highest seq in the folder (0: none).
    pub exported_through: u64,
    /// Bumped whenever an import takes records out of this branch's history (a rewind).
    pub generation: u64,
}

/// Records set aside now (`current` quarantine rows).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineSummary {
    pub count: u32,
    /// `folder`, `local`, `foreign`.
    pub origins: Vec<String>,
    /// Count per reason (`STALE_EPOCH`, `MALFORMED`, `DIVERGED_TAIL`, `SUPERSEDED`,
    /// `CONFLICT_COPY`).
    pub reasons: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    pub code: String,
    pub message: String,
}

/// `fin project status` (serialised camelCase).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    pub project_id: String,
    pub storage: StorageKind,
    pub storage_warning: Option<String>,
    pub lease: LeaseState,
    /// The last fencing check failed only because `.finisher/` could not be listed.
    pub lease_check: Option<String>,
    pub read_only: Option<ReadOnly>,
    pub branches: Vec<BranchStatus>,
    /// Unix ms of the last export that wrote a segment (this process).
    pub last_export_ms: Option<i64>,
    pub export_error: Option<CoreError>,
    /// The last import pass's error (a record that cannot be applied, an unlistable folder).
    pub import_error: Option<CoreError>,
    pub quarantine: QuarantineSummary,
    /// Foreign files, relative to the project folder.
    pub foreign: Vec<String>,
    /// Missing predecessor segments (gaps): imported once they arrive.
    pub waiting: Vec<String>,
    /// Segments neither imported nor set aside yet (beyond a fork, behind a gap).
    pub pending: Vec<String>,
    /// Folder branch directories waiting for their `branch` record (never read-only).
    pub unknown_branches: Vec<String>,
    pub warnings: Vec<Warning>,
    /// The database's incarnation: with a branch's `generation`, what a caller passes to
    /// commit against what it read (`Generation`). New whenever the database is rebuilt.
    pub incarnation: String,
    /// Pending compaction manifests, the last run, its last problem.
    pub compaction: crate::CompactionStatus,
}

/// What the import passes found, kept for status.
#[derive(Debug, Default)]
pub(crate) struct Follow {
    foreign: Vec<String>,
    waiting: Vec<String>,
    pending: Vec<String>,
    unknown_branches: Vec<String>,
    import_error: Option<CoreError>,
    /// Incomplete files and the consecutive polls each was seen so.
    incomplete: HashMap<PathBuf, u32>,
    stuck: Vec<String>,
    /// Files that could not be read (`NotArrivedWhy::Locked`) in the last pass.
    locked: Vec<PathBuf>,
    /// Placeholders and incomplete or locked files under `segments/`.
    arriving_files: Vec<String>,
    /// Conflict copies of `finisher.project.json` in the project folder.
    project_file_copies: Vec<String>,
    pub(crate) last_export_ms: Option<i64>,
    last_status: Option<ProjectStatus>,
    /// Events of import passes (and rebuilds) not yet returned by `tick`: every pass queues
    /// its `Rewound` then its `Applied` events, also the passes run outside `tick` (open,
    /// takeover, rebuild).
    pub(crate) queued: Vec<StoreEvent>,
}

impl Follow {
    pub(crate) fn arriving(&self) -> bool {
        !self.arriving_files.is_empty()
    }

    pub(crate) fn observe(&mut self, r: &ImportReport, kind: StorageKind, project_dir: &Path) {
        self.import_error = r.error.clone();
        self.project_file_copies = std::fs::read_dir(project_dir)
            .map(|rd| {
                let mut v: Vec<String> = rd
                    .filter_map(|e| e.ok()?.file_name().into_string().ok())
                    .filter(|n| crate::conflict_names::classify_project_file(n).is_some())
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        if r.error.as_ref().is_some_and(|e| e.code == "IO_ERROR") {
            return; // the folder could not be listed: what it held before still stands
        }
        self.foreign = r
            .foreign
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect();
        self.waiting = r.waiting.clone();
        self.pending = r.pending.clone();
        self.unknown_branches = r.unknown_branches.clone();
        let segments = project_dir.join(".finisher").join("segments");
        self.arriving_files = r
            .not_arrived
            .iter()
            .filter(|(p, why)| {
                matches!(
                    why,
                    NotArrivedWhy::Placeholder | NotArrivedWhy::Incomplete | NotArrivedWhy::Locked
                ) && p.starts_with(&segments)
            })
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .collect();
        self.locked = r
            .not_arrived
            .iter()
            .filter(|(_, why)| *why == NotArrivedWhy::Locked)
            .map(|(p, _)| p.clone())
            .collect();
        let mut seen = HashMap::new();
        for (path, why) in &r.not_arrived {
            if *why == NotArrivedWhy::Incomplete {
                let n = self.incomplete.get(path).copied().unwrap_or(0) + 1;
                seen.insert(path.clone(), n);
            }
        }
        self.incomplete = seen;
        self.stuck = if kind.is_sync() {
            vec![]
        } else {
            let mut v: Vec<String> = self
                .incomplete
                .iter()
                .filter(|(_, n)| **n > INCOMPLETE_POLLS)
                .map(|(p, _)| p.to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
    }
}

impl Store {
    /// Why the project is read-only, if it is (precedence in [`ReadOnlyReason`]).
    pub fn read_only(&self) -> Option<ReadOnlyReason> {
        if let Some(r) = self.import.read_only() {
            // `ChainFork` and `ConflictCopy` outrank a newer project file.
            if matches!(r, ReadOnlyReason::ChainFork | ReadOnlyReason::ConflictCopy)
                || !self.file_newer
            {
                return Some(r);
            }
        }
        if self.file_newer {
            return Some(ReadOnlyReason::NewerFormat);
        }
        if self.holding() {
            return match self.ctl.gate {
                Gate::Open => None,
                Gate::Pending { .. } => Some(ReadOnlyReason::CatchingUp),
            };
        }
        match (self.lease(), &self.ctl.seen) {
            (None, LeaseState::HeldElsewhere { .. } | LeaseState::Stale { .. }) => {
                Some(ReadOnlyReason::LeaseHeld)
            }
            _ => Some(ReadOnlyReason::LeaseLost),
        }
    }

    /// The project's storage status (module docs).
    pub fn status(&self) -> ProjectStatus {
        let read_only = self.read_only().map(|reason| ReadOnly {
            reason,
            fix: reason.fix().to_string(),
        });
        let f = &self.follow;
        let quarantine = quarantine_summary(&self.conn).unwrap_or_default();
        let compaction = self.compaction_status();
        let warnings = self.status_warnings(&quarantine, &compaction);
        let rel = |p: &String| {
            std::path::Path::new(p)
                .strip_prefix(&self.project_dir)
                .map_or_else(|_| p.clone(), |r| r.to_string_lossy().into_owned())
        };
        ProjectStatus {
            project_id: self.project_id.clone(),
            storage: self.storage_kind(),
            storage_warning: self.storage_warning.clone(),
            lease: self.lease_state(),
            lease_check: self.ctl.check_error.clone(),
            read_only,
            branches: self.branch_status().unwrap_or_default(),
            last_export_ms: f.last_export_ms,
            export_error: self.export.error.clone(),
            import_error: f.import_error.clone(),
            quarantine,
            foreign: f.foreign.iter().map(rel).collect(),
            waiting: f.waiting.clone(),
            pending: f.pending.clone(),
            unknown_branches: f.unknown_branches.clone(),
            warnings,
            incarnation: crate::history::incarnation(&self.conn).unwrap_or_default(),
            compaction,
        }
    }

    /// The status, if it differs from the last one returned here (lease ages ignored).
    pub(crate) fn status_if_changed(&mut self) -> Option<ProjectStatus> {
        let now = self.status();
        let key = stable(&now);
        if self.follow.last_status.as_ref() == Some(&key) {
            return None;
        }
        self.follow.last_status = Some(key);
        Some(now)
    }

    /// Stops status reporting the rewind of `branch` that started `generation` (the
    /// `REWOUND` / `SPLIT_BRAIN_SUSPECTED` warning; the records stay set aside). `false` when
    /// no such rewind is recorded.
    pub fn acknowledge_rewind(
        &mut self,
        branch: &str,
        generation: u64,
    ) -> Result<bool, StoreError> {
        crate::history::acknowledge(&self.conn, branch, generation)
    }

    fn branch_status(&self) -> Result<Vec<BranchStatus>, StoreError> {
        let mut out = vec![];
        let ix = crate::branch::index::load(&self.conn)?;
        // `main` first, then by numeric id (the index's order), then branches without an
        // index row, by name.
        let mut all = db::branches(&self.conn)?;
        let mut ordered: Vec<String> = ix
            .list()
            .into_iter()
            .map(|(_, i)| i.id.clone())
            .filter(|id| all.contains(id))
            .collect();
        all.retain(|b| !ordered.contains(b));
        ordered.append(&mut all);
        for branch in ordered {
            let info = ix.get(&branch);
            let through: Option<i64> = self.conn.query_row(
                "SELECT MAX(seq) FROM txs WHERE branch = ?1 AND exported = 1",
                params![branch],
                |r| r.get(0),
            )?;
            out.push(BranchStatus {
                name: info.map_or_else(|| branch.clone(), |i| i.name.clone()),
                parent: info.and_then(|i| i.parent.clone()),
                fork_seq: info.map_or(0, |i| i.fork_seq),
                state: info.map_or(fin_core::branch::BranchState::Active, |i| i.state()),
                head: self.head_of(&branch),
                generation: crate::history::current(&self.conn, &branch)?,
                exported_through: through.map_or(Ok(0), db::to_u64)?,
                branch,
            });
        }
        Ok(out)
    }
}

/// `status` without what changes on every poll.
fn stable(s: &ProjectStatus) -> ProjectStatus {
    let mut s = s.clone();
    if let LeaseState::HeldElsewhere { age_server_ms, .. } = &mut s.lease {
        *age_server_ms = None;
    }
    s
}
