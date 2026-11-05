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
//!   the records, their writers and epochs; shown while its `SUPERSEDED` records are still set
//!   aside (the losing segments are in the folder). `SPLIT_BRAIN_SUSPECTED` instead when the
//!   lost records span more than one segment or more than 30 s of commits: not one late
//!   rename, but two servers writing at once;
//! - `FOREIGN_FILES`: files under `.finisher/` that are not ours;
//! - `INCOMPLETE_FILES`: a file seen incomplete for more than [`INCOMPLETE_POLLS`] polls on a
//!   local or network root, where every write is atomic, so it cannot be in transit: a crashed
//!   foreign writer or damage;
//! - `SERVER_ID_RENEWED`: the server data was found to be a copy (another directory or
//!   machine than its id was made in), so this server took a new id at open;
//! - `LEASE_CHECK`: the last fencing check could not list `.finisher/` (the lease stands);
//! - `SYNC_ARRIVING`: on a sync folder, segment files are placeholders or incomplete (they
//!   also hold `CATCHING_UP` after an acquisition);
//! - `PROJECT_FILE_CONFLICT`: a sync conflict copy of `finisher.project.json` sits in the
//!   project folder (warning only: the copy is never read).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use fin_core::{CommittedTx, CoreError};
use rusqlite::params;
use serde::Serialize;

use crate::import::{ImportReport, ReadOnlyReason};
use crate::lease::{LeaseState, SystemClock};
use crate::scan::NotArrivedWhy;
use crate::storage_kind::StorageKind;
use crate::tick::Gate;
use crate::{Store, StoreError, db};

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
    pub branch: String,
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
    pub warnings: Vec<Warning>,
}

/// What the import passes found, kept for status.
#[derive(Debug, Default)]
pub(crate) struct Follow {
    foreign: Vec<String>,
    waiting: Vec<String>,
    pending: Vec<String>,
    import_error: Option<CoreError>,
    /// Incomplete files and the consecutive polls each was seen so.
    incomplete: HashMap<PathBuf, u32>,
    stuck: Vec<String>,
    /// Placeholders and incomplete files under `segments/`.
    arriving_files: Vec<String>,
    /// Conflict copies of `finisher.project.json` in the project folder.
    project_file_copies: Vec<String>,
    pub(crate) last_export_ms: Option<i64>,
    last_status: Option<ProjectStatus>,
    /// `Rewound` events not yet returned by `tick`.
    pub(crate) rewound: Vec<StoreEvent>,
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
        let segments = project_dir.join(".finisher").join("segments");
        self.arriving_files = r
            .not_arrived
            .iter()
            .filter(|(p, why)| {
                matches!(why, NotArrivedWhy::Placeholder | NotArrivedWhy::Incomplete)
                    && p.starts_with(&segments)
            })
            .map(|(p, _)| p.to_string_lossy().into_owned())
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
        let mut warnings = vec![];
        let mut warn = |code: &str, message: String| {
            warnings.push(Warning {
                code: code.to_string(),
                message,
            });
        };
        if let Some(w) = &self.storage_warning {
            warn("STORAGE_OVERRIDE", w.clone());
        }
        if !SystemClock::counts_sleep() {
            warn("CLOCK_IGNORES_SLEEP", "this machine's monotonic clock does not count time asleep: after a sleep the lease relies on its record check and claims alone".into());
        }
        if quarantine.count > 0 {
            let mut m = format!(
                "{} record(s) set aside ({})",
                quarantine.count,
                quarantine
                    .reasons
                    .iter()
                    .map(|(r, n)| format!("{r}: {n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if quarantine.reasons.contains_key("DIVERGED_TAIL") {
                m.push_str("; DIVERGED_TAIL records are this server's own edits that lost to the project folder: they are kept and stay listed until recovered");
            }
            warn("QUARANTINE", m);
        }
        for (code, m) in rewind_warnings(&self.conn, &self.server_id).unwrap_or_default() {
            warn(code, m);
        }
        if !f.foreign.is_empty() {
            warn(
                "FOREIGN_FILES",
                format!(
                    "{} file(s) under .finisher are not Finisher's",
                    f.foreign.len()
                ),
            );
        }
        if !f.stuck.is_empty() {
            warn(
                "INCOMPLETE_FILES",
                format!(
                    "incomplete for more than {INCOMPLETE_POLLS} polls on a {} folder, where writes are atomic: {}",
                    self.storage_kind(),
                    f.stuck.join(", ")
                ),
            );
        }
        if let Some(why) = &self.identity_note {
            warn(
                "SERVER_ID_RENEWED",
                format!("this server has a new server id: {why}"),
            );
        }
        if let Some(e) = &self.ctl.check_error {
            warn("LEASE_CHECK", e.clone());
        }
        if self.storage_kind().is_sync() && f.arriving() {
            warn(
                "SYNC_ARRIVING",
                format!(
                    "{} segment file(s) are still arriving from the sync client; after taking the lease the project stays CATCHING_UP until they are in (on a sync folder the 10 s wait does not bound how late another machine's edits arrive)",
                    f.arriving_files.len()
                ),
            );
        }
        if !f.project_file_copies.is_empty() {
            warn(
                "PROJECT_FILE_CONFLICT",
                format!(
                    "a sync client made a conflict copy of finisher.project.json: {} (this file is used; compare and delete the copy)",
                    f.project_file_copies.join(", ")
                ),
            );
        }
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
            warnings,
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

    fn branch_status(&self) -> Result<Vec<BranchStatus>, StoreError> {
        let mut out = vec![];
        for branch in db::branches(&self.conn)? {
            let through: Option<i64> = self.conn.query_row(
                "SELECT MAX(seq) FROM txs WHERE branch = ?1 AND exported = 1",
                params![branch],
                |r| r.get(0),
            )?;
            out.push(BranchStatus {
                head: self.head(&branch),
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

fn quarantine_summary(conn: &rusqlite::Connection) -> Result<QuarantineSummary, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT origin, reason, COUNT(*) FROM quarantine WHERE current = 1
         GROUP BY origin, reason ORDER BY origin, reason",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut s = QuarantineSummary::default();
    for row in rows {
        let (origin, reason, n) = row?;
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        s.count = s.count.saturating_add(n);
        if !s.origins.contains(&origin) {
            s.origins.push(origin);
        }
        *s.reasons.entry(reason).or_default() += n;
    }
    Ok(s)
}

/// `REWOUND` / `SPLIT_BRAIN_SUSPECTED` per branch (module docs).
fn rewind_warnings(
    conn: &rusqlite::Connection,
    server_id: &str,
) -> Result<Vec<(&'static str, String)>, StoreError> {
    let mut out = vec![];
    for branch in db::branches(conn)? {
        let Some(r) = crate::history::rewinds(conn, &branch)?
            .into_iter()
            .rev()
            .find(|r| r.kind == "appliedLine")
        else {
            continue;
        };
        let aside: i64 = conn.query_row(
            "SELECT COUNT(*) FROM quarantine
             WHERE branch = ?1 AND reason = 'SUPERSEDED' AND current = 1",
            params![branch],
            |row| row.get(0),
        )?;
        if aside == 0 {
            continue;
        }
        let list = |v: Vec<String>| v.join(", ");
        let mut m = format!(
            "branch {branch:?} was rewound from rev {}: {} record(s) written by {} at epoch {} lost to a line at epoch {} and are set aside (SUPERSEDED)",
            r.from_rev,
            r.count,
            list(r.writers.clone()),
            list(r.epochs.iter().map(u64::to_string).collect()),
            r.winner_epoch,
        );
        if r.own > 0 {
            m.push_str(&format!(
                "; {} of them are this server's own edits ({server_id}), which other servers never applied",
                r.own
            ));
        }
        let code = if r.split_brain() {
            m.push_str(&format!(
                "; the lost records span {} segment(s) and {} s of commits: two servers may have written at once",
                r.segments,
                r.span_ms / 1000
            ));
            "SPLIT_BRAIN_SUSPECTED"
        } else {
            "REWOUND"
        };
        out.push((code, m));
    }
    Ok(out)
}
