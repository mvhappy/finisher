//! `Store::open` (plan Task 11); `Store::create` is in `crate::create`.
//!
//! # Open, in order
//!
//! 1. Read `finisher.project.json` (retrying a briefly missing file, [`read_project_file`]).
//! 2. No database: rebuild it from the folder ([`rebuild`]: the file's
//!    [`Genesis`](crate::Genesis) at rev 0, then the whole folder, seeded from a snapshot on
//!    the chain when there is one). Without a genesis: `PROJECT_DB_MISSING`.
//! 3. Take the shared `db.lock` (`crate::dblock`), then check the database read-only:
//!    refused when it is someone else's or newer (`NOT_A_PROJECT_DB`, `DB_TOO_NEW`); a bad
//!    header, an unreadable schema or `meta` of ours, or a failing `PRAGMA quick_check`
//!    means damage (`rebuild::damaged`): a damaged database is rebuilt from the folder, its readable unexported
//!    records salvaged into quarantine, and kept as `project.fdb.corrupt-<ts>` ([`rebuild`];
//!    status warns `DB_REBUILT`). Then `PROJECT_DB_MISMATCH`, and refuse a copy of the project folder (`PROJECT_COPY`) or a folder whose original
//!    cannot be reached to tell (`PROJECT_ORIGINAL_UNREACHABLE`; [`copies`]).
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
//! single acquisition attempt only takes a free lease, this process's own stopped one, or
//! this server's own record left by a process that stopped: the server-data lock
//! `<server-data>/projects/<id>/lease.lock` proves no process of this server data still holds
//! it, so a restart is writable at once, at a fresh epoch).
//! The folder I/O is a lease probe, the record read, a claim create, one scan and, while
//! holding, one lease check: tens of milliseconds on a local disk.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fin_core::CoreError;
use fin_core::layout::TextMeasurer;
use serde_json::json;

use crate::lease::{MonoClock, Params, ServerLock, SystemClock};
use crate::project_file::{self, ProjectFile, read_project_file};
use crate::storage_kind::{self, StorageKind};
use crate::{
    Head, MAIN, Store, StoreError, copies, db, db_path, export, import, migrations,
    project_file as pf, rebuild, refresh, server_identity, status, tick,
};

/// How a store is opened. [`OpenOptions::default`] is production: the system's monotonic
/// clock and the spec's lease timing.
#[derive(Clone, Default)]
pub struct OpenOptions {
    /// The monotonic clock of the lease and of the ordering rule's wait (tests: a
    /// [`FakeClock`](crate::lease::FakeClock)).
    pub clock: Option<Arc<dyn MonoClock>>,
    /// How often the heartbeat thread looks at the clock ([`Params::poll`]).
    pub poll: Option<Duration>,
    /// The user says this folder is where the project now lives: the database is moved to
    /// it even though its recorded folder cannot be reached (`PROJECT_ORIGINAL_UNREACHABLE`).
    /// Never set automatically.
    pub relocate: bool,
    /// What edits, history writes and recovery check caption overflow with (plan 4C Task 20;
    /// [`Store::set_measurer`] changes it later). `None` measures nothing. Replay (import,
    /// rebuild) never measures.
    pub measurer: Option<Arc<dyn TextMeasurer>>,
    /// The project the caller checked this folder holds (`finisherd`'s chain read
    /// `finisher.project.json` before authorising): another id is `PROJECT_NOT_FOUND {reason:
    /// changed}` before anything is rebuilt, locked or leased (review of 5A Task 6, finding 6).
    pub expect_id: Option<String>,
    /// Accept project data that is not on a known local filesystem (review #4,
    /// [`crate::locality`]). `None`: [`locality::ALLOW_NONLOCAL_ENV`](crate::locality::ALLOW_NONLOCAL_ENV) decides.
    pub allow_nonlocal: Option<bool>,
}

impl std::fmt::Debug for OpenOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenOptions")
            .field("clock", &self.clock)
            .field("poll", &self.poll)
            .field("relocate", &self.relocate)
            .field("measurer", &self.measurer.as_ref().map(|m| m.fingerprint()))
            .field("expect_id", &self.expect_id)
            .field("allow_nonlocal", &self.allow_nonlocal)
            .finish()
    }
}

impl OpenOptions {
    /// Refuses `data_dir` (a project's data folder) unless it is on a known local filesystem.
    pub(crate) fn check_local(&self, data_dir: &Path) -> Result<(), StoreError> {
        let allow = self
            .allow_nonlocal
            .unwrap_or_else(crate::locality::allowed_by_env);
        crate::locality::check(data_dir, "project data", allow)
    }

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
        if let Some(want) = &opts.expect_id
            && *want != file.id
        {
            return Err(CoreError::new(
                "PROJECT_NOT_FOUND",
                format!("the folder {} no longer holds project {want}", project_dir.display()),
            )
            .with_details(json!({ "dir": project_dir.to_string_lossy(), "projectId": want, "reason": "changed" }))
            .with_fix("open the project by its folder again")
            .into());
        }
        let db_path = db_path(server_data, &file.id);
        opts.check_local(db_path.parent().unwrap_or(server_data))?;
        let identity = server_identity(server_data)?;
        // The folder names this id: a create mark left by a crash after its commit point goes.
        crate::orphans::unmark(db_path.parent().unwrap_or(server_data));
        crate::orphans::sweep_once(server_data);
        let kind = storage_kind::resolve(project_dir, file.storage_override.as_ref());
        let plan = rebuild::Plan {
            db_path: &db_path,
            project_dir,
            file: &file,
            identity: &identity,
            kind: kind.0,
            opts: &opts,
        };
        let data_dir = db_path.parent().unwrap_or(server_data);
        let mut rebuilt = None;
        if !db_path.is_file() {
            rebuild::genesis(&file, &db_path)?;
            rebuilt = rebuild::rebuild(&plan, rebuild::Why::Missing, None)?;
        }
        let mut db_lock = crate::dblock::shared(data_dir)?;
        if let Some(problem) = rebuild::damaged(&db_path, &file.id)? {
            drop(db_lock);
            rebuilt = rebuild::rebuild(&plan, rebuild::Why::Corrupt(problem), None)?.or(rebuilt);
            db_lock = crate::dblock::shared(data_dir)?;
            migrations::check(&db_path)?;
        }
        let mut conn = db::connect(&db_path, false)?;
        let stored = db::meta(&conn, "projectId")?;
        if stored.as_deref() != Some(file.id.as_str()) {
            return Err(CoreError::new("PROJECT_DB_MISMATCH", format!("{} belongs to another project", db_path.display()))
                .with_details(json!({ "projectId": file.id, "dbProjectId": stored, "path": db_path.to_string_lossy() }))
                .into());
        }
        copies::check(&conn, project_dir, &file.id, opts.relocate)?;
        migrations::migrate(&mut conn, db_path.parent().unwrap_or(server_data))?;
        db::set_meta(&conn, "lastOpenedBy", &db::written_by())?;
        copies::record(&conn, project_dir)?;
        let mut heads = HashMap::new();
        for branch in db::branches(&conn)? {
            refresh(&conn, &mut heads, &branch)?;
        }
        let mut store = Store::new(
            conn,
            file.id.clone(),
            db_path,
            project_dir,
            identity,
            heads,
            kind,
            &opts,
        );
        store.file_newer = file.is_newer();
        store.collect = crate::collect::policy_of(&file);
        store.rebuilt = rebuilt;
        store.db_lock = Some(db_lock);
        store.start(Some(file))?;
        Ok(store)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        conn: rusqlite::Connection,
        project_id: String,
        db_path: PathBuf,
        project_dir: &Path,
        identity: crate::ServerIdentity,
        heads: HashMap<String, Head>,
        (kind, storage_warning): (StorageKind, Option<String>),
        opts: &OpenOptions,
    ) -> Store {
        let mut ctl = tick::LeaseCtl::new(opts.params(kind));
        // `<server-data>/projects/<id>/lease.lock`; the restart shortcut only where an OS lock
        // means something (local storage).
        let data_dir = db_path.parent().unwrap_or(Path::new("."));
        ctl.server_lock = Some(ServerLock {
            path: data_dir.join("lease.lock"),
            shortcut: identity.shortcut_ok
                && storage_kind::detect(data_dir, None) == StorageKind::Local
                && storage_kind::local_disk::is_local_disk(data_dir),
        });
        Store {
            conn,
            project_id,
            db_path,
            project_dir: project_dir.to_path_buf(),
            server_id: identity.id,
            identity_note: identity.renewed,
            export: export::ExportState::default(),
            import: import::ImportState::with_kind(kind),
            storage_warning,
            ctl,
            follow: status::Follow::default(),
            file_newer: false,
            collect: crate::collect::CollectPolicy::Default,
            heads: RefCell::new(heads),
            rebuilt: None,
            daily: tick::DailyState::default(),
            due: tick::DueWork::default(),
            db_lock: None,
            damage: RefCell::new(crate::verify::DamageSeen::default()),
            quarantine_seen: RefCell::new(None),
            compact: crate::compact::CompactState::default(),
            measurer: opts.measurer.clone(),
            write_outcome: None,
            guard: crate::WriteGuard::default(),
            when_lost: fin_core::rebase::placement::WhenLost::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_after_apply: false,
        }
    }

    /// Steps 6–8 of the module docs; `file` is the project file read at open (`None` at
    /// create, whose file is complete).
    pub(crate) fn start(&mut self, file: Option<ProjectFile>) -> Result<(), StoreError> {
        let dot = self.project_dir.join(".finisher");
        // A folder from before sub-project 4 has none yet. Failing to make it (a read-only
        // share) only means the lease cannot be acquired: the store follows.
        let _ = fs::create_dir_all(&dot);
        crate::history::note_server_id(&self.conn, &self.server_id)?;
        crate::history::ensure_incarnation(&self.conn)?;
        crate::project_file::db_genesis_hash(&self.conn)?;
        self.try_acquire();
        if self.rebuilt.is_some() {
            self.queue_rebuilt();
        }
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
