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
//! `.finisher/`: Windows can report a just-renamed file missing for about 5 s (spec §3.4). A
//! folder without `.finisher/` (never opened by sub-project 4, or not a project) fails at once.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use fin_core::{CoreError, Rational, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::paths::{self, PROJECT_FILE, project_file};
use crate::{StoreError, unwritable};

/// The project file's format revision written by this code…
pub const FORMAT_VERSION: u32 = 1;
/// …and the oldest reader that understands it.
pub const MIN_READER_VERSION: u32 = 1;
/// How long a missing project file is retried once the folder holds `.finisher/`.
pub const READ_RETRY: Duration = Duration::from_secs(15);
const RETRY_EVERY: Duration = Duration::from_millis(100);

/// What the rev-0 project is built from: `fin_core::new_project(id, name, rate)`. `name` is
/// the name at creation (a later rename changes the file's `name`, not the history).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Genesis {
    pub name: String,
    pub rate: Rational,
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
    let bytes = loop {
        match fs::read(&path) {
            Ok(b) => break b,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && project_dir.join(".finisher").is_dir()
                    && started.elapsed() < retry =>
            {
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

    #[test]
    fn newer_reader_version_is_flagged() {
        let json = r#"{"id":"x","name":"n","schemaVersion":1,"minReaderVersion":2}"#;
        let f: ProjectFile = serde_json::from_str(json).unwrap();
        assert!(f.is_newer());
    }
}
