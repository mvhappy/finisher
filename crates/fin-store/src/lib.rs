//! Finisher's local store: one SQLite op log per project at
//! `<server-data>/projects/<id>/project.fdb`, with rebase-on-commit (spec §6.1, D27).
//!
//! The project folder only holds `finisher.project.json` (`{id, name, schemaVersion}`); the
//! live database never lives there. One `Store` is the single writer for its project.

mod backup;
mod commit;
mod db;
mod migrations;
mod paths;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use fin_core::{CommittedTx, CoreError, Project, Rational, SCHEMA_VERSION, new_project};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub use db::{SNAPSHOT_EVERY, replay_record};
#[cfg(any(test, feature = "test-hooks"))]
pub use migrations::fail_next_migration_step;
pub use migrations::{APPLICATION_ID, DB_VERSION};
pub use paths::{PROJECT_FILE, db_path, project_file, server_data_dir};

/// The only branch sub-project 2 creates.
pub const MAIN: &str = "main";

/// A store failure. Converts to the one `CoreError` shape: `PROJECT_EXISTS`,
/// `PROJECT_NOT_FOUND`, `PROJECT_INVALID`, `PROJECT_DB_MISSING`, `PROJECT_DB_MISMATCH`,
/// `BRANCH_NOT_FOUND`, `BASE_REV_AHEAD`, `DB_TOO_NEW`, `NOT_A_PROJECT_DB`, `PROJECT_DIR_UNWRITABLE` (the user's project folder),
/// `IO_ERROR` (server data), `DB_ERROR`, or a `fin-core` code
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

    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
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
fn unwritable(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
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

/// `finisher.project.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
}

/// Reads and checks `<project_dir>/finisher.project.json`.
pub fn read_project_file(project_dir: &Path) -> Result<ProjectFile, StoreError> {
    let path = project_file(project_dir);
    let invalid = |why: String| {
        CoreError::new(
            "PROJECT_INVALID",
            format!("{} is not a valid project file: {why}", path.display()),
        )
        .with_details(json!({ "path": path.to_string_lossy() }))
    };
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(CoreError::new(
                "PROJECT_NOT_FOUND",
                format!("no {PROJECT_FILE} in {}", project_dir.display()),
            )
            .with_details(json!({ "path": path.to_string_lossy() }))
            .with_fix(
                "create the project first, or pass the folder that holds finisher.project.json",
            )
            .into());
        }
        Err(e) => return Err(StoreError::io(&path)(e)),
    };
    let file: ProjectFile = serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
    // The id names a directory under the server data dir: accept only a lowercase ULID.
    if file.id.len() != 26
        || !file
            .id
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    {
        return Err(invalid(format!("id {:?} is not a lowercase ULID", file.id)).into());
    }
    if file.schema_version != SCHEMA_VERSION {
        return Err(CoreError::new("SCHEMA_VERSION", format!("project schema version {} is not supported", file.schema_version))
            .with_details(json!({ "expected": SCHEMA_VERSION, "actual": file.schema_version, "path": path.to_string_lossy() }))
            .into());
    }
    Ok(file)
}

/// A branch head, cached in memory.
#[derive(Debug, Clone)]
struct Head {
    rev: u64,
    project: Project,
}

/// The op log of one project. Not `Sync`; callers serialise access (one writer per project).
pub struct Store {
    conn: rusqlite::Connection,
    project_id: String,
    db_path: PathBuf,
    /// Branch heads, cached. Another `Store` (or process) on the same database may commit, so
    /// every read and commit first checks the newest rev in the database and catches up.
    heads: RefCell<HashMap<String, Head>>,
    #[cfg(any(test, feature = "test-hooks"))]
    fail_after_apply: bool,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("project_id", &self.project_id)
            .field("db_path", &self.db_path)
            .finish()
    }
}

impl Store {
    /// Creates a project: a new lowercase-ULID id, the database at
    /// `<server_data>/projects/<id>/project.fdb` with `main` at rev 0, then
    /// `<project_dir>/finisher.project.json` (written via an fsync'd temp file and rename).
    /// Fails with `PROJECT_EXISTS` when the project file is already there.
    pub fn create(
        server_data: &Path,
        project_dir: &Path,
        name: &str,
        rate: Rational,
    ) -> Result<Store, StoreError> {
        let file_path = project_file(project_dir);
        if fs::symlink_metadata(&file_path).is_ok() {
            return Err(CoreError::new(
                "PROJECT_EXISTS",
                format!("{} already holds a project", project_dir.display()),
            )
            .with_details(json!({ "path": file_path.to_string_lossy() }))
            .with_fix("open the existing project, or choose another folder")
            .into());
        }
        let id = ulid::Ulid::new().to_string().to_ascii_lowercase();
        let project = new_project(id.clone(), name.to_string(), rate)?;
        fs::create_dir_all(project_dir).map_err(unwritable(project_dir))?;
        let db_path = db_path(server_data, &id);
        let db_dir = db_path.parent().unwrap_or(server_data);
        fs::create_dir_all(db_dir).map_err(StoreError::io(db_dir))?;
        if db_path.exists() {
            return Err(CoreError::new(
                "DB_ERROR",
                format!("{} already exists", db_path.display()),
            )
            .into());
        }
        let mut conn = db::connect(&db_path, true)?;
        db::init(&mut conn, &project, MAIN)?;
        let marker = ProjectFile {
            id: id.clone(),
            name: name.to_string(),
            schema_version: SCHEMA_VERSION,
        };
        let mut bytes = serde_json::to_vec_pretty(&marker)?;
        bytes.push(b'\n');
        paths::write_atomic(&file_path, &bytes).map_err(unwritable(&file_path))?;
        let heads = HashMap::from([(MAIN.to_string(), Head { rev: 0, project })]);
        Ok(Store::new(conn, id, db_path, heads))
    }

    /// Opens the project whose `finisher.project.json` is in `project_dir`. A missing database
    /// is `PROJECT_DB_MISSING` (rebuilding from segments is sub-project 4).
    ///
    /// The database is checked read-only first: one written by a newer Finisher is
    /// `DB_TOO_NEW`, anything else that is not a project database `NOT_A_PROJECT_DB`, and
    /// neither is modified. An older one is backed up and migrated to [`DB_VERSION`].
    pub fn open(server_data: &Path, project_dir: &Path) -> Result<Store, StoreError> {
        let file = read_project_file(project_dir)?;
        let db_path = db_path(server_data, &file.id);
        if !db_path.is_file() {
            return Err(CoreError::new("PROJECT_DB_MISSING", format!("the database for project {} is missing", file.id))
                .with_details(json!({ "projectId": file.id, "path": db_path.to_string_lossy() }))
                .with_fix("restore project.fdb into the server data dir; rebuilding from segments is not available yet")
                .into());
        }
        migrations::check(&db_path)?;
        let mut conn = db::connect(&db_path, false)?;
        let stored = db::meta(&conn, "projectId")?;
        if stored.as_deref() != Some(file.id.as_str()) {
            return Err(CoreError::new("PROJECT_DB_MISMATCH", format!("{} belongs to another project", db_path.display()))
                .with_details(json!({ "projectId": file.id, "dbProjectId": stored, "path": db_path.to_string_lossy() }))
                .into());
        }
        migrations::migrate(&mut conn, db_path.parent().unwrap_or(server_data))?;
        db::set_meta(&conn, "lastOpenedBy", &db::written_by())?;
        let mut heads = HashMap::new();
        for branch in db::branches(&conn)? {
            refresh(&conn, &mut heads, &branch)?;
        }
        Ok(Store::new(conn, file.id, db_path, heads))
    }

    fn new(
        conn: rusqlite::Connection,
        project_id: String,
        db_path: PathBuf,
        heads: HashMap<String, Head>,
    ) -> Store {
        Store {
            conn,
            project_id,
            db_path,
            heads: RefCell::new(heads),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_after_apply: false,
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Path of the live database.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// The head revision of `branch` (0 for a branch that does not exist). Catches up with
    /// commits made through another `Store`; if the database cannot be read, the cached head is
    /// returned.
    pub fn head(&self, branch: &str) -> u64 {
        let mut heads = self.heads.borrow_mut();
        let _ = refresh(&self.conn, &mut heads, branch);
        heads.get(branch).map_or(0, |h| h.rev)
    }

    /// The head revision of `branch` and the project at it.
    pub fn snapshot(&self, branch: &str) -> Result<(u64, Project), StoreError> {
        let mut heads = self.heads.borrow_mut();
        refresh(&self.conn, &mut heads, branch)?;
        let head = heads.get(branch).ok_or_else(|| branch_not_found(branch))?;
        Ok((head.rev, head.project.clone()))
    }

    /// Committed txs on `branch` with `rev > after`, in rev order.
    pub fn ops_since(&self, branch: &str, after: u64) -> Result<Vec<CommittedTx>, StoreError> {
        if db::latest_rev(&self.conn, branch)?.is_none() {
            return Err(branch_not_found(branch).into());
        }
        db::txs_since(&self.conn, branch, after)
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
/// usable cache. Drops the cache entry when the branch does not exist.
pub(crate) fn refresh(
    conn: &rusqlite::Connection,
    heads: &mut HashMap<String, Head>,
    branch: &str,
) -> Result<(), StoreError> {
    let Some(latest) = db::latest_rev(conn, branch)? else {
        heads.remove(branch);
        return Ok(());
    };
    if let Some(head) = heads.get_mut(branch) {
        if head.rev == latest {
            return Ok(());
        }
        if head.rev < latest {
            for (seq, bytes) in db::record_rows(conn, branch, head.rev)? {
                head.project = db::replay_row(&head.project, seq, &bytes)?.project;
                head.rev = seq;
            }
            return Ok(());
        }
    }
    let (mut rev, mut project) =
        db::latest_snapshot(conn, branch)?.ok_or_else(|| branch_not_found(branch))?;
    for (seq, bytes) in db::record_rows(conn, branch, rev)? {
        project = db::replay_row(&project, seq, &bytes)?.project;
        rev = seq;
    }
    heads.insert(branch.to_string(), Head { rev, project });
    Ok(())
}

fn branch_not_found(branch: &str) -> CoreError {
    CoreError::new(
        "BRANCH_NOT_FOUND",
        format!("branch {branch:?} does not exist"),
    )
    .with_details(json!({ "branch": branch }))
    .with_fix(format!("use branch {MAIN:?}"))
}
