//! `Store::create`: atomic and idempotent.
//!
//! # Create, in order
//!
//! 1. Validate before touching disk: the name (`INVALID_NAME` when empty or whitespace), the
//!    rate (`fin_core::new_project`) and the key.
//! 2. The folder already holds `finisher.project.json`: a retry of this create (the same
//!    `idem` key in that project's `meta.createKey`, and the same name and rate) opens and
//!    returns that project; the same key with another name or rate is `IDEM_KEY_REUSED`;
//!    anything else is `PROJECT_EXISTS`.
//! 3. Build the database as `project.fdb.creating-<ulid>` beside where it goes (a fresh id, so
//!    nothing else can look at it), checkpoint and close it, then rename it into place
//!    (never replacing).
//! 4. Write the project file **write-once** ([`fsio::write_once`]): a temp file renamed without
//!    replacing. That rename is the commit point. When it finds a file already there (another
//!    create won the race since step 2), this create's database is removed and step 2 runs
//!    against the winner. Any other failure removes the database too, and the folder when
//!    this create made it: a failed create leaves no orphan database and no half-made folder.
//! 5. Open the database and start the store as `open` does (`.finisher/`, the lease, the
//!    first import pass). `.finisher/` is made only now: a folder holding it makes `open`
//!    wait for a project file that is late to appear (Windows), so it must not exist
//!    without one.
//!
//! A retry (step 2) can land between another create's step 4 and its lease: it waits up to
//! [`CREATOR_LEASE_WAIT`] for the folder's `lease.json` before it opens, so it follows the
//! creator rather than taking the lease from under it.
//!
//! A crash between steps 3 and 4 leaves a database in server data that no project file names;
//! it is never opened, and costs only its space (a later sweep can remove it).

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use fin_core::{CoreError, Rational, new_project};
use serde_json::json;

use crate::open::OpenOptions;
use crate::project_file::{self as pf, Genesis, ProjectFile};
use crate::{
    Head, MAIN, PROJECT_FILE, Store, StoreError, copies, db, db_path, fsio, paths, server_identity,
    storage_kind, unwritable,
};

/// `meta` key holding the key a project was created with (carried across rebuilds).
pub(crate) const CREATE_KEY: &str = "createKey";

/// How long a retry waits for the creator's lease (module docs).
const CREATOR_LEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// `INVALID_NAME` for an empty or whitespace-only project name.
pub fn validate_name(name: &str) -> Result<(), CoreError> {
    if name.trim().is_empty() {
        return Err(CoreError::new("INVALID_NAME", "the project name is empty")
            .with_details(json!({ "field": "name", "input": name }))
            .with_fix("give the project a name with at least one visible character"));
    }
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static FAIL_MARKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static DELAY_START: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

/// Test hook: the next create on this thread pauses for `d` after writing its project file,
/// before it opens the database and takes the lease.
#[cfg(any(test, feature = "test-hooks"))]
pub fn delay_next_create_start_for_test(d: std::time::Duration) {
    DELAY_START.with(|c| c.set(Some(d)));
}

/// Test hook: the next create on this thread fails to write its project file, after its
/// database was moved into place.
#[cfg(any(test, feature = "test-hooks"))]
pub fn fail_next_create_marker_for_test() {
    FAIL_MARKER.with(|f| f.set(true));
}

fn write_marker(project_dir: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(any(test, feature = "test-hooks"))]
    if FAIL_MARKER.with(|f| f.replace(false)) {
        return Err(io::Error::other("injected project file failure"));
    }
    fsio::write_once(project_dir, PROJECT_FILE, bytes)
}

fn exists_error(project_dir: &Path) -> StoreError {
    let path = paths::project_file(project_dir);
    CoreError::new(
        "PROJECT_EXISTS",
        format!("{} already holds a project", project_dir.display()),
    )
    .with_details(json!({ "path": path.to_string_lossy() }))
    .with_fix("open the existing project, or choose another folder")
    .into()
}

/// The database and folder of a create that did not happen (step 4 of the module docs).
struct Undo<'a> {
    db_dir: &'a Path,
    /// The project folder, when this create made it.
    made_dir: Option<&'a Path>,
}

impl Undo<'_> {
    fn run(&self) {
        // A fresh id's directory: everything in it is this create's.
        let _ = fs::remove_dir_all(self.db_dir);
        if let Some(dir) = self.made_dir {
            let _ = fs::remove_dir(dir); // only if still empty
        }
    }
}

impl Store {
    /// Creates a project (module docs): a new lowercase-ULID id, the database at
    /// `<server_data>/projects/<id>/project.fdb` with `main` at rev 0, then
    /// `<project_dir>/finisher.project.json` with the project's [`Genesis`], written once,
    /// and `.finisher/`; acquires the lease. `idem`: a key that makes a retry of this create
    /// return the project it made (stored in `meta.createKey`).
    ///
    /// Errors: `INVALID_NAME`, `PROJECT_EXISTS`, `IDEM_KEY_REUSED` (the key created this
    /// folder's project with another name or rate), `PROJECT_DIR_UNWRITABLE`, a rate error.
    pub fn create(
        server_data: &Path,
        project_dir: &Path,
        name: &str,
        rate: Rational,
        idem: Option<&str>,
    ) -> Result<Store, StoreError> {
        Store::create_with(
            server_data,
            project_dir,
            name,
            rate,
            idem,
            OpenOptions::default(),
        )
    }

    /// [`Store::create`] with [`OpenOptions`].
    pub fn create_with(
        server_data: &Path,
        project_dir: &Path,
        name: &str,
        rate: Rational,
        idem: Option<&str>,
        opts: OpenOptions,
    ) -> Result<Store, StoreError> {
        validate_name(name)?;
        if idem.is_some_and(|k| k.trim().is_empty()) {
            return Err(CoreError::new("INVALID_OP", "the create key is empty")
                .with_details(json!({ "field": "idemKey" }))
                .with_fix("pass a non-empty key, or none")
                .into());
        }
        let genesis = Genesis {
            name: name.to_string(),
            rate,
        };
        let id = ulid::Ulid::new().to_string().to_ascii_lowercase();
        let project = new_project(id.clone(), name.to_string(), rate)?;
        // Before anything is written: not into the project folder either (review #4).
        opts.check_local(db_path(server_data, &id).parent().unwrap_or(server_data))?;
        if fs::symlink_metadata(paths::project_file(project_dir)).is_ok() {
            return existing(server_data, project_dir, &genesis, idem, opts);
        }
        let made_dir = !project_dir.exists();
        fs::create_dir_all(project_dir).map_err(unwritable(project_dir))?;
        let identity = server_identity(server_data)?;
        let db_path = db_path(server_data, &id);
        let db_dir = db_path.parent().unwrap_or(server_data).to_path_buf();
        let undo = Undo {
            db_dir: &db_dir,
            made_dir: made_dir.then_some(project_dir),
        };
        let (kind, warning) = storage_kind::resolve(project_dir, None);
        let built = (|| {
            fs::create_dir_all(&db_dir).map_err(StoreError::io(&db_dir))?;
            build(&db_path, &project, project_dir, idem)
        })();
        if let Err(e) = built {
            undo.run();
            return Err(e);
        }
        let file = ProjectFile::new(&id, genesis.clone(), kind.to_string());
        let mut bytes = serde_json::to_vec_pretty(&file)?;
        bytes.push(b'\n');
        match write_marker(project_dir, &bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                undo.run();
                return existing(server_data, project_dir, &genesis, idem, opts);
            }
            // The rename may have happened with only the directory fsync failing: then the
            // project file is ours and the create stands.
            Err(_) if pf::read_project_file_once(project_dir).is_ok_and(|f| f.id == id) => {}
            Err(e) => {
                undo.run();
                return Err(unwritable(&paths::project_file(project_dir))(e));
            }
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(d) = DELAY_START.with(|c| c.take()) {
            std::thread::sleep(d);
        }
        let db_lock = crate::dblock::shared(&db_dir)?;
        let conn = db::connect(&db_path, false)?;
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
            identity,
            heads,
            (kind, warning),
            &opts,
        );
        store.db_lock = Some(db_lock);
        store.start(None)?;
        Ok(store)
    }
}

/// Step 3 of the module docs: the database, built aside and renamed to `db_path`.
fn build(
    db_path: &Path,
    project: &fin_core::Project,
    project_dir: &Path,
    idem: Option<&str>,
) -> Result<(), StoreError> {
    let dir = db_path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        "project.fdb.creating-{}",
        ulid::Ulid::new().to_string().to_ascii_lowercase()
    ));
    let mut conn = db::connect(&tmp, true)?;
    db::init(&mut conn, project, MAIN)?;
    copies::record(&conn, project_dir)?;
    if let Some(key) = idem {
        db::set_meta(&conn, CREATE_KEY, key)?;
    }
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    drop(conn);
    for ext in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{ext}", tmp.display()));
    }
    fsio::rename_noreplace(&tmp, db_path).map_err(StoreError::io(db_path))?;
    fsio::sync_dir(dir).map_err(StoreError::io(dir))
}

/// Step 2 of the module docs: the folder holds a project already.
fn existing(
    server_data: &Path,
    project_dir: &Path,
    genesis: &Genesis,
    idem: Option<&str>,
    opts: OpenOptions,
) -> Result<Store, StoreError> {
    let Some(key) = idem else {
        return Err(exists_error(project_dir));
    };
    let Ok(file) = pf::read_project_file_once(project_dir) else {
        return Err(exists_error(project_dir));
    };
    if create_key(&db_path(server_data, &file.id)).as_deref() != Some(key) {
        return Err(exists_error(project_dir));
    }
    if file.genesis.as_ref() != Some(genesis) {
        return Err(CoreError::new(
            "IDEM_KEY_REUSED",
            format!(
                "the create key {key:?} already created project {} with another name or rate",
                file.id
            ),
        )
        .with_details(json!({ "projectId": file.id, "idemKey": key, "genesis": file.genesis }))
        .with_fix("use a new key for a different project, or retry with the same name and rate")
        .into());
    }
    wait_for_creator_lease(project_dir);
    Store::open_with(server_data, project_dir, opts)
}

/// Until the folder holds `lease.json` (the creator took its lease), at most
/// [`CREATOR_LEASE_WAIT`]. Then the lease is the creator's, or the creator is gone and this
/// retry may take it.
fn wait_for_creator_lease(project_dir: &Path) {
    let lease = project_dir
        .join(".finisher")
        .join(crate::lease::record::FILE);
    let started = std::time::Instant::now();
    while !lease.exists() && started.elapsed() < CREATOR_LEASE_WAIT {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// `meta.createKey` of the database at `path`, read without writing to it.
fn create_key(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let conn = crate::migrations::open_inspect(path).ok()?;
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [CREATE_KEY], |r| {
        r.get(0)
    })
    .ok()
}
