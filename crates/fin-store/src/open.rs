//! `Store::create` and `Store::open` (plan Task 11).
//!
//! # Open, in order
//!
//! 1. Read `finisher.project.json` (retrying a briefly missing file, [`read_project_file`]).
//! 2. No database: create it from the file's [`Genesis`](crate::Genesis) at rev 0
//!    ([`rebuild::fresh_db`]); step 7 then imports the whole folder. Without a genesis:
//!    `PROJECT_DB_MISSING`.
//! 3. Check the database read-only (`DB_TOO_NEW`, `NOT_A_PROJECT_DB`, `PROJECT_DB_MISMATCH`),
//!    then refuse a copy of the project folder (`PROJECT_COPY`, [`copies`]).
//! 4. Migrate; record `lastOpenedBy` and `projectDir`.
//! 5. Detect the storage kind (`storageOverride` wins).
//! 6. Try to acquire the lease ([`Contender::try_acquire`](crate::lease::Contender)); a
//!    follower keeps its contender for `tick`.
//! 7. Import ([`Store::import`]). The read-only state (fork, conflict copy, newer format, lease
//!    held elsewhere, not caught up) is therefore in place before `open` returns: no commit
//!    can slip in after a restart.
//! 8. Holding the lease and the file has no genesis (a sub-project 2 project): backfill it
//!    from the database's rev-0 snapshot, with the file's version fields and `storage`.
//!
//! Unexported records (a crash before their export) are not written by `open` itself: the
//! first `tick` exports them after the export delay, and `close` flushes. That keeps `open`
//! free of segment writes, which can take seconds on a slow share.
//!
//! `open` never blocks on the ordering rule's directory-cache wait: when it applies, the store
//! is `CATCHING_UP` until a `tick` after the wait (at open it practically never does: a
//! single acquisition attempt only takes a free lease or this process's own stopped one).
//! The folder I/O is a lease probe, the record read, a claim create, one scan and, while
//! holding, one lease check: tens of milliseconds on a local disk.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fin_core::{CoreError, Rational, new_project};
use serde_json::json;

use crate::lease::{MonoClock, Params, SystemClock};
use crate::project_file::{self, Genesis, ProjectFile, read_project_file};
use crate::storage_kind::{self, StorageKind};
use crate::{
    Head, MAIN, Store, StoreError, copies, db, db_path, export, import, migrations, paths,
    project_file as pf, rebuild, refresh, server_id, status, tick, unwritable,
};

/// How a store is opened. [`OpenOptions::default`] is production: the system's monotonic
/// clock and the spec's lease timing.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// The monotonic clock of the lease and of the ordering rule's wait (tests: a
    /// [`FakeClock`](crate::lease::FakeClock)).
    pub clock: Option<Arc<dyn MonoClock>>,
    /// How often the heartbeat thread looks at the clock ([`Params::poll`]).
    pub poll: Option<Duration>,
}

impl OpenOptions {
    pub(crate) fn params(&self, kind: StorageKind) -> Params {
        let mut p = Params::for_kind(kind);
        p.clock = self
            .clock
            .clone()
            .unwrap_or_else(|| Arc::new(SystemClock) as Arc<dyn MonoClock>);
        if let Some(poll) = self.poll {
            p.poll = poll;
        }
        p
    }
}

impl Store {
    /// Creates a project: a new lowercase-ULID id, the database at
    /// `<server_data>/projects/<id>/project.fdb` with `main` at rev 0, `.finisher/`, then
    /// `<project_dir>/finisher.project.json` with the project's [`Genesis`] (written via an
    /// fsync'd temp file and rename), and acquires the lease. Fails with `PROJECT_EXISTS`
    /// when the project file is already there.
    pub fn create(
        server_data: &Path,
        project_dir: &Path,
        name: &str,
        rate: Rational,
    ) -> Result<Store, StoreError> {
        Store::create_with(server_data, project_dir, name, rate, OpenOptions::default())
    }

    /// [`Store::create`] with [`OpenOptions`].
    pub fn create_with(
        server_data: &Path,
        project_dir: &Path,
        name: &str,
        rate: Rational,
        opts: OpenOptions,
    ) -> Result<Store, StoreError> {
        let file_path = paths::project_file(project_dir);
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
        let dot = project_dir.join(".finisher");
        fs::create_dir_all(&dot).map_err(unwritable(&dot))?;
        let server_id = server_id(server_data)?;
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
        copies::record(&conn, project_dir)?;
        let (kind, warning) = storage_kind::resolve(project_dir, None);
        let genesis = Genesis {
            name: name.to_string(),
            rate,
        };
        pf::write(
            project_dir,
            &ProjectFile::new(&id, genesis, kind.to_string()),
        )?;
        let heads = HashMap::from([(
            MAIN.to_string(),
            Head {
                rev: 0,
                tip: None,
                project,
            },
        )]);
        let mut store = Store::new(
            conn,
            id,
            db_path,
            project_dir,
            server_id,
            heads,
            (kind, warning),
            &opts,
        );
        store.start(None)?;
        Ok(store)
    }

    /// Opens the project whose `finisher.project.json` is in `project_dir` (module docs for
    /// the order of work).
    ///
    /// The database is checked read-only first: one written by a newer Finisher is
    /// `DB_TOO_NEW`, anything else that is not a project database `NOT_A_PROJECT_DB`, and
    /// neither is modified. An older one is backed up and migrated to
    /// [`DB_VERSION`](crate::DB_VERSION).
    pub fn open(server_data: &Path, project_dir: &Path) -> Result<Store, StoreError> {
        Store::open_with(server_data, project_dir, OpenOptions::default())
    }

    /// [`Store::open`] with [`OpenOptions`].
    pub fn open_with(
        server_data: &Path,
        project_dir: &Path,
        opts: OpenOptions,
    ) -> Result<Store, StoreError> {
        let file = read_project_file(project_dir)?;
        let db_path = db_path(server_data, &file.id);
        if !db_path.is_file() {
            let genesis = rebuild::genesis(&file, &db_path)?;
            rebuild::fresh_db(&db_path, &file.id, &genesis)?;
        }
        migrations::check(&db_path)?;
        let server_id = server_id(server_data)?;
        let mut conn = db::connect(&db_path, false)?;
        let stored = db::meta(&conn, "projectId")?;
        if stored.as_deref() != Some(file.id.as_str()) {
            return Err(CoreError::new("PROJECT_DB_MISMATCH", format!("{} belongs to another project", db_path.display()))
                .with_details(json!({ "projectId": file.id, "dbProjectId": stored, "path": db_path.to_string_lossy() }))
                .into());
        }
        copies::check(&conn, project_dir, &file.id)?;
        migrations::migrate(&mut conn, db_path.parent().unwrap_or(server_data))?;
        db::set_meta(&conn, "lastOpenedBy", &db::written_by())?;
        copies::record(&conn, project_dir)?;
        let mut heads = HashMap::new();
        for branch in db::branches(&conn)? {
            refresh(&conn, &mut heads, &branch)?;
        }
        let kind = storage_kind::resolve(project_dir, file.storage_override.as_ref());
        let mut store = Store::new(
            conn,
            file.id.clone(),
            db_path,
            project_dir,
            server_id,
            heads,
            kind,
            &opts,
        );
        store.file_newer = file.is_newer();
        store.start(Some(file))?;
        Ok(store)
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        conn: rusqlite::Connection,
        project_id: String,
        db_path: PathBuf,
        project_dir: &Path,
        server_id: String,
        heads: HashMap<String, Head>,
        (kind, storage_warning): (StorageKind, Option<String>),
        opts: &OpenOptions,
    ) -> Store {
        let ctl = tick::LeaseCtl::new(opts.params(kind));
        Store {
            conn,
            project_id,
            db_path,
            project_dir: project_dir.to_path_buf(),
            server_id,
            export: export::ExportState::default(),
            import: import::ImportState::with_kind(kind),
            storage_warning,
            ctl,
            follow: status::Follow::default(),
            file_newer: false,
            heads: RefCell::new(heads),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_after_apply: false,
        }
    }

    /// Steps 6–8 of the module docs; `file` is the project file read at open (`None` at
    /// create, whose file is complete).
    fn start(&mut self, file: Option<ProjectFile>) -> Result<(), StoreError> {
        let dot = self.project_dir.join(".finisher");
        // A folder from before sub-project 4 has none yet. Failing to make it (a read-only
        // share) only means the lease cannot be acquired: the store follows.
        let _ = fs::create_dir_all(&dot);
        self.try_acquire();
        self.import_pass()?;
        if let Some(file) = file
            && file.genesis.is_none()
            && !file.is_newer()
            && self.export.lease.quick_check().is_ok()
        {
            self.backfill_genesis(file)?;
        }
        Ok(())
    }

    /// Writes the genesis of a sub-project 2 project into its file (module docs, step 8).
    /// A failure is not fatal: the next open by the holder tries again.
    fn backfill_genesis(&mut self, mut file: ProjectFile) -> Result<(), StoreError> {
        let Some(rev0) = db::snapshot_at(&self.conn, MAIN, 0)? else {
            return Ok(());
        };
        let Some(genesis) = project_file::genesis_from(&self.project_id, &rev0) else {
            return Ok(());
        };
        file.genesis = Some(genesis);
        file.format_version = Some(project_file::FORMAT_VERSION);
        file.min_reader_version = Some(project_file::MIN_READER_VERSION);
        file.storage = Some(self.storage_kind().to_string());
        let _ = pf::write(&self.project_dir, &file);
        Ok(())
    }
}
