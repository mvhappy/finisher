//! Finisher's local store: one SQLite op log per project at
//! `<server-data>/projects/<id>/project.fdb`, with rebase-on-commit (spec §6.1, D27).
//!
//! The project folder holds `finisher.project.json` ([`ProjectFile`]) and `.finisher/`: the
//! lease (one writer per folder), and under `segments/` the write-once export of the
//! committed records; the live database never lives there. [`Store::open`] acquires the lease
//! when it can and imports what other servers exported before it returns; [`Store::tick`]
//! then follows the folder, exports, and retries or re-checks the lease. One `Store` is the
//! single writer for its project database.

mod backup;
mod branch;
pub mod chain;
mod collect;
mod commit;
mod compact;
pub mod conflict_names;
mod copies;
mod counters;
mod create;
mod db;
mod dblock;
mod derived;
mod diffs;
mod export;
pub mod fsio;
mod guard;
mod history;
mod history_ops;
pub mod identity;
mod import;
pub mod lease;
mod link_records;
mod localdate;
mod migrations;
pub mod names;
mod newer;
mod open;
mod paths;
pub mod project_file;
mod rebuild;
mod recover;
pub mod scan;
pub mod segment;
pub mod server_id;
pub mod snapfile;
mod status;
pub mod storage_kind;
mod tick;
mod verify;
pub mod wire;
mod write;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fin_core::{CommittedTx, CoreError, Project};
use serde_json::json;

pub use branch::create::{At, BranchCreate, BranchSet};
pub use collect::{CollectPolicy, policy_of};
#[cfg(any(test, feature = "test-hooks"))]
pub use compact::CompactFail;
pub use compact::{
    CompactReport, CompactionStatus, PendingCompaction, TRIGGER as COMPACT_TRIGGER,
    grace as compaction_grace,
};
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub use copies::mount_root_for_test as copies_mount_root_for_test;
pub use create::validate_name;
#[cfg(any(test, feature = "test-hooks"))]
pub use create::{delay_next_create_start_for_test, fail_next_create_marker_for_test};
pub use db::{SNAPSHOT_EVERY, replay_record};
pub use export::ExportReport;
#[cfg(any(test, feature = "test-hooks"))]
pub use export::FailPoint;
pub use guard::WriteGuard;
pub use history::Generation;
pub use history_ops::checkpoint::CheckpointReq;
pub use history_ops::merge::MergeReq;
pub use history_ops::pick::{CherryPickReq, RevertReq};
pub use history_ops::undo::HistoryReq;
pub use import::{ForeignFile, ImportReport, ReadOnlyReason, Rewind};
#[cfg(any(test, feature = "test-hooks"))]
pub use migrations::fail_next_migration_step;
pub use migrations::{APPLICATION_ID, DB_VERSION};
pub use open::OpenOptions;
pub use paths::{PROJECT_FILE, db_path, project_file, server_data_dir};
pub use project_file::{Genesis, ProjectFile, read_project_file};
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub use rebuild::fail_next_rebuild_swap_for_test;
pub use rebuild::{RebuildReport, rebuild_project};
pub use recover::{RecoverReq, RecoverSel};
pub use server_id::{ServerIdentity, server_id, server_identity};
pub use status::{
    BranchStatus, INCOMPLETE_POLLS, ProjectStatus, QuarantineSummary, ReadOnly, StoreEvent, Warning,
};
pub use tick::{DueWork, TickOptions, TickReport};
pub use verify::{Check, VerifyReport};
pub use wire::{
    BranchCreated, BranchView, EngineEvent, HistoryResult, MergeReport, RecoverGroup,
    RecoverReport, SetAside, SkippedFile, TxSetAside,
};
pub use write::WriteOutcome;

/// The root branch, which every project has.
pub const MAIN: &str = fin_core::branch::MAIN;

/// A store failure. Converts to the one `CoreError` shape: `PROJECT_EXISTS`,
/// `PROJECT_NOT_FOUND`, `PROJECT_INVALID`, `PROJECT_DB_MISSING`, `PROJECT_DB_MISMATCH`,
/// `BRANCH_NOT_FOUND`, `BASE_REV_AHEAD`, `DB_TOO_NEW`, `NOT_A_PROJECT_DB`, `PROJECT_COPY`,
/// `PROJECT_READ_ONLY`, `LEASE_TAKEOVER_REFUSED`, `PROJECT_DIR_UNWRITABLE` (the user's project
/// folder), `IO_ERROR` (server data), `DB_ERROR`, or a `fin-core` code
/// (`CONFLICT`, validation codes) passed through.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("stored data does not decode: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl StoreError {
    pub fn code(&self) -> &str {
        match self {
            StoreError::Core(e) => &e.code,
            StoreError::Db(_) | StoreError::Json(_) => "DB_ERROR",
            StoreError::Io { .. } => "IO_ERROR",
        }
    }

    pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
        move |source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl From<StoreError> for CoreError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Core(e) => e,
            StoreError::Db(e) => CoreError::new("DB_ERROR", e.to_string()),
            StoreError::Json(e) => {
                CoreError::new("DB_ERROR", format!("stored data does not decode: {e}"))
            }
            StoreError::Io { path, source } => CoreError::new("IO_ERROR", source.to_string())
                .with_details(json!({ "path": path.to_string_lossy() })),
        }
    }
}

/// A failure writing into the user's project folder (as opposed to server data): the caller chose
/// the folder, so it is a fixable input error, not an internal one.
pub(crate) fn unwritable(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        CoreError::new(
            "PROJECT_DIR_UNWRITABLE",
            format!("cannot write {}: {source}", path.display()),
        )
        .with_details(json!({ "path": path.to_string_lossy(), "reason": source.to_string() }))
        .with_fix("choose a writable folder for the project")
        .into()
    }
}

/// What became of a `txId` ([`Store::tx_status`]).
#[derive(Debug, Clone, PartialEq)]
pub enum TxStatus {
    /// On its branch's history.
    Committed(CommittedTx),
    /// Set aside in quarantine (`reason`: `SUPERSEDED` for an applied record whose line lost
    /// to a newer lease epoch, `DIVERGED_TAIL`, `STALE_EPOCH`, `MALFORMED`, `CONFLICT_COPY`;
    /// `origin`: `local`, `folder`, `foreign`). Its bytes are kept.
    SetAside {
        branch: String,
        reason: String,
        origin: String,
    },
    /// Not known here.
    Unknown,
}

/// A branch head, cached in memory.
#[derive(Debug, Clone)]
pub(crate) struct Head {
    rev: u64,
    /// `txHash` of the row at `rev` (`None` at a branch's start). An import can replace rows
    /// with as many others (a diverged tail), so the rev alone does not identify a head.
    tip: Option<String>,
    project: Project,
}

/// The op log of one project. Not `Sync`; callers serialise access (one writer per project).
pub struct Store {
    conn: rusqlite::Connection,
    project_id: String,
    db_path: PathBuf,
    /// The user's project folder, which receives the export.
    project_dir: PathBuf,
    /// This server's id ([`server_id()`]), the `writerId` of exported segments.
    server_id: String,
    /// Why this server's id was renewed at open (its server data is a clone).
    identity_note: Option<String>,
    export: export::ExportState,
    import: import::ImportState,
    storage_warning: Option<String>,
    /// The lease as this store drives it: acquisition, re-checks, the ordering rule.
    ctl: tick::LeaseCtl,
    /// What the last import passes found, for status.
    follow: status::Follow,
    /// `finisher.project.json` needs a newer Finisher (`NEWER_FORMAT`).
    file_newer: bool,
    /// Its `collect` flag as last read (`collect.rs`).
    collect: collect::CollectPolicy,
    /// Branch heads, cached. Another `Store` (or process) on the same database may commit, so
    /// every read and commit first checks the newest rev in the database and catches up.
    heads: RefCell<HashMap<String, Head>>,
    /// The rebuild this store's open (or `Store::rebuild`) ran.
    rebuilt: Option<rebuild::RebuildReport>,
    /// The local date of the last daily backup taken or found, or `(date, ms)` of the last
    /// failed attempt (retried after [`tick::BACKUP_RETRY_MS`]).
    daily: tick::DailyState,
    /// Heavy work a tick found due but left to [`Store::run_due_backup`] /
    /// [`Store::run_due_compaction`] ([`TickOptions`]).
    due: tick::DueWork,
    /// The shared `db.lock` this store holds while it has the database open (`dblock`).
    db_lock: Option<std::fs::File>,
    /// Damaged folder files and since when they look unchanged (verify's 60 s rule).
    damage: RefCell<verify::DamageSeen>,
    /// Compaction manifests seen and their grace periods.
    compact: compact::CompactState,
    /// The last quarantine summary and the database version it was read at
    /// (`status::warnings::quarantine_summary`): status asks on every poll.
    quarantine_seen: RefCell<Option<((i64, u64), QuarantineSummary)>>,
    /// What writes check caption overflow with ([`OpenOptions::measurer`]).
    measurer: Option<std::sync::Arc<dyn fin_core::layout::TextMeasurer>>,
    /// What the last write did ([`Store::take_write_outcome`]).
    write_outcome: Option<WriteOutcome>,
    /// What writes check their target branch against ([`Store::with_guard`]).
    guard: WriteGuard,
    #[cfg(any(test, feature = "test-hooks"))]
    fail_after_apply: bool,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("project_id", &self.project_id)
            .field("db_path", &self.db_path)
            .field("project_dir", &self.project_dir)
            .finish()
    }
}

impl Store {
    /// What later writes check caption overflow with (plan 4C Task 20): edits, stale commits,
    /// undo, redo, merge, cherry-pick, revert and recovery re-runs. `None` measures nothing;
    /// replay (import, rebuild, open) never measures.
    pub fn set_measurer(&mut self, m: Option<std::sync::Arc<dyn fin_core::layout::TextMeasurer>>) {
        self.measurer = m;
    }

    /// The storage kind in use: the project's `storageOverride`, else detected at open.
    pub fn storage_kind(&self) -> storage_kind::StorageKind {
        self.import.kind
    }

    /// Why a `storageOverride` in the project file was ignored, if it was.
    pub fn storage_warning(&self) -> Option<&str> {
        self.storage_warning.as_deref()
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Path of the live database.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// The project folder.
    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    /// This server's id, the `writerId` of the segments it exports.
    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    /// The id a branch reference names ([`Store::resolve_branch`]), else `r` itself (for
    /// reads that answer a default for an unknown branch).
    fn branch_or_raw(&self, r: &str) -> String {
        self.resolve_branch(r).unwrap_or_else(|_| r.to_string())
    }

    /// The head revision of `branch` (a branch reference: an id, an active name or
    /// `<name>#<id>`; 0 for a branch that does not exist). Catches up with commits made
    /// through another `Store`; if the database cannot be read, the cached head is returned.
    pub fn head(&self, branch: &str) -> u64 {
        self.head_of(&self.branch_or_raw(branch))
    }

    /// [`Store::head`] of a branch id.
    pub(crate) fn head_of(&self, id: &str) -> u64 {
        let mut heads = self.heads.borrow_mut();
        let _ = refresh(&self.conn, &mut heads, id);
        heads.get(id).map_or(0, |h| h.rev)
    }

    /// The head revision of `branch` (a reference) and the project at it.
    pub fn snapshot(&self, branch: &str) -> Result<(u64, Project), StoreError> {
        let id = self.resolve_branch(branch)?;
        let mut heads = self.heads.borrow_mut();
        refresh(&self.conn, &mut heads, &id)?;
        let head = heads.get(&id).ok_or_else(|| branch_not_found(branch))?;
        Ok((head.rev, head.project.clone()))
    }

    /// Committed txs on `branch` (a reference) with `rev > after`, in rev order. Below the
    /// branch's fork they are its ancestors' (each `tx.branch` names the branch that holds
    /// it).
    pub fn ops_since(&self, branch: &str, after: u64) -> Result<Vec<CommittedTx>, StoreError> {
        let id = self.resolve_branch(branch)?;
        if db::latest_rev(&self.conn, &id)?.is_none() {
            return Err(branch_not_found(branch).into());
        }
        branch::lineage::records_since(&self.conn, &id, after)
    }

    /// Committed txs on `branch` with `rev > after`, with the branch's generation, read
    /// together: a caller that later commits with [`Store::commit_at_generation`] passes it.
    pub fn ops_since_at(
        &self,
        branch: &str,
        after: u64,
    ) -> Result<(Generation, Vec<CommittedTx>), StoreError> {
        let txn = self.conn.unchecked_transaction()?;
        let id = self.resolve_branch(branch)?;
        let ops = self.ops_since(&id, after)?;
        let generation = history::generation(&txn, &id)?;
        txn.commit()?;
        Ok((generation, ops))
    }

    /// The rewind count of `branch` in this database: bumped whenever an import takes
    /// records out of its history (module `history`). 0 when it cannot be read. Callers that
    /// commit against it hold [`Store::generation_token`], which names the database too.
    pub fn generation(&self, branch: &str) -> u64 {
        history::current(&self.conn, &self.branch_or_raw(branch)).unwrap_or(0)
    }

    /// The [`Generation`] of `branch` (this database's incarnation and the rewind count), as
    /// [`Store::commit_at_generation`] takes it.
    pub fn generation_token(&self, branch: &str) -> Generation {
        history::generation(&self.conn, &self.branch_or_raw(branch)).unwrap_or(Generation {
            incarnation: String::new(),
            n: 0,
        })
    }

    /// The `genesisHash` every segment and snapshot of this project names
    /// ([`project_file::genesis_hash`] of the rev-0 genesis).
    pub fn genesis_hash(&self) -> String {
        project_file::db_genesis_hash(&self.conn).unwrap_or_default()
    }

    /// This database's incarnation (module `history`): new whenever it is rebuilt.
    pub fn incarnation(&self) -> String {
        history::incarnation(&self.conn).unwrap_or_default()
    }

    /// Takes today's daily backup (`<project-data>/backups/daily-YYYY-MM-DD.fdb`, the local
    /// calendar day of unix ms `now_ms`) unless that day has one, keeping the last 7. `tick`
    /// calls it; returns the new backup.
    pub fn daily_backup(&mut self, now_ms: i64) -> Result<Option<PathBuf>, StoreError> {
        let dir = self
            .db_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        backup::daily(&self.conn, &dir, now_ms)
    }

    /// What became of `tx_id` (`tx.get`): committed, set aside (a superseded record is
    /// `SetAside { reason: "SUPERSEDED" }`), or unknown.
    pub fn tx_status(&self, tx_id: &str) -> Result<TxStatus, StoreError> {
        history::tx_status(&self.conn, tx_id)
    }

    /// Test hook: `(txHash, reason, origin)` of the records set aside now, by `txHash`.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn quarantined_for_test(&self) -> Vec<(String, String, String)> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT tx_hash, reason, origin FROM quarantine WHERE current = 1 ORDER BY tx_hash",
            )
            .expect("prepare");
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    /// What the last write did (plan D, Decision 22), cleared by the call: `Written`,
    /// `Deduped` (a retry of a committed txId) or `Planned` (a dry run); `None` when no write
    /// was reached since the last call, or the last one failed. Take it before a call (to
    /// clear it) and after, with no other call on this store in between.
    pub fn take_write_outcome(&mut self) -> Option<WriteOutcome> {
        self.write_outcome.take()
    }

    /// Test hook: the next `commit` fails after applying and writing its rows but before the
    /// SQLite commit, as if the process died there.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn fail_next_commit_after_apply(&mut self) {
        self.fail_after_apply = true;
    }
}

/// Brings the cached head of `branch` up to the newest rev in the database: replays the txs
/// committed since the cached rev, or loads the latest snapshot plus its tail when there is no
/// usable cache. The cache is usable only while the row at its rev still has the hash it was
/// built from: an import (here or through another `Store` on the same database) may have
/// replaced rows. A branch at its fork has no row there: its tip is the fork hash
/// ([`branch::lineage::hash_at`]). Drops the cache entry when the branch does not exist.
pub(crate) fn refresh(
    conn: &rusqlite::Connection,
    heads: &mut HashMap<String, Head>,
    branch: &str,
) -> Result<(), StoreError> {
    let Some(latest) = db::latest_rev(conn, branch)? else {
        heads.remove(branch);
        return Ok(());
    };
    if let Some(head) = heads.get_mut(branch)
        && head.rev <= latest
        && branch::lineage::hash_at(conn, branch, head.rev)? == head.tip
    {
        for row in db::record_rows(conn, branch, head.rev)? {
            head.project = db::replay_row(&head.project, branch, &row)?.project;
            head.rev = row.0;
            head.tip = Some(db::hash_bytes(&row.2));
        }
        return Ok(());
    }
    let (mut rev, mut project) =
        db::latest_snapshot(conn, branch)?.ok_or_else(|| branch_not_found(branch))?;
    let mut tip = branch::lineage::hash_at(conn, branch, rev)?;
    for row in db::record_rows(conn, branch, rev)? {
        project = db::replay_row(&project, branch, &row)?.project;
        rev = row.0;
        tip = Some(db::hash_bytes(&row.2));
    }
    heads.insert(branch.to_string(), Head { rev, tip, project });
    Ok(())
}

pub(crate) fn branch_not_found(branch: &str) -> CoreError {
    CoreError::new(
        "BRANCH_NOT_FOUND",
        format!("branch {branch:?} does not exist"),
    )
    .with_details(json!({ "branch": branch }))
    .with_fix(format!("use branch {MAIN:?}"))
}
