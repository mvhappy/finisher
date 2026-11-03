//! Where `fin` gets project state from and sends transactions to.
//!
//! `Offline` reads the `FIN_OFFLINE_SNAPSHOT=<file>` test hook (`{ "rev", "project" }`); every
//! write against it is a dry run. `Unavailable` stands in for the server transport (Task 12
//! adds a socket implementation of [`Backend`]) and answers `SERVER_UNAVAILABLE`.

use std::path::{Path, PathBuf};

use fin_core::{AudioInfo, CommittedTx, CoreError, Project, Rational, Tx, VideoInfo};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::output::{internal, unavailable};

pub const SNAPSHOT_ENV: &str = "FIN_OFFLINE_SNAPSHOT";
const PROJECT_FILE: &str = "finisher.project.json";

/// A project at a revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub rev: u64,
    pub project: Project,
}

/// What the server's probe reports about a media file (the `asset.add` fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probed {
    /// Absolute path as the server resolved it.
    pub path: String,
    pub duration: Rational,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
    pub fingerprint: String,
}

pub trait Backend {
    /// The current project state on `main`.
    fn snapshot(&self) -> Result<Snapshot, CoreError>;
    /// Commits `tx`; the server rebases it over anything newer than `tx.base_rev`.
    fn commit(&self, tx: &Tx) -> Result<CommittedTx, CoreError>;
    /// Probes a media file for `asset add`.
    fn probe(&self, path: &Path) -> Result<Probed, CoreError>;
    /// Creates a project folder; returns the server's description of it.
    fn create_project(&self, dir: &Path, name: &str, rate: Rational) -> Result<Value, CoreError>;
    /// Opens (registers) an existing project folder.
    fn open_project(&self, dir: &Path) -> Result<Value, CoreError>;
    /// True when writes can only be dry runs (no server to commit to).
    fn dry_run_only(&self) -> bool {
        false
    }
}

/// Reads a snapshot file; every write is a dry run, nothing reaches a server.
pub struct Offline {
    snapshot: Snapshot,
}

impl Offline {
    pub fn load(path: &Path) -> Result<Offline, CoreError> {
        let bad = |why: String| {
            internal(format!("cannot use {SNAPSHOT_ENV}: {why}"), json!({ "path": path.display().to_string() }))
                .with_fix(format!("point {SNAPSHOT_ENV} at a JSON file holding {{ \"rev\": N, \"project\": {{...}} }}"))
        };
        let text = std::fs::read_to_string(path).map_err(|e| bad(e.to_string()))?;
        let snapshot = serde_json::from_str(&text).map_err(|e| bad(e.to_string()))?;
        Ok(Offline { snapshot })
    }
}

fn offline_only(what: &str) -> CoreError {
    unavailable(json!({ "mode": "offline", "operation": what }))
}

impl Backend for Offline {
    fn snapshot(&self) -> Result<Snapshot, CoreError> {
        Ok(self.snapshot.clone())
    }

    fn commit(&self, _tx: &Tx) -> Result<CommittedTx, CoreError> {
        Err(offline_only("commit"))
    }

    fn probe(&self, _path: &Path) -> Result<Probed, CoreError> {
        Err(offline_only("probe"))
    }

    fn create_project(&self, _dir: &Path, _name: &str, _rate: Rational) -> Result<Value, CoreError> {
        Err(offline_only("project.create"))
    }

    fn open_project(&self, _dir: &Path) -> Result<Value, CoreError> {
        Err(offline_only("project.open"))
    }

    fn dry_run_only(&self) -> bool {
        true
    }
}

/// No transport yet: every call is `SERVER_UNAVAILABLE`, naming the project it was for.
pub struct Unavailable {
    project: Option<PathBuf>,
}

impl Unavailable {
    fn fail(&self) -> CoreError {
        unavailable(json!({ "project": self.project.as_ref().map(|p| p.display().to_string()) }))
    }
}

impl Backend for Unavailable {
    fn snapshot(&self) -> Result<Snapshot, CoreError> {
        Err(self.fail())
    }

    fn commit(&self, _tx: &Tx) -> Result<CommittedTx, CoreError> {
        Err(self.fail())
    }

    fn probe(&self, _path: &Path) -> Result<Probed, CoreError> {
        Err(self.fail())
    }

    fn create_project(&self, _dir: &Path, _name: &str, _rate: Rational) -> Result<Value, CoreError> {
        Err(self.fail())
    }

    fn open_project(&self, _dir: &Path) -> Result<Value, CoreError> {
        Err(self.fail())
    }
}

/// `--project <dir>`, else the nearest ancestor of the working directory that holds
/// `finisher.project.json`.
pub fn project_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()));
    }
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors().find(|d| d.join(PROJECT_FILE).is_file()).map(Path::to_path_buf)
}

/// The snapshot hook when set, otherwise the (not yet implemented) server transport.
pub fn select(project: Option<&Path>) -> Result<Box<dyn Backend>, CoreError> {
    match std::env::var_os(SNAPSHOT_ENV) {
        Some(file) if !file.is_empty() => Ok(Box::new(Offline::load(Path::new(&file))?)),
        _ => Ok(Box::new(Unavailable { project: project_dir(project) })),
    }
}
