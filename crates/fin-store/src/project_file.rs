//! `finisher.project.json` (sub-project 4 spec §3.4): the one file of the project folder that
//! is replaced rather than written once. It is written temp-then-rename ([`write`]), at
//! create and when settings change; plan Task 11 adds one more rewrite, the backfill of
//! [`Genesis`] into a sub-project 2 project file ([`Store::open`](crate::Store::open)).
//!
//! Fields:
//! - `id`, `name`, `schemaVersion` (sub-project 2);
//! - `formatVersion`, `minReaderVersion` ([`FORMAT_VERSION`], [`MIN_READER_VERSION`]): a file
//!   whose `minReaderVersion` is above ours opens read-only (`NEWER_FORMAT`);
//! - `storage`: the storage kind detected when the file was written (informational only;
//!   every open detects again);
//! - `storageOverride`: the user's choice of storage kind (Task 10);
//! - `genesis`: what the rev-0 project is built from ([`Genesis`]). A server with no database
//!   rebuilds the project from it plus the folder's segments (spec §3.8).
//!
//! Fields this version does not know are kept when the file is rewritten.
//!
//! Reading retries a missing file for up to [`READ_RETRY`] (15 s) once the folder holds
//! `.finisher/`: Windows can report a just-renamed file missing for about 5 s (spec §3.4).
//! On a network share a folder that shows neither (a project another host has just created)
//! is retried for up to [`NETWORK_RETRY`] (12 s): a client that looked the path up before
//! keeps answering "not found" from its cache for a while (measured: Windows ~4.9 s, its
//! not-found cache, once the path and `.finisher/` were looked up; macOS smbfs 0.4–2.4 s
//! after listing the parent or looking the path up). On macOS network shares
//! each retry first refreshes the listings of the folder's parent and the folder (at most once
//! a second, `scan::refresh_listing`), which shows the new folder at once there; on Windows
//! that does not help, so only the wait does. Anywhere else (local disks, sync folders: a
//! typo) a folder without `.finisher/` fails at once.
//! Transient errors ([`crate::fsio::is_transient`]: Windows sharing violations and access
//! denied while another process replaces the file, `EINTR`/`EAGAIN`/`EBUSY`/`ESTALE`) are
//! retried for the same 15 s.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use fin_core::{CoreError, Rational, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::paths::{self, PROJECT_FILE, project_file};
use crate::storage_kind::StorageKind;
use crate::{StoreError, unwritable};

/// The project file's format revision written by this code…
pub const FORMAT_VERSION: u32 = 1;
/// …and the oldest reader that understands it.
pub const MIN_READER_VERSION: u32 = 1;
/// How long a missing project file is retried once the folder holds `.finisher/`.
pub const READ_RETRY: Duration = Duration::from_secs(15);
const RETRY_EVERY: Duration = Duration::from_millis(100);
/// How long a project file that a network share does not show at all is retried.
pub const NETWORK_RETRY: Duration = Duration::from_secs(12);
/// Listings are refreshed at most this often while retrying.
const REFRESH_EVERY: Duration = Duration::from_secs(1);

/// What the rev-0 project is built from: `fin_core::new_project(id, name, rate)`. `name` is
/// the name at creation (a later rename changes the file's `name`, not the history).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Genesis {
    pub name: String,
    pub rate: Rational,
}

/// The genesis every segment and snapshot header names (`genesisHash`): BLAKE3 of the
/// genesis's JSON (`{"name","rate"}`, field order fixed by the struct). A rebuild refuses a
/// project file whose genesis does not hash to it (`GENESIS_MISMATCH`), and verify reports
/// it: an edited `genesis.rate` would otherwise silently change a rebuilt history.
pub fn genesis_hash(g: &Genesis) -> String {
    crate::db::hash_bytes(&serde_json::to_vec(g).expect("a genesis always serialises"))
}

/// `GENESIS_MISMATCH` for segment `files` written under another genesis than `expected`
/// (an import error: they are not imported, and writes wait until they are moved out).
pub(crate) fn genesis_mismatch(expected: &str, files: Vec<String>) -> CoreError {
    CoreError::new(
        "GENESIS_MISMATCH",
        format!(
            "{} segment file(s) in the project folder were written under another genesis than this project's",
            files.len()
        ),
    )
    .with_details(json!({ "expected": expected, "files": files }))
    .with_fix("move these segment files out of .finisher/segments: they are not this project's history")
}

/// The genesis of a rev-0 project (its creation name and rate).
pub(crate) fn genesis_of(rev0: &fin_core::Project) -> Genesis {
    Genesis {
        name: rev0.name.clone(),
        rate: rev0.rate,
    }
}

/// The database's `genesisHash` (`meta`), computed from its rev-0 snapshot of `main` and
/// stored the first time.
pub(crate) fn db_genesis_hash(conn: &rusqlite::Connection) -> Result<String, StoreError> {
    if let Some(h) = crate::db::meta(conn, "genesisHash")? {
        return Ok(h);
    }
    let rev0 = crate::db::snapshot_at(conn, crate::MAIN, 0)?
        .ok_or_else(|| CoreError::new("DB_ERROR", "the database has no rev-0 snapshot of main"))?;
    let h = genesis_hash(&genesis_of(&rev0));
    crate::db::set_meta(conn, "genesisHash", &h)?;
    Ok(h)
}

/// `finisher.project.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_reader_version: Option<u32>,
    /// The storage kind detected when the file was written (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    /// User override of the detected storage kind (`local`, `network` or `sync[:<provider>]`).
    /// Kept as raw JSON so a bad value cannot stop the project from opening: it is ignored
    /// with a warning instead ([`Store::storage_warning`](crate::Store::storage_warning)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_override: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genesis: Option<Genesis>,
    /// Fields of a newer revision, kept on rewrite.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

impl ProjectFile {
    /// A new project's file.
    pub(crate) fn new(id: &str, genesis: Genesis, storage: String) -> ProjectFile {
        ProjectFile {
            id: id.to_string(),
            name: genesis.name.clone(),
            schema_version: SCHEMA_VERSION,
            format_version: Some(FORMAT_VERSION),
            min_reader_version: Some(MIN_READER_VERSION),
            storage: Some(storage),
            storage_override: None,
            genesis: Some(genesis),
            other: serde_json::Map::new(),
        }
    }

    /// Written by a newer Finisher that this one cannot fully read.
    pub fn is_newer(&self) -> bool {
        self.min_reader_version.is_some_and(|v| v > FORMAT_VERSION)
    }
}

/// Test hook: the read retry window (`None`: [`READ_RETRY`]).
static RETRY_MS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Test hook: how long a missing project file is retried.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn set_read_retry_for_test(d: Duration) {
    RETRY_MS.store(
        u64::try_from(d.as_millis()).unwrap_or(u64::MAX - 1),
        Ordering::SeqCst,
    );
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static NETWORK: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
}

/// Test hook: on this thread, every folder counts as on a macOS network share whose missing
/// project file is retried for `window` (instead of [`NETWORK_RETRY`]); `None` undoes it.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn network_for_test(window: Option<Duration>) {
    NETWORK.with(|n| n.set(window));
}

/// How long a project folder that does not show its project file is retried as a network
/// share's, and whether its listings are refreshed first (module docs). Judged by the nearest
/// folder that exists: the project folder itself may be what the cache hides.
fn network_retry(project_dir: &Path) -> (Duration, bool) {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(w) = NETWORK.with(std::cell::Cell::get) {
        return (w, true);
    }
    let abs = std::path::absolute(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let Some(existing) = abs.ancestors().find(|a| a.is_dir()) else {
        return (Duration::ZERO, false);
    };
    match crate::storage_kind::detect(existing, None) {
        k @ StorageKind::Network => (NETWORK_RETRY, crate::storage_kind::slow_dir_cache(k)),
        _ => (Duration::ZERO, false),
    }
}

/// Drops the client's cached listings that could hide a new project folder: its parent's
/// and its own.
fn refresh_listings(project_dir: &Path) {
    if let Some(parent) = project_dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        crate::scan::refresh_listing(parent);
    }
    if project_dir.is_dir() {
        crate::scan::refresh_listing(project_dir);
    }
}

fn retry_window() -> Duration {
    match RETRY_MS.load(Ordering::SeqCst) {
        u64::MAX => READ_RETRY,
        ms => Duration::from_millis(ms),
    }
}

/// Reads and checks `<project_dir>/finisher.project.json` (module docs for the retry).
pub fn read_project_file(project_dir: &Path) -> Result<ProjectFile, StoreError> {
    read(project_dir, retry_window())
}

/// [`read_project_file`] without the retry.
pub(crate) fn read_project_file_once(project_dir: &Path) -> Result<ProjectFile, StoreError> {
    read(project_dir, Duration::ZERO)
}

fn read(project_dir: &Path, retry: Duration) -> Result<ProjectFile, StoreError> {
    let path = project_file(project_dir);
    let invalid = |why: String| {
        CoreError::new(
            "PROJECT_INVALID",
            format!("{} is not a valid project file: {why}", path.display()),
        )
        .with_details(json!({ "path": path.to_string_lossy() }))
    };
    let started = Instant::now();
    let mut network: Option<(Duration, bool)> = None;
    let mut refreshed: Option<Instant> = None;
    let bytes = loop {
        match crate::fsio::hydrating(&path, || fs::read(&path)) {
            Ok(b) => break b,
            Err(e)
                if started.elapsed() < retry
                    && ((e.kind() == std::io::ErrorKind::NotFound
                        && project_dir.join(".finisher").is_dir())
                        || crate::fsio::is_transient(&e)) =>
            {
                std::thread::sleep(RETRY_EVERY);
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && !retry.is_zero()
                    && started.elapsed()
                        < network.get_or_insert_with(|| network_retry(project_dir)).0 =>
            {
                if network.is_some_and(|n| n.1)
                    && refreshed.is_none_or(|t| t.elapsed() >= REFRESH_EVERY)
                {
                    refresh_listings(project_dir);
                    refreshed = Some(Instant::now());
                    continue;
                }
                std::thread::sleep(RETRY_EVERY);
            }
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
        }
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

/// Writes `file` to `<project_dir>/finisher.project.json`, temp-then-rename.
pub(crate) fn write(project_dir: &Path, file: &ProjectFile) -> Result<(), StoreError> {
    let path = project_file(project_dir);
    let mut bytes = serde_json::to_vec_pretty(file)?;
    bytes.push(b'\n');
    paths::write_atomic(&path, &bytes).map_err(unwritable(&path))
}

/// The genesis of a sub-project 2 project, from its database's rev-0 snapshot: only if
/// `new_project` rebuilds exactly that snapshot (otherwise a rebuild would not reproduce the
/// history, and nothing is backfilled).
pub(crate) fn genesis_from(id: &str, rev0: &fin_core::Project) -> Option<Genesis> {
    let g = Genesis {
        name: rev0.name.clone(),
        rate: rev0.rate,
    };
    let rebuilt = fin_core::new_project(id.to_string(), g.name.clone(), g.rate).ok()?;
    (rebuilt == *rev0).then_some(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fields_survive_a_rewrite() {
        let json = r#"{"id":"x","name":"n","schemaVersion":1,"future":{"a":1}}"#;
        let f: ProjectFile = serde_json::from_str(json).unwrap();
        let back = serde_json::to_value(&f).unwrap();
        assert_eq!(back["future"], json!({"a": 1}));
        assert!(back.get("genesis").is_none());
    }

    /// A folder that shows neither the project file nor `.finisher/`: on a network share
    /// retried (refreshing listings first) until another host's new project shows up,
    /// within a bound; on a local disk answered at once (a typo).
    #[test]
    fn a_project_a_network_share_does_not_show_yet_is_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        let not_found = |r: Result<ProjectFile, StoreError>| match r {
            Err(StoreError::Core(e)) => assert_eq!(e.code, "PROJECT_NOT_FOUND"),
            other => panic!("{other:?}"),
        };
        let t = Instant::now();
        not_found(read_project_file(&dir));
        assert!(t.elapsed() < Duration::from_millis(150), "local: at once");

        network_for_test(Some(Duration::from_secs(10)));
        let refreshes = crate::scan::refreshes();
        let genesis = Genesis {
            name: "Demo".into(),
            rate: Rational::from_int(30),
        };
        let file = ProjectFile::new("01jbcdefghjkmnpqrstvwxyz00", genesis, "network".into());
        let creator = std::thread::spawn({
            let dir = dir.clone();
            move || {
                std::thread::sleep(Duration::from_millis(400));
                fs::create_dir(&dir).unwrap();
                write(&dir, &file).unwrap();
            }
        });
        let t = Instant::now();
        assert_eq!(read_project_file(&dir).unwrap().name, "Demo");
        assert!(t.elapsed() >= Duration::from_millis(350));
        creator.join().unwrap();
        assert!(crate::scan::refreshes() > refreshes, "listings refreshed");
        // Bounded, and never for the once-only read.
        let gone = tmp.path().join("gone");
        network_for_test(Some(Duration::from_millis(300)));
        let t = Instant::now();
        not_found(read_project_file(&gone));
        assert!(t.elapsed() >= Duration::from_millis(300));
        let t = Instant::now();
        not_found(read_project_file_once(&gone));
        assert!(t.elapsed() < Duration::from_millis(150));
        network_for_test(None);
    }

    #[test]
    fn newer_reader_version_is_flagged() {
        let json = r#"{"id":"x","name":"n","schemaVersion":1,"minReaderVersion":2}"#;
        let f: ProjectFile = serde_json::from_str(json).unwrap();
        assert!(f.is_newer());
    }
}
