//! Building a project database from the project folder (sub-project 4 spec §3.8).
//!
//! Plan Task 11 needs the minimal form: a server with no database for a project whose file
//! carries its [`Genesis`] creates the database at rev 0 from it (`new_project`), and the
//! open that follows imports every segment of the folder from rev 0 ([`Store::import`]).
//! Each import pass is one transaction, so an interrupted rebuild leaves a valid database at
//! a prefix of the folder's history, and the next open continues it.
//!
//! Task 12 builds on this: `quick_check` failure, seeding from a folder snapshot whose
//! `txHash` is on the resolved chain, verification, salvage of the old database and
//! `fin project rebuild`.
//!
//! [`Store::import`]: crate::Store::import

use std::path::{Path, PathBuf};

use fin_core::{CoreError, new_project};
use rusqlite::Connection;
use serde_json::json;

use crate::project_file::{Genesis, ProjectFile};
use crate::{MAIN, StoreError, db};

/// The genesis of `file`, or `PROJECT_DB_MISSING` when the file has none (a sub-project 2
/// project whose database was never opened by sub-project 4).
pub(crate) fn genesis(file: &ProjectFile, db_path: &Path) -> Result<Genesis, StoreError> {
    file.genesis.clone().ok_or_else(|| {
        CoreError::new(
            "PROJECT_DB_MISSING",
            format!("the database for project {} is missing", file.id),
        )
        .with_details(json!({ "projectId": file.id, "path": db_path.to_string_lossy() }))
        .with_fix("restore project.fdb into the server data dir: this project's folder does not record how to rebuild it (open it once with the server that has its database)")
        .into()
    })
}

/// Creates the database at `db_path` holding the rev-0 project of `genesis` on `main`: built
/// as `project.fdb.rebuild-<ulid>` beside it, closed, then moved into place without replacing
/// anything (`fsio::rename_noreplace`), so a crash never leaves a half-made `project.fdb` and
/// two opens racing to rebuild share the first one's database.
pub(crate) fn fresh_db(db_path: &Path, id: &str, genesis: &Genesis) -> Result<(), StoreError> {
    let project = new_project(id.to_string(), genesis.name.clone(), genesis.rate)?;
    let dir = db_path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(StoreError::io(dir))?;
    let ulid = ulid::Ulid::new().to_string().to_ascii_lowercase();
    let tmp = dir.join(format!("project.fdb.rebuild-{ulid}"));
    let built = (|| {
        let mut conn: Connection = db::connect(&tmp, true)?;
        db::init(&mut conn, &project, MAIN)?;
        // Everything into the main file; closing the last connection removes `-wal` and
        // `-shm`. The file stays in WAL mode, so later opens need not switch it (a switch takes
        // an exclusive lock, which two racing opens would fight over).
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        drop(conn);
        // Under `rebuild.lock`, so no other open of this server data is between its own
        // check and link: never over a database another open made meanwhile (a replaced file
        // under a live connection loses its writes; the first one in place wins and is used),
        // and stale `-wal`/`-shm` files of a deleted database go first (SQLite would replay
        // that WAL onto the new file).
        let lock_path = dir.join("rebuild.lock");
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(StoreError::io(&lock_path))?;
        lock.lock().map_err(StoreError::io(&lock_path))?;
        if db_path.exists() {
            let _ = std::fs::remove_file(&tmp);
            return Ok(());
        }
        for ext in ["-wal", "-shm"] {
            let side = PathBuf::from(format!("{}{ext}", db_path.display()));
            match std::fs::remove_file(&side) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(StoreError::io(&side)(e)),
            }
        }
        match crate::fsio::rename_noreplace(&tmp, db_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = std::fs::remove_file(&tmp);
                return Ok(());
            }
            Err(e) => return Err(StoreError::io(db_path)(e)),
        }
        crate::fsio::sync_dir(dir).map_err(StoreError::io(dir))
    })();
    if built.is_err() {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", tmp.display()));
        }
    }
    built
}
