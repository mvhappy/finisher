//! Where `fin` gets project state from and sends transactions to.
//!
//! `Offline` reads the `FIN_OFFLINE_SNAPSHOT=<file>` test hook (`{ "rev", "project" }`); every
//! write against it is a dry run. `Online` talks to `finisherd` over its local socket.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};

use fin_core::{AudioInfo, CommittedTx, CoreError, Project, Rational, Tx, VideoInfo};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::Client;
use crate::output::{internal, unavailable};
use crate::{endpoint, paths};

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

fn no_project() -> CoreError {
    let cwd = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
    CoreError::new("PROJECT_NOT_FOUND", format!("no {PROJECT_FILE} in the working directory or any parent"))
        .with_details(json!({ "cwd": cwd }))
        .with_fix("run `fin project create <dir> --name <s>` to make a project, or pass --project <dir>")
}

fn unexpected(tag: &str, e: impl std::fmt::Display, value: &Value) -> CoreError {
    internal(format!("finisherd answered {tag} with an unexpected shape: {e}"), json!({ "tag": tag, "value": value }))
        .with_fix("use fin and finisherd from the same Finisher release")
}

/// Test hook: `FIN_TEST_COMMIT_GATE=<file>` makes `commit` create `<file>.ready.<pid>` and wait
/// until `<file>` exists, so a test can line up concurrent writers on the same snapshot.
pub const COMMIT_GATE_ENV: &str = "FIN_TEST_COMMIT_GATE";

fn commit_gate(gate: &Path) -> Result<(), CoreError> {
    let ready = format!("{}.ready.{}", gate.display(), std::process::id());
    std::fs::write(&ready, b"").map_err(|e| internal(format!("{COMMIT_GATE_ENV}: {e}"), json!({ "path": ready })))?;
    let deadline = std::time::Instant::now() + crate::client::TIMEOUT;
    while !gate.exists() {
        if std::time::Instant::now() > deadline {
            return Err(internal(format!("{COMMIT_GATE_ENV} never opened"), json!({ "path": gate.display().to_string() })));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

/// The running `finisherd`, reached over its local socket. Connects on first use (so local-only
/// paths such as `project create --dry-run` never need a server) and registers the project
/// folder with `project.open` to learn its id.
pub struct Online {
    project: Option<PathBuf>,
    client: RefCell<Option<Client>>,
    project_id: OnceCell<String>,
}

impl Online {
    pub fn new(project: Option<PathBuf>) -> Online {
        Online { project, client: RefCell::new(None), project_id: OnceCell::new() }
    }

    fn with_client<T>(&self, f: impl FnOnce(&mut Client) -> Result<T, CoreError>) -> Result<T, CoreError> {
        let mut slot = self.client.borrow_mut();
        let client = match &mut *slot {
            Some(c) => c,
            None => slot.insert(Client::connect(&endpoint::resolve()?, crate::client::TIMEOUT)?),
        };
        f(client)
    }

    fn call(&self, tag: &str, payload: Value) -> Result<Value, CoreError> {
        self.with_client(|c| c.call(tag, payload))
    }

    fn call_as<T: DeserializeOwned>(&self, tag: &str, payload: Value) -> Result<T, CoreError> {
        let value = self.call(tag, payload)?;
        serde_json::from_value(value.clone()).map_err(|e| unexpected(tag, e, &value))
    }

    fn project_id(&self) -> Result<String, CoreError> {
        if let Some(id) = self.project_id.get() {
            return Ok(id.clone());
        }
        let dir = self.project.as_ref().ok_or_else(no_project)?;
        let opened: ProjectRef = self.call_as("project.open", json!({ "dir": dir }))?;
        Ok(self.project_id.get_or_init(|| opened.project_id).clone())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRef {
    project_id: String,
}

/// `asset.probe`'s answer; the path is the one fin sent.
#[derive(Deserialize)]
struct ProbeResult {
    duration: Rational,
    video: Option<VideoInfo>,
    audio: Option<AudioInfo>,
    fingerprint: String,
}

impl Backend for Online {
    fn snapshot(&self) -> Result<Snapshot, CoreError> {
        let project_id = self.project_id()?;
        self.call_as("snapshot.get", json!({ "projectId": project_id, "branch": "main" }))
    }

    fn commit(&self, tx: &Tx) -> Result<CommittedTx, CoreError> {
        let project_id = self.project_id()?;
        if let Some(gate) = std::env::var_os(COMMIT_GATE_ENV).filter(|g| !g.is_empty()) {
            commit_gate(Path::new(&gate))?;
        }
        let payload = json!({ "projectId": project_id, "tx": tx });
        let value = self.with_client(|c| c.commit(payload, &tx.tx_id))?;
        serde_json::from_value(value.clone()).map_err(|e| unexpected("tx.commit", e, &value))
    }

    fn probe(&self, path: &Path) -> Result<Probed, CoreError> {
        let Some(text) = path.to_str() else {
            return Err(CoreError::new("ASSET_UNREADABLE", "media path is not valid UTF-8")
                .with_details(json!({ "path": path.to_string_lossy() }))
                .with_fix("rename the file to a UTF-8 name"));
        };
        let p: ProbeResult = self.call_as("asset.probe", json!({ "path": text }))?;
        Ok(Probed { path: text.to_string(), duration: p.duration, video: p.video, audio: p.audio, fingerprint: p.fingerprint })
    }

    fn create_project(&self, dir: &Path, name: &str, rate: Rational) -> Result<Value, CoreError> {
        self.call("project.create", json!({ "dir": dir, "name": name, "rate": rate }))
    }

    fn open_project(&self, dir: &Path) -> Result<Value, CoreError> {
        self.call("project.open", json!({ "dir": dir }))
    }
}

/// `--project <dir>` (with `~` expanded), else the nearest ancestor of the working directory
/// that holds `finisher.project.json`.
pub fn project_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(paths::resolve(dir));
    }
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors().find(|d| d.join(PROJECT_FILE).is_file()).map(Path::to_path_buf)
}

/// The snapshot hook when set, otherwise the server.
pub fn select(project: Option<&Path>) -> Result<Box<dyn Backend>, CoreError> {
    match std::env::var_os(SNAPSHOT_ENV) {
        Some(file) if !file.is_empty() => Ok(Box::new(Offline::load(Path::new(&file))?)),
        _ => Ok(Box::new(Online::new(project_dir(project)))),
    }
}
