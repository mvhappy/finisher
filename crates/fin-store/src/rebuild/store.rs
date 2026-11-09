//! The rebuild entry points: [`Store::rebuild`] on an open store, [`rebuild_project`]
//! without one (`fin project rebuild`, plan Task 15).

use std::path::Path;

use fin_core::CoreError;
use rusqlite::Connection;

use super::{Plan, RebuildReport, Why, files, rebuild};
use crate::open::OpenOptions;
use crate::{
    Store, StoreError, db, db_path, dblock, export, history, server_identity, storage_kind,
};

/// Rebuilds the database of the project in `project_dir` from its folder, with no store open
/// on it (`fin project rebuild` while no server has the project open). Refused with
/// `DB_IN_USE` while any store on this server data has it open, and with
/// `REBUILD_WOULD_LOSE` when the folder no longer holds records the database exported.
/// Without a database this is the rebuild an open would do.
pub fn rebuild_project(
    server_data: &Path,
    project_dir: &Path,
) -> Result<RebuildReport, StoreError> {
    let file = crate::project_file::read_project_file(project_dir)?;
    let db_path = db_path(server_data, &file.id);
    let opts = OpenOptions::default();
    opts.check_local(db_path.parent().unwrap_or(server_data))?;
    let identity = server_identity(server_data)?;
    let kind = storage_kind::resolve(project_dir, file.storage_override.as_ref()).0;
    let plan = Plan {
        db_path: &db_path,
        project_dir,
        file: &file,
        identity: &identity,
        kind,
        opts: &opts,
    };
    let dir = db_path.parent().unwrap_or(Path::new("."));
    let lock = dblock::exclusive_now(dir)?;
    let why = if db_path.is_file() {
        Why::Manual
    } else {
        Why::Missing
    };
    rebuild(&plan, why, Some(lock))?
        .ok_or_else(|| CoreError::new("DB_ERROR", "the rebuild did not run").into())
}

impl Store {
    /// Rebuilds this project's database from the folder (`fin project rebuild`; module docs
    /// of `rebuild`). When this store holds the lease and may write, what is not exported
    /// yet is exported first. Refused with `DB_IN_USE` while another store has the database
    /// open, and with `REBUILD_WOULD_LOSE` (nothing changed) when the folder no longer holds
    /// records this database exported. The old database is kept as
    /// `project.fdb.corrupt-<ts>`. Afterwards an import pass runs, as at open; callers holding
    /// a generation of the old database are refused (`HISTORY_REWOUND`, `REBUILT`).
    ///
    /// On any failure the store keeps working on the database it had.
    pub fn rebuild(&mut self) -> Result<RebuildReport, StoreError> {
        let now = self.ctl.now();
        export::export_pending(self, now, true)?;
        let file = crate::project_file::read_project_file(&self.project_dir)?;
        let dir = self
            .db_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        // Our own shared lock goes first; then nobody else may hold it.
        self.db_lock = None;
        let lock = match dblock::exclusive_now(&dir) {
            Ok(l) => l,
            Err(e) => {
                self.db_lock = Some(dblock::shared(&dir)?);
                return Err(e);
            }
        };
        let identity = crate::ServerIdentity {
            id: self.server_id.clone(),
            renewed: None,
            renewed_reason: None,
            shortcut_ok: false,
        };
        let opts = OpenOptions::default();
        let (db_path, project_dir) = (self.db_path.clone(), self.project_dir.clone());
        let plan = Plan {
            db_path: &db_path,
            project_dir: &project_dir,
            file: &file,
            identity: &identity,
            kind: self.storage_kind(),
            opts: &opts,
        };
        // Close our connection: the file is renamed (Windows cannot rename it while open).
        drop(std::mem::replace(
            &mut self.conn,
            Connection::open_in_memory()?,
        ));
        let rebuilt = rebuild(&plan, Why::Manual, Some(lock));
        // Whatever happened, a database must be in place before reconnecting: a swap that
        // failed moves the old one back; should it not have, the newest kept copy returns.
        if !db_path.exists()
            && let Some(newest) = files::copies(&dir, files::REPLACED).pop()
        {
            for ext in ["", "-wal", "-shm"] {
                let _ = crate::fsio::rename_noreplace(
                    Path::new(&format!("{}{ext}", newest.display())),
                    Path::new(&format!("{}{ext}", db_path.display())),
                );
            }
        }
        self.db_lock = Some(dblock::shared(&dir)?);
        self.conn = db::connect(&db_path, false)?;
        // Another connection: its change counters say nothing about the last summary.
        *self.quarantine_seen.get_mut() = None;
        let report = match rebuilt {
            Ok(Some(r)) => r,
            Ok(None) => {
                return Err(CoreError::new("DB_ERROR", "the rebuild did not run").into());
            }
            Err(e) => {
                // The same database as before: the cached heads still describe it.
                return Err(e);
            }
        };
        self.adopt_rebuilt(Some(report.clone()))?;
        self.queue_rebuilt();
        self.import_pass()?;
        Ok(report)
    }

    /// The rebuild that this store's open (or [`Store::rebuild`]) ran, if any.
    pub fn last_rebuild(&self) -> Option<&RebuildReport> {
        self.rebuilt.as_ref()
    }

    /// After the database file was replaced: forget what was cached about the old one.
    fn adopt_rebuilt(&mut self, report: Option<RebuildReport>) -> Result<(), StoreError> {
        history::note_server_id(&self.conn, &self.server_id)?;
        history::ensure_incarnation(&self.conn)?;
        let kind = self.import.kind;
        self.import = crate::import::ImportState::with_kind(kind);
        let heads = self.heads.get_mut();
        heads.clear();
        for branch in db::branches(&self.conn)? {
            crate::refresh(&self.conn, heads, &branch)?;
        }
        self.rebuilt = report;
        Ok(())
    }
}

impl Store {
    /// Stops status reporting `SALVAGE_INCOMPLETE` for the kept copy named `copy` (its file
    /// name in the project's server data); from the next rebuild on it is pruned like any
    /// other copy. `false` when no such incomplete salvage is recorded.
    pub fn acknowledge_salvage(&mut self, copy: &str) -> Result<bool, StoreError> {
        let mut all = super::incomplete(&self.conn);
        let Some(i) = all.iter_mut().find(|i| i.copy == copy && !i.acknowledged) else {
            return Ok(false);
        };
        i.acknowledged = true;
        super::set_incomplete(&self.conn, &all)?;
        Ok(true)
    }

    /// The kept copies whose salvage was incomplete and not acknowledged, with their first
    /// errors (status `SALVAGE_INCOMPLETE`).
    pub(crate) fn unacknowledged_salvages(&self) -> Vec<(String, Vec<String>)> {
        super::incomplete(&self.conn)
            .into_iter()
            .filter(|i| !i.acknowledged)
            .map(|i| (i.copy, i.errors))
            .collect()
    }
}
