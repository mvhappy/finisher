//! Building a project database from the project folder (sub-project 4 spec §3.8).
//!
//! # When
//!
//! - **Missing:** a server with no database for a project whose file carries its [`Genesis`]
//!   (`Store::open`).
//! - **Corrupt:** `PRAGMA quick_check` fails on open ([`quick_check`]). A file that is not a
//!   project database at all (`NOT_A_PROJECT_DB`, `DB_TOO_NEW`, checked first) is never
//!   rebuilt over.
//! - **Manual:** [`Store::rebuild`] (`fin project rebuild`).
//!
//! # How ([`rebuild`]), under `<project-data>/rebuild.lock`
//!
//! 1. Re-check under the lock: another open of this server data may have rebuilt meanwhile
//!    (the database now exists, or passes `quick_check`); then its database is used.
//! 2. The rev-0 state: the old database's rev-0 snapshot when it can be read (the history
//!    was built on it), else `new_project` of the file's genesis; neither is
//!    `DB_CORRUPT`. A genesis that does not match a readable rev 0 is reported
//!    ([`RebuildReport::genesis_mismatch`]).
//! 3. Build `project.fdb.rebuild-<ulid>` beside the database: the schema with a fresh
//!    incarnation (`crate::history`), rev 0, then one import pass of the whole folder
//!    ([`Store::import`], through a store on the new file that holds no lease), offered the
//!    folder's snapshots as seeds: one seeds only if its `txHash` is on the resolved chain
//!    (`import::apply`, step 3; the others are ignored). An import error or a fork stops
//!    the branch where the folder does: the database holds what the folder holds, and the
//!    store reports it as it would on any open.
//! 4. Salvage from the old database ([`salvage`]): its unexported records into
//!    `quarantine` (`SALVAGED`, origin `salvage`, bytes kept), its quarantine rows (origin
//!    `salvage`), its id counters (never lowered: ids those records used are never handed out
//!    again), its `projectDir` and server ids. Whatever cannot be read is skipped.
//! 5. Verify the new database with the checks that need no replay (`crate::verify`): the
//!    import just replayed everything a seed did not cover. Mismatches are reported, never
//!    fatal.
//! 6. Swap: the old database is renamed to `project.fdb.corrupt-<UTC ts>` together with its
//!    `-wal` and `-shm` (renamed alongside, so SQLite never replays that WAL onto the new
//!    file, and the kept copy still opens with it); without an old database, stale
//!    `-wal`/`-shm` files are removed. The new file is then moved into place without
//!    replacing anything (`fsio::rename_noreplace`) and the directory synced.
//!
//! A crash at any step leaves either the old database in place (plus a `rebuild-` file that
//! is never used again) or the new one. Two opens racing share the first one's database.
//!
//! Long-running (it imports the whole folder): `fin-node` runs it, and [`Store::verify`], on
//! the per-engine worker thread (plan Decision 7, Task 14).

mod salvage;
mod seeds;

use std::path::{Path, PathBuf};

use fin_core::{CoreError, Project, new_project};
use rusqlite::{Connection, ErrorCode};
use serde::Serialize;
use serde_json::json;

use crate::open::OpenOptions;
use crate::project_file::{Genesis, ProjectFile};
use crate::storage_kind::StorageKind;
use crate::verify::VerifyReport;
use crate::{Head, MAIN, ServerIdentity, Store, StoreError, copies, db, export, fsio, history};

/// What a rebuild did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuildReport {
    /// `missing`, `corrupt` or `manual`.
    pub reason: String,
    /// What `quick_check` reported (`corrupt`).
    pub problem: Option<String>,
    /// Records in the new database.
    pub records: u64,
    /// Rows salvaged from the old database into quarantine: its unexported records
    /// (`SALVAGED`) and its own quarantine rows.
    pub salvaged: u32,
    /// The seq of the folder snapshot the rebuild was seeded from.
    pub seeded_from_snapshot: Option<u64>,
    /// Where the old database was kept (`project.fdb.corrupt-<ts>`).
    pub corrupt_copy: Option<PathBuf>,
    /// The project file's genesis does not describe the old database's rev 0 (that rev 0
    /// was used).
    pub genesis_mismatch: bool,
    /// Why the import stopped early (it stopped where any open would).
    pub import_error: Option<CoreError>,
    /// The checks of the new database that need no replay.
    pub verify: VerifyReport,
}

/// Why a rebuild runs.
#[derive(Debug, Clone)]
pub(crate) enum Why {
    Missing,
    Corrupt(String),
    Manual,
}

/// What a rebuild works from.
pub(crate) struct Plan<'a> {
    pub(crate) db_path: &'a Path,
    pub(crate) project_dir: &'a Path,
    pub(crate) file: &'a ProjectFile,
    pub(crate) identity: &'a ServerIdentity,
    pub(crate) kind: StorageKind,
    pub(crate) opts: &'a OpenOptions,
}

/// The genesis of `file`, or `PROJECT_DB_MISSING` when the file has none (a sub-project 2
/// project whose database was never opened by sub-project 4).
pub(crate) fn genesis(file: &ProjectFile, db_path: &Path) -> Result<Genesis, StoreError> {
    file.genesis.clone().ok_or_else(|| missing(file, db_path))
}

fn missing(file: &ProjectFile, db_path: &Path) -> StoreError {
    CoreError::new(
        "PROJECT_DB_MISSING",
        format!("the database for project {} is missing", file.id),
    )
    .with_details(json!({ "projectId": file.id, "path": db_path.to_string_lossy() }))
    .with_fix("restore project.fdb into the server data dir: this project's folder does not record how to rebuild it (open it once with the server that has its database)")
    .into()
}

/// `Some(problem)` when `PRAGMA quick_check` fails on the database at `path` (or SQLite
/// reports it corrupt while checking); `None` when it passes.
pub(crate) fn quick_check(path: &Path) -> Result<Option<String>, StoreError> {
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let checked = Connection::open_with_flags(path, flags).and_then(|c| {
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut stmt = c.prepare("PRAGMA quick_check(20)")?;
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    });
    match checked {
        Ok(rows) if rows == ["ok"] => Ok(None),
        Ok(rows) => Ok(Some(rows.join("; "))),
        Err(e) if is_corruption(&e) => Ok(Some(e.to_string())),
        Err(e) => Err(e.into()),
    }
}

/// SQLite says the file is damaged.
pub(crate) fn is_corruption(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _)
        if matches!(f.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase))
}

/// Rebuilds the database at `plan.db_path` (module docs). `None`: another open rebuilt it
/// meanwhile, and that database is used.
pub(crate) fn rebuild(plan: &Plan, why: Why) -> Result<Option<RebuildReport>, StoreError> {
    let dir = plan.db_path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(StoreError::io(dir))?;
    let lock_path = dir.join("rebuild.lock");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(StoreError::io(&lock_path))?;
    lock.lock().map_err(StoreError::io(&lock_path))?;
    let old = match &why {
        Why::Missing if plan.db_path.exists() => return Ok(None),
        Why::Missing => None,
        Why::Corrupt(_) if quick_check(plan.db_path)?.is_none() => return Ok(None),
        Why::Corrupt(_) | Why::Manual => Some(plan.db_path),
    };
    let old_rev0 = old.and_then(|p| salvage::rev0(p, &plan.file.id));
    let rev0 = match (&old_rev0, &plan.file.genesis) {
        (Some(p), _) => p.clone(),
        (None, Some(g)) => new_project(plan.file.id.clone(), g.name.clone(), g.rate)?,
        (None, None) if old.is_none() => return Err(missing(plan.file, plan.db_path)),
        (None, None) => return Err(corrupt_unrecoverable(plan, &why)),
    };
    let genesis_mismatch = match (&old_rev0, &plan.file.genesis) {
        (Some(p), Some(g)) => new_project(plan.file.id.clone(), g.name.clone(), g.rate)
            .map_or(true, |fresh| fresh != *p),
        _ => false,
    };
    let ulid = ulid::Ulid::new().to_string().to_ascii_lowercase();
    let tmp = dir.join(format!("project.fdb.rebuild-{ulid}"));
    let result = build(plan, &tmp, rev0, old).and_then(|mut report| {
        report.reason = match &why {
            Why::Missing => "missing",
            Why::Corrupt(_) => "corrupt",
            Why::Manual => "manual",
        }
        .to_string();
        report.problem = match &why {
            Why::Corrupt(p) => Some(p.clone()),
            _ => None,
        };
        report.genesis_mismatch = genesis_mismatch;
        report.corrupt_copy = swap(plan.db_path, &tmp, old.is_some())?;
        Ok(report)
    });
    match result {
        Ok(r) => Ok(Some(r)),
        Err(e) => {
            for ext in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{ext}", tmp.display()));
            }
            Err(e)
        }
    }
}

fn corrupt_unrecoverable(plan: &Plan, why: &Why) -> StoreError {
    let problem = match why {
        Why::Corrupt(p) => p.clone(),
        _ => String::new(),
    };
    CoreError::new(
        "DB_CORRUPT",
        format!(
            "the database of project {} is damaged and cannot be rebuilt: neither it nor the project file records the project's rev 0",
            plan.file.id
        ),
    )
    .with_details(json!({ "path": plan.db_path.to_string_lossy(), "problem": problem }))
    .with_fix("restore project.fdb from <server-data>/projects/<id>/backups (daily-*.fdb), or open the project once with a server whose database is intact")
    .into()
}

/// Steps 3–5 of the module docs, into `tmp`.
fn build(
    plan: &Plan,
    tmp: &Path,
    rev0: Project,
    old: Option<&Path>,
) -> Result<RebuildReport, StoreError> {
    let mut conn = db::connect(tmp, true)?;
    db::init(&mut conn, &rev0, MAIN)?;
    copies::record(&conn, plan.project_dir)?;
    history::note_server_id(&conn, &plan.identity.id)?;
    let heads = std::collections::HashMap::from([(
        MAIN.to_string(),
        Head {
            rev: 0,
            tip: None,
            project: rev0,
        },
    )]);
    let mut store = Store::new(
        conn,
        plan.file.id.clone(),
        tmp.to_path_buf(),
        plan.project_dir,
        plan.identity.clone(),
        heads,
        (plan.kind, None),
        plan.opts,
    );
    store.export.lease = export::StoreLease::NotHeld;
    store.import.seeds = seeds::read(plan.project_dir, &plan.file.id);
    let imported = store.import()?;
    let salvaged = match old {
        Some(old) => salvage::run(old, &store.conn, crate::commit::now_ms())?,
        None => 0,
    };
    let records: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM txs", [], |r| r.get(0))?;
    let verify = crate::verify::run(&crate::verify::Subject {
        conn: &store.conn,
        project_dir: plan.project_dir,
        project_id: &plan.file.id,
        kind: plan.kind,
        live: None,
    });
    let seeded = store.import.seeded;
    store
        .conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    drop(store);
    Ok(RebuildReport {
        reason: String::new(),
        problem: None,
        records: db::to_u64(records)?,
        salvaged,
        seeded_from_snapshot: seeded,
        corrupt_copy: None,
        genesis_mismatch: false,
        import_error: imported.error,
        verify,
    })
}

/// Step 6 of the module docs. Returns where the old database went.
fn swap(db_path: &Path, tmp: &Path, had_old: bool) -> Result<Option<PathBuf>, StoreError> {
    let dir = db_path.parent().unwrap_or(Path::new("."));
    let side = |p: &Path, ext: &str| PathBuf::from(format!("{}{ext}", p.display()));
    let mut kept = None;
    if had_old {
        let mut ms = crate::commit::now_ms();
        let corrupt = loop {
            let c = dir.join(format!(
                "project.fdb.corrupt-{}",
                crate::backup::timestamp(ms)
            ));
            if !c.exists() && !side(&c, "-wal").exists() {
                break c;
            }
            ms += 1;
        };
        match fsio::rename_noreplace(db_path, &corrupt) {
            Ok(()) => kept = Some(corrupt.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(StoreError::io(db_path)(e)),
        }
        for ext in ["-wal", "-shm"] {
            match fsio::rename_noreplace(&side(db_path, ext), &side(&corrupt, ext)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(StoreError::io(&side(db_path, ext))(e)),
            }
        }
    } else {
        for ext in ["-wal", "-shm"] {
            let s = side(db_path, ext);
            match std::fs::remove_file(&s) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(StoreError::io(&s)(e)),
            }
        }
    }
    match fsio::rename_noreplace(tmp, db_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Only an open that does not take `rebuild.lock` could have made it: theirs wins.
            for ext in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(side(tmp, ext));
            }
            return Ok(kept);
        }
        Err(e) => return Err(StoreError::io(db_path)(e)),
    }
    fsio::sync_dir(dir).map_err(StoreError::io(dir))?;
    Ok(kept)
}

impl Store {
    /// Rebuilds this project's database from the folder (`fin project rebuild`; module
    /// docs). When this store holds the lease and may write, what is not exported yet is
    /// exported first, so only what cannot be exported is salvaged. The old database is kept
    /// as `project.fdb.corrupt-<ts>`. Afterwards an import pass runs, as at open; callers
    /// holding a generation of the old database are refused (`HISTORY_REWOUND`, `REBUILT`).
    ///
    /// This store's connection is closed for the swap: other connections to the old file (a
    /// second `Store` on it) keep using the old file.
    pub fn rebuild(&mut self) -> Result<RebuildReport, StoreError> {
        let now = self.ctl.now();
        export::export_pending(self, now, true)?;
        let file = crate::project_file::read_project_file(&self.project_dir)?;
        let identity = ServerIdentity {
            id: self.server_id.clone(),
            renewed: None,
            shortcut_ok: false,
        };
        let opts = OpenOptions::default();
        let plan = Plan {
            db_path: &self.db_path.clone(),
            project_dir: &self.project_dir.clone(),
            file: &file,
            identity: &identity,
            kind: self.storage_kind(),
            opts: &opts,
        };
        // Close our connection: the file is renamed (Windows cannot rename it while open).
        let conn = std::mem::replace(&mut self.conn, Connection::open_in_memory()?);
        drop(conn);
        let rebuilt = rebuild(&plan, Why::Manual);
        self.conn = db::connect(&self.db_path, false)?;
        let report = rebuilt?.ok_or_else(|| {
            StoreError::from(CoreError::new("DB_ERROR", "the rebuild did not run"))
        })?;
        self.adopt_rebuilt(Some(report.clone()))?;
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
