//! The media cache's index (6B-1 plan Decision 10): `cache/media/index.db`, SQLite in WAL with
//! one writer (the cache thread). Tables:
//! - `entries {key PK, recipe, recipe_version, fingerprint, bytes, created_at, last_used_at,
//!   meta}`: `bytes` counts the files and `meta.json`; `meta` is the entry's `meta.json` as
//!   checked at commit or adoption, so a lookup reads no file;
//! - `entry_projects {key, project_id}`: the projects an entry serves (reach, spec §7.3);
//! - `pins {key, holder}`: holders `job:<id>` and `project:<id>`; a pinned entry is never
//!   evicted. Pins do not outlive a server run: [`Index::clear_pins`] at open, and the server
//!   pins again for what it opens.
//!
//! Rows of `entry_projects` and `pins` go with their entry (`ON DELETE CASCADE`). A damaged
//! database (`SQLITE_NOTADB`, `SQLITE_CORRUPT`) or one of another schema version is replaced by
//! an empty one: the adoption at start (`adopt.rs`) rebuilds the rows from the entries on disk.
//! Any other failure to open (busy, locked, permissions) is `CACHE_IO {reason: index}`, and the
//! file is kept.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use fin_core::CoreError;
use fin_media::cache::{EntryMeta, RecipeUsage};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

/// The index's schema version (`PRAGMA user_version`).
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE entries (
    key TEXT PRIMARY KEY,
    recipe TEXT NOT NULL,
    recipe_version INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    meta TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX entries_lru ON entries (last_used_at, key);
CREATE INDEX entries_fingerprint ON entries (fingerprint);
CREATE TABLE entry_projects (
    key TEXT NOT NULL REFERENCES entries (key) ON DELETE CASCADE,
    project_id TEXT NOT NULL,
    PRIMARY KEY (key, project_id)
) WITHOUT ROWID;
CREATE INDEX entry_projects_project ON entry_projects (project_id);
CREATE TABLE pins (
    key TEXT NOT NULL REFERENCES entries (key) ON DELETE CASCADE,
    holder TEXT NOT NULL,
    PRIMARY KEY (key, holder)
) WITHOUT ROWID;
CREATE INDEX pins_holder ON pins (holder);
";

/// An I/O failure of the cache (its index included): `CACHE_IO {reason, freeBytes?}`.
pub fn cache_io(reason: &str, message: impl Into<String>) -> CoreError {
    CoreError::new("CACHE_IO", message)
        .with_details(json!({ "reason": reason }))
        .with_fix(
            "check the volume that holds the server data (free space, permissions), then retry",
        )
}

fn db(e: rusqlite::Error) -> CoreError {
    cache_io("index", format!("the media cache index failed: {e}"))
}

/// One entry row.
#[derive(Debug, Clone)]
pub struct Row {
    pub key: String,
    pub bytes: u64,
}

pub struct Index {
    conn: Connection,
}

/// Why the index did not open.
enum OpenError {
    /// Damaged (`SQLITE_NOTADB`, `SQLITE_CORRUPT`) or of another schema version: replaced.
    Replace(String),
    /// Anything else (busy, locked, permissions, no file handles left): kept, `CACHE_IO`.
    Fail(String),
}

impl From<rusqlite::Error> for OpenError {
    fn from(e: rusqlite::Error) -> OpenError {
        use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
        match e.sqlite_error_code() {
            Some(NotADatabase | DatabaseCorrupt) => OpenError::Replace(e.to_string()),
            _ => OpenError::Fail(e.to_string()),
        }
    }
}

fn open_conn(path: &Path) -> Result<Connection, OpenError> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(OpenError::Fail(format!("journal mode {mode}, not WAL")));
    }
    // A lost tail after a power cut is a missing row: the adoption at start puts it back.
    conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        let tables: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |r| r.get(0),
        )?;
        if tables > 0 {
            return Err(OpenError::Replace(format!("schema version {version}")));
        }
        conn.execute_batch(&format!(
            "BEGIN; {SCHEMA} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
        ))?;
    }
    Ok(conn)
}

fn remove_db(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(p);
    }
}

impl Index {
    /// Opens `path`. A damaged database (`SQLITE_NOTADB`, `SQLITE_CORRUPT`) or one of another
    /// schema version is replaced by an empty one (the adoption rebuilds the rows); any other
    /// failure is `CACHE_IO {reason: index}` and the file is kept. Returns whether it was
    /// replaced.
    pub fn open(path: &Path) -> Result<(Index, bool), CoreError> {
        let fail = |e: String| {
            cache_io(
                "index",
                format!("the media cache index {} did not open: {e}", path.display()),
            )
        };
        match open_conn(path) {
            Ok(conn) => Ok((Index { conn }, false)),
            Err(OpenError::Fail(e)) => Err(fail(e)),
            Err(OpenError::Replace(_)) => {
                remove_db(path);
                match open_conn(path) {
                    Ok(conn) => Ok((Index { conn }, true)),
                    Err(OpenError::Fail(e)) => Err(fail(e)),
                    Err(OpenError::Replace(why)) => Err(cache_io(
                        "index",
                        format!("the media cache index is still unusable once replaced: {why}"),
                    )),
                }
            }
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<EntryMeta>, CoreError> {
        let text: Option<String> = self
            .conn
            .query_row("SELECT meta FROM entries WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(db)?;
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
    }

    pub fn contains(&self, key: &str) -> Result<bool, CoreError> {
        self.conn
            .query_row("SELECT 1 FROM entries WHERE key = ?1", [key], |_| Ok(()))
            .optional()
            .map(|r| r.is_some())
            .map_err(db)
    }

    /// Inserts the entry with its projects, and a `project:<id>` pin for each of them that is
    /// in `open` (the projects open now), in one transaction.
    pub fn insert(
        &mut self,
        meta: &EntryMeta,
        bytes: u64,
        now: i64,
        projects: &[String],
        open: &[String],
    ) -> Result<(), CoreError> {
        let text = serde_json::to_string(meta).map_err(|e| cache_io("index", e.to_string()))?;
        let tx = self.conn.transaction().map_err(db)?;
        tx.execute(
            "INSERT INTO entries (key, recipe, recipe_version, fingerprint, bytes, created_at, \
             last_used_at, meta) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                meta.key,
                meta.recipe,
                meta.recipe_version,
                meta.fingerprint,
                bytes as i64,
                meta.created_at,
                now,
                text
            ],
        )
        .map_err(db)?;
        add_projects(&tx, &meta.key, projects, open)?;
        tx.commit().map_err(db)
    }

    /// Adds projects an existing entry serves (and their pins when open).
    pub fn add_projects(
        &mut self,
        key: &str,
        projects: &[String],
        open: &[String],
    ) -> Result<(), CoreError> {
        let tx = self.conn.transaction().map_err(db)?;
        add_projects(&tx, key, projects, open)?;
        tx.commit().map_err(db)
    }

    pub fn touch(&self, key: &str, now: i64) -> Result<(), CoreError> {
        self.conn
            .execute(
                "UPDATE entries SET last_used_at = ?2 WHERE key = ?1",
                params![key, now],
            )
            .map(drop)
            .map_err(db)
    }

    pub fn delete(&self, key: &str) -> Result<(), CoreError> {
        self.conn
            .execute("DELETE FROM entries WHERE key = ?1", [key])
            .map(drop)
            .map_err(db)
    }

    /// Every row's key (for the adoption's check that each has its directory).
    pub fn keys(&self) -> Result<Vec<String>, CoreError> {
        let mut st = self.conn.prepare("SELECT key FROM entries").map_err(db)?;
        let rows = st.query_map([], |r| r.get(0)).map_err(db)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db)
    }

    /// Unpinned entries, least recently used first (ties by key).
    pub fn eviction_order(&self) -> Result<Vec<Row>, CoreError> {
        let mut st = self
            .conn
            .prepare(
                "SELECT key, bytes FROM entries \
                 WHERE key NOT IN (SELECT key FROM pins) ORDER BY last_used_at, key",
            )
            .map_err(db)?;
        let rows = st
            .query_map([], |r| {
                Ok(Row {
                    key: r.get(0)?,
                    bytes: r.get::<_, i64>(1)? as u64,
                })
            })
            .map_err(db)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db)
    }

    pub fn total_bytes(&self) -> Result<u64, CoreError> {
        let n: i64 = self
            .conn
            .query_row("SELECT coalesce(sum(bytes), 0) FROM entries", [], |r| {
                r.get(0)
            })
            .map_err(db)?;
        Ok(n as u64)
    }

    /// `(entries, pinned entries, per recipe)`.
    pub fn usage(&self) -> Result<(u64, u64, BTreeMap<String, RecipeUsage>), CoreError> {
        let mut st = self
            .conn
            .prepare("SELECT recipe, count(*), sum(bytes) FROM entries GROUP BY recipe")
            .map_err(db)?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    RecipeUsage {
                        entries: r.get::<_, i64>(1)? as u64,
                        bytes: r.get::<_, i64>(2)? as u64,
                    },
                ))
            })
            .map_err(db)?;
        let by: BTreeMap<String, RecipeUsage> =
            rows.collect::<rusqlite::Result<_>>().map_err(db)?;
        let entries = by.values().map(|u| u.entries).sum();
        let pinned: i64 = self
            .conn
            .query_row("SELECT count(DISTINCT key) FROM pins", [], |r| r.get(0))
            .map_err(db)?;
        Ok((entries, pinned as u64, by))
    }

    /// Entries made from the source with `fingerprint`, by recipe then key.
    pub fn entries_for(&self, fingerprint: &str) -> Result<Vec<EntryMeta>, CoreError> {
        let mut st = self
            .conn
            .prepare("SELECT meta FROM entries WHERE fingerprint = ?1 ORDER BY recipe, key")
            .map_err(db)?;
        let rows = st
            .query_map([fingerprint], |r| r.get::<_, String>(0))
            .map_err(db)?;
        let texts: Vec<String> = rows.collect::<rusqlite::Result<_>>().map_err(db)?;
        Ok(texts
            .iter()
            .filter_map(|t| serde_json::from_str(t).ok())
            .collect())
    }

    pub fn projects_of(&self, key: &str) -> Result<Vec<String>, CoreError> {
        let mut st = self
            .conn
            .prepare("SELECT project_id FROM entry_projects WHERE key = ?1 ORDER BY project_id")
            .map_err(db)?;
        let rows = st.query_map([key], |r| r.get(0)).map_err(db)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db)
    }

    /// Pins `key` for `holder`; false when there is no such entry.
    pub fn pin(&self, key: &str, holder: &str) -> Result<bool, CoreError> {
        if !self.contains(key)? {
            return Ok(false);
        }
        self.conn
            .execute(
                "INSERT OR IGNORE INTO pins (key, holder) VALUES (?1, ?2)",
                params![key, holder],
            )
            .map_err(db)?;
        Ok(true)
    }

    /// Removes `holder`'s pin of `key`, or with `key: None` every pin of `holder`; how many.
    pub fn unpin(&self, key: Option<&str>, holder: &str) -> Result<u64, CoreError> {
        let n = match key {
            Some(k) => self.conn.execute(
                "DELETE FROM pins WHERE key = ?1 AND holder = ?2",
                params![k, holder],
            ),
            None => self
                .conn
                .execute("DELETE FROM pins WHERE holder = ?1", [holder]),
        }
        .map_err(db)?;
        Ok(n as u64)
    }

    /// Pins every entry serving `project` for `project:<project>`.
    pub fn pin_project(&self, project: &str) -> Result<u64, CoreError> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO pins (key, holder) \
                 SELECT key, 'project:' || project_id FROM entry_projects WHERE project_id = ?1",
                [project],
            )
            .map(|n| n as u64)
            .map_err(db)
    }

    pub fn pinned(&self, key: &str) -> Result<Vec<String>, CoreError> {
        let mut st = self
            .conn
            .prepare("SELECT holder FROM pins WHERE key = ?1 ORDER BY holder")
            .map_err(db)?;
        let rows = st.query_map([key], |r| r.get(0)).map_err(db)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db)
    }

    pub fn clear_pins(&self) -> Result<(), CoreError> {
        self.conn
            .execute("DELETE FROM pins", [])
            .map(drop)
            .map_err(db)
    }
}

fn add_projects(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    projects: &[String],
    open: &[String],
) -> Result<(), CoreError> {
    for p in projects {
        tx.execute(
            "INSERT OR IGNORE INTO entry_projects (key, project_id) VALUES (?1, ?2)",
            params![key, p],
        )
        .map_err(db)?;
        if open.contains(p) {
            tx.execute(
                "INSERT OR IGNORE INTO pins (key, holder) VALUES (?1, ?2)",
                params![key, format!("project:{p}")],
            )
            .map_err(db)?;
        }
    }
    Ok(())
}
