//! Database versioning (sub-project 4 spec §3.1).
//!
//! - The header carries `application_id` = [`APPLICATION_ID`] and `user_version` = the schema
//!   version; `meta` carries `formatVersion` and `minReaderVersion`.
//! - Version 1 is sub-project 4A ([`m0001_records`]); version 2 adds 4B's derived history
//!   tables ([`m0002_history`]); version 3 is 4C's version bump only ([`m0003_version`]);
//!   version 4 adds 5E's derived `drops` index ([`m0004_drops`]); version 5 is the
//!   stable-keyframe-ids bump (model v3, [`m0005_keyframe_ids`]); version 6 is the graphic
//!   `sourceIn` and fade-offsets bump (model v4, [`m0006_fade_offsets`]); version 7 is the
//!   `VideoInfo` display and `asset.reprobe` bump (model v5, [`m0007_video_display`]).
//! - The sub-project 2 layout predates both: `application_id` 0, `user_version` 0, a `meta`
//!   table and a `txs.json` column. It is version 0.
//! - [`check`] reads all of this through a read-only connection, so a database this code
//!   refuses (`DB_TOO_NEW`, `NOT_A_PROJECT_DB`) is never written to.
//! - [`migrate`] backs the database up once (`VACUUM INTO`, [`crate::backup`]), then runs each
//!   step in its own `IMMEDIATE` transaction that also sets `user_version` to the step's target.
//!   A failed step rolls back whole; earlier steps stay committed.

mod m0001_records;
mod m0002_history;
mod m0003_version;
mod m0004_drops;
mod m0005_keyframe_ids;
mod m0006_fade_offsets;
mod m0007_video_display;

use std::path::{Path, PathBuf};

use fin_core::CoreError;
use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde_json::json;

use crate::{StoreError, backup};

/// `PRAGMA application_id` of a Finisher project database: "FIN1".
pub const APPLICATION_ID: i32 = 0x4649_4E31;
/// The database schema version this code writes (`PRAGMA user_version`).
pub const DB_VERSION: u32 = 7;
/// The oldest code (by `DB_VERSION`) that can read what this code writes.
pub(crate) const MIN_READER_VERSION: u32 = 7;

/// One migration: the database at `from` → `from + 1`. `run` must not touch `user_version`.
struct Step {
    from: u32,
    run: fn(&Transaction<'_>) -> Result<(), StoreError>,
}

const STEPS: &[Step] = &[
    Step {
        from: 0,
        run: m0001_records::run,
    },
    Step {
        from: 1,
        run: m0002_history::run,
    },
    Step {
        from: 2,
        run: m0003_version::run,
    },
    Step {
        from: 3,
        run: m0004_drops::run,
    },
    Step {
        from: 4,
        run: m0005_keyframe_ids::run,
    },
    Step {
        from: 5,
        run: m0006_fade_offsets::run,
    },
    Step {
        from: 6,
        run: m0007_video_display::run,
    },
];

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static FAIL_NEXT_STEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test hook: the next migration step on this thread fails after its work, before its commit.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn fail_next_migration_step() {
    FAIL_NEXT_STEP.with(|f| f.set(true));
}

/// The version of the database at `path`, read without writing to it. Refuses with
/// `NOT_A_PROJECT_DB` or `DB_TOO_NEW`.
///
/// Without a `-wal` or `-journal` beside it, the main file holds the whole database, so it is
/// opened `immutable=1`: no locks, no `-wal`/`-shm` created. Otherwise (another connection is
/// live, or a crash left a journal), or if the URI open fails, it is opened plainly read-only,
/// which reads through the WAL but may create `-shm`.
pub(crate) fn check(path: &Path) -> Result<u32, StoreError> {
    let side = |ext: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(ext);
        PathBuf::from(name).exists()
    };
    if !side("-wal")
        && !side("-journal")
        && let Some(uri) = immutable_uri(path)
    {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        match Connection::open_with_flags(&uri, flags)
            .map_err(|e| classify(e, path))
            .and_then(|conn| version(&conn, path))
        {
            Err(StoreError::Db(_)) => {} // fall back to a plain read-only open
            other => return other,
        }
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| classify(e, path))?;
    // Another open may be switching the database to WAL right now.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    version(&conn, path)
}

/// A read-only connection that writes nothing beside the file: with `immutable=1` when there
/// is no `-wal` or `-journal` to read (a plain read-only open of a WAL database creates
/// `-wal`/`-shm`), else a plain read-only open.
pub(crate) fn open_inspect(path: &Path) -> rusqlite::Result<Connection> {
    let side = |ext: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(ext);
        PathBuf::from(name).exists()
    };
    if !side("-wal")
        && !side("-journal")
        && let Some(uri) = immutable_uri(path)
    {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if let Ok(c) = Connection::open_with_flags(&uri, flags) {
            return Ok(c);
        }
    }
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(c)
}

/// `file:` URI for the absolute `path` with `immutable=1`, every byte outside `[A-Za-z0-9-._~/:]`
/// percent-encoded; Windows separators become `/` (`C:/x` → `file:///C:/x`, UNC `//h/s` →
/// `file:////h/s`). `None` for a path that is not UTF-8.
fn immutable_uri(path: &Path) -> Option<String> {
    let path = std::path::absolute(path).ok()?;
    let text = path.to_str()?;
    let text = if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.to_string()
    };
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri.push_str("?immutable=1");
    Some(uri)
}

/// Brings the database to [`DB_VERSION`]. Before the first step it is backed up to
/// `<project_data>/backups/pre-migrate-<from>-<to>-<ts>.fdb` (the last 3 are kept).
pub(crate) fn migrate(conn: &mut Connection, project_data: &Path) -> Result<(), StoreError> {
    let path = conn
        .path()
        .map_or_else(|| project_data.join("project.fdb"), PathBuf::from);
    let from = version(conn, &path)?;
    if from == DB_VERSION {
        return Ok(());
    }
    backup::pre_migration(conn, project_data, from, DB_VERSION)?;
    for step in STEPS.iter().filter(|s| s.from >= from) {
        let txn = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Another process may have migrated since `from` was read.
        let now = version(&txn, &path)?;
        if now > step.from {
            continue;
        }
        (step.run)(&txn)?;
        txn.pragma_update(None, "user_version", step.from + 1)?;
        #[cfg(any(test, feature = "test-hooks"))]
        if FAIL_NEXT_STEP.with(|f| f.replace(false)) {
            return Err(CoreError::new(
                "TEST_FAULT",
                format!("injected failure in migration step {}", step.from + 1),
            )
            .into());
        }
        txn.commit()?;
    }
    Ok(())
}

/// The schema version of an open database (see the module docs).
fn version(conn: &Connection, path: &Path) -> Result<u32, StoreError> {
    let read = |sql: &str| -> Result<i64, StoreError> {
        conn.query_row(sql, [], |r| r.get(0))
            .map_err(|e| classify(e, path))
    };
    let app_id = read("PRAGMA application_id")?;
    let user_version = read("PRAGMA user_version")?;
    let has_meta =
        read("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'")? > 0;
    let version = if app_id == i64::from(APPLICATION_ID) {
        u32::try_from(user_version).unwrap_or(u32::MAX)
    } else if app_id == 0
        && user_version == 0
        && has_meta
        && read("SELECT COUNT(*) FROM pragma_table_info('txs') WHERE name = 'json'")? > 0
    {
        0
    } else {
        return Err(not_a_project_db(
            path,
            json!({ "applicationId": app_id, "userVersion": user_version }),
        ));
    };
    if version > DB_VERSION {
        return Err(too_new(
            path,
            json!({ "dbVersion": version, "supported": DB_VERSION }),
        ));
    }
    if has_meta {
        let min_reader: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'minReaderVersion'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(min) = min_reader {
            let n: u64 = min
                .parse()
                .map_err(|_| not_a_project_db(path, json!({ "minReaderVersion": min })))?;
            if n > u64::from(DB_VERSION) {
                return Err(too_new(
                    path,
                    json!({ "dbVersion": version, "minReaderVersion": n, "supported": DB_VERSION }),
                ));
            }
        }
    }
    Ok(version)
}

/// An SQLite error reading the header: "not a database" is `NOT_A_PROJECT_DB`.
fn classify(e: rusqlite::Error, path: &Path) -> StoreError {
    match e.sqlite_error_code() {
        Some(ErrorCode::NotADatabase) => not_a_project_db(path, json!({ "reason": e.to_string() })),
        _ => e.into(),
    }
}

fn not_a_project_db(path: &Path, extra: serde_json::Value) -> StoreError {
    let mut details = json!({ "path": path.to_string_lossy() });
    merge(&mut details, extra);
    CoreError::new(
        "NOT_A_PROJECT_DB",
        format!("{} is not a Finisher project database", path.display()),
    )
    .with_details(details)
    .with_fix("restore project.fdb from a backup in the project's backups folder")
    .into()
}

fn too_new(path: &Path, extra: serde_json::Value) -> StoreError {
    let mut details = json!({ "path": path.to_string_lossy() });
    merge(&mut details, extra);
    CoreError::new(
        "DB_TOO_NEW",
        format!("{} was written by a newer Finisher", path.display()),
    )
    .with_details(details)
    .with_fix("update Finisher to open this project")
    .into()
}

fn merge(into: &mut serde_json::Value, extra: serde_json::Value) {
    if let (Some(a), serde_json::Value::Object(b)) = (into.as_object_mut(), extra) {
        a.extend(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `immutable=1` URI opens the database itself for every spelling of its path (spec
    /// §3.13 item 13): characters that need encoding, and on Windows the verbatim `\\?\` form
    /// `canonicalize` returns and a UNC path (the admin share of the same file, where enabled).
    /// A URI that failed would only fall back to a plain read-only open, silently.
    #[test]
    fn immutable_uris_open_every_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a b%c#d");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("p.fdb");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (42);")
            .unwrap();
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut spellings = vec![path.clone(), std::fs::canonicalize(&path).unwrap()];
        #[cfg(windows)]
        {
            let abs = std::path::absolute(&path).unwrap();
            let s = abs.to_str().unwrap();
            let unc = PathBuf::from(format!(r"\\localhost\{}${}", &s[..1], &s[2..]));
            if unc.exists() {
                spellings.push(unc);
            } else {
                eprintln!("no admin share: UNC spelling not checked");
            }
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
        for p in spellings {
            let uri = immutable_uri(&p).unwrap();
            let c = Connection::open_with_flags(&uri, flags)
                .unwrap_or_else(|e| panic!("{} as {uri}: {e}", p.display()));
            let x: i64 = c
                .query_row("SELECT x FROM t", [], |r| r.get(0))
                .unwrap_or_else(|e| panic!("{} as {uri}: {e}", p.display()));
            assert_eq!(x, 42, "{uri}");
        }
    }
}
