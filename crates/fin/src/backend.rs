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

/// A project at a revision. `generation` (`{incarnation, n}`, read together with `rev`) is
/// sent back with the commit, so a branch rewound or a database rebuilt in between refuses it
/// with `HISTORY_REWOUND` instead of rebasing it over a history this command never saw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub rev: u64,
    pub project: Project,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<Value>,
}

/// What became of a tx id (`tx.get`).
#[derive(Debug, Clone, PartialEq)]
pub enum TxLookup {
    Committed(Box<CommittedTx>),
    /// Set aside in quarantine: `{txId, branch, reason, origin}`.
    SetAside(Value),
    Unknown,
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
    /// The current project state on `branch` (a branch reference; the offline hook has one
    /// branch and ignores it).
    fn snapshot(&self, branch: &str) -> Result<Snapshot, CoreError>;
    /// `(server-data, project id)`: where `fin-config.json` lives and which project's entry
    /// is ours. `None` when there is no server to ask (offline) or it cannot be reached.
    fn config_scope(&self) -> Option<(PathBuf, String)> {
        None
    }
    /// Commits `tx`; the server rebases it over anything newer than `tx.base_rev`.
    /// `generation`: the snapshot's (see [`Snapshot`]).
    fn commit(&self, tx: &Tx, generation: Option<&Value>) -> Result<CommittedTx, CoreError>;
    /// What became of `tx_id`.
    fn tx_get(&self, tx_id: &str) -> Result<TxLookup, CoreError>;
    /// A project-scoped RPC (`project.status`, …): `payload` gains the project's id.
    fn project_call(&self, tag: &str, payload: Value) -> Result<Value, CoreError>;
    /// Rebuilds the project database: through the server when one runs (it holds the
    /// database), else in this process. `Ok((report, offline))`.
    fn rebuild(&self) -> Result<(Value, bool), CoreError>;
    /// Probes a media file for `asset add`.
    fn probe(&self, path: &Path) -> Result<Probed, CoreError>;
    /// Creates a project folder; returns the server's description of it. `idem`: a retry
    /// with the same key returns the project that create made.
    fn create_project(
        &self,
        dir: &Path,
        name: &str,
        rate: Rational,
        idem: Option<&str>,
    ) -> Result<Value, CoreError>;
    /// Opens (registers) an existing project folder; `relocate`: see `fin project open`.
    fn open_project(&self, dir: &Path, relocate: bool) -> Result<Value, CoreError>;
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
            internal(
                format!("cannot use {SNAPSHOT_ENV}: {why}"),
                json!({ "path": path.display().to_string() }),
            )
            .with_fix(format!(
                "point {SNAPSHOT_ENV} at a JSON file holding {{ \"rev\": N, \"project\": {{...}} }}"
            ))
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
    fn snapshot(&self, _branch: &str) -> Result<Snapshot, CoreError> {
        Ok(self.snapshot.clone())
    }

    fn commit(&self, _tx: &Tx, _generation: Option<&Value>) -> Result<CommittedTx, CoreError> {
        Err(offline_only("commit"))
    }

    fn tx_get(&self, _tx_id: &str) -> Result<TxLookup, CoreError> {
        Err(offline_only("tx.get"))
    }

    fn project_call(&self, tag: &str, _payload: Value) -> Result<Value, CoreError> {
        Err(offline_only(tag))
    }

    fn rebuild(&self) -> Result<(Value, bool), CoreError> {
        Err(offline_only("project.rebuild"))
    }

    fn probe(&self, _path: &Path) -> Result<Probed, CoreError> {
        Err(offline_only("probe"))
    }

    fn create_project(
        &self,
        _dir: &Path,
        _name: &str,
        _rate: Rational,
        _idem: Option<&str>,
    ) -> Result<Value, CoreError> {
        Err(offline_only("project.create"))
    }

    fn open_project(&self, _dir: &Path, _relocate: bool) -> Result<Value, CoreError> {
        Err(offline_only("project.open"))
    }

    fn dry_run_only(&self) -> bool {
        true
    }
}

fn no_project() -> CoreError {
    let cwd = std::env::current_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    CoreError::new(
        "PROJECT_NOT_FOUND",
        format!("no {PROJECT_FILE} in the working directory or any parent"),
    )
    .with_details(json!({ "cwd": cwd }))
    .with_fix(
        "run `fin project create <dir> --name <s>` to make a project, or pass --project <dir>",
    )
}

fn unexpected(tag: &str, e: impl std::fmt::Display, value: &Value) -> CoreError {
    internal(
        format!("finisherd answered {tag} with an unexpected shape: {e}"),
        json!({ "tag": tag, "value": value }),
    )
    .with_fix("use fin and finisherd from the same Finisher release")
}

/// Test hook: `FIN_TEST_COMMIT_GATE=<file>` makes `commit` create `<file>.ready.<pid>` and wait
/// until `<file>` exists, so a test can line up concurrent writers on the same snapshot.
pub const COMMIT_GATE_ENV: &str = "FIN_TEST_COMMIT_GATE";

fn commit_gate(gate: &Path) -> Result<(), CoreError> {
    let ready = format!("{}.ready.{}", gate.display(), std::process::id());
    std::fs::write(&ready, b"")
        .map_err(|e| internal(format!("{COMMIT_GATE_ENV}: {e}"), json!({ "path": ready })))?;
    let deadline = std::time::Instant::now() + crate::client::TIMEOUT;
    while !gate.exists() {
        if std::time::Instant::now() > deadline {
            return Err(internal(
                format!("{COMMIT_GATE_ENV} never opened"),
                json!({ "path": gate.display().to_string() }),
            ));
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
        Online {
            project,
            client: RefCell::new(None),
            project_id: OnceCell::new(),
        }
    }

    fn with_client<T>(
        &self,
        f: impl FnOnce(&mut Client) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let mut slot = self.client.borrow_mut();
        let client = match &mut *slot {
            Some(c) => c,
            None => slot.insert(Client::connect(
                &endpoint::resolve()?,
                crate::client::TIMEOUT,
            )?),
        };
        let result = f(client);
        // A transport failure leaves the connection unusable: the next call reconnects.
        if let Err(e) = &result
            && crate::client::IN_FLIGHT_CODES.contains(&e.code.as_str())
        {
            *slot = None;
        }
        result
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
    fn snapshot(&self, branch: &str) -> Result<Snapshot, CoreError> {
        let project_id = self.project_id()?;
        self.call_as(
            "snapshot.get",
            json!({ "projectId": project_id, "branch": branch }),
        )
    }

    fn config_scope(&self) -> Option<(PathBuf, String)> {
        Some((endpoint::server_data()?, self.project_id().ok()?))
    }

    fn commit(&self, tx: &Tx, generation: Option<&Value>) -> Result<CommittedTx, CoreError> {
        let project_id = self.project_id()?;
        if let Some(gate) = std::env::var_os(COMMIT_GATE_ENV).filter(|g| !g.is_empty()) {
            commit_gate(Path::new(&gate))?;
        }
        let mut payload = json!({ "projectId": project_id, "tx": tx });
        if let Some(g) = generation {
            payload["generation"] = g.clone();
        }
        let value = self.with_client(|c| c.commit(payload, &tx.tx_id))?;
        serde_json::from_value(value.clone()).map_err(|e| unexpected("tx.commit", e, &value))
    }

    fn tx_get(&self, tx_id: &str) -> Result<TxLookup, CoreError> {
        let value = self.project_call("tx.get", json!({ "txId": tx_id }))?;
        if value.is_null() {
            return Ok(TxLookup::Unknown);
        }
        if let Some(aside) = value.get("setAside") {
            return Ok(TxLookup::SetAside(aside.clone()));
        }
        serde_json::from_value(value.clone())
            .map(|c| TxLookup::Committed(Box::new(c)))
            .map_err(|e| unexpected("tx.get", e, &value))
    }

    fn project_call(&self, tag: &str, mut payload: Value) -> Result<Value, CoreError> {
        let project_id = self.project_id()?;
        payload["projectId"] = json!(project_id);
        self.call(tag, payload)
    }

    fn rebuild(&self) -> Result<(Value, bool), CoreError> {
        // Only "nobody listens" means no server holds the database; any other failure to
        // reach one is reported, never answered with a rebuild behind its back.
        match self.with_client(|_| Ok(())) {
            Ok(()) => Ok((self.project_call("project.rebuild", json!({}))?, false)),
            Err(e) if e.code == "SERVER_UNAVAILABLE" => {
                let dir = self.project.as_ref().ok_or_else(no_project)?;
                let sd = endpoint::server_data().ok_or(e)?;
                let report = rebuild_here(&sd, dir)?;
                let report = serde_json::to_value(report).map_err(|e| {
                    internal(format!("cannot encode the rebuild report: {e}"), json!({}))
                })?;
                Ok((report, true))
            }
            Err(e) => Err(e),
        }
    }

    fn probe(&self, path: &Path) -> Result<Probed, CoreError> {
        let Some(text) = path.to_str() else {
            return Err(
                CoreError::new("ASSET_UNREADABLE", "media path is not valid UTF-8")
                    .with_details(json!({ "path": path.to_string_lossy() }))
                    .with_fix("rename the file to a UTF-8 name"),
            );
        };
        let p: ProbeResult = self.call_as("asset.probe", json!({ "path": text }))?;
        Ok(Probed {
            path: text.to_string(),
            duration: p.duration,
            video: p.video,
            audio: p.audio,
            fingerprint: p.fingerprint,
        })
    }

    fn create_project(
        &self,
        dir: &Path,
        name: &str,
        rate: Rational,
        idem: Option<&str>,
    ) -> Result<Value, CoreError> {
        let mut payload = json!({ "dir": dir, "name": name, "rate": rate });
        if let Some(key) = idem {
            payload["idemKey"] = json!(key);
        }
        self.call("project.create", payload)
    }

    fn open_project(&self, dir: &Path, relocate: bool) -> Result<Value, CoreError> {
        let mut payload = json!({ "dir": dir });
        if relocate {
            payload["relocate"] = json!(true);
        }
        self.call("project.open", payload)
    }
}

/// `fin project rebuild` with no server: opened in-process as a server would, so commits the
/// database holds but never exported are exported first (`Store::rebuild`) rather than set
/// aside. Only a database that cannot be opened is rebuilt from the folder alone
/// (`rebuild_project`, which salvages what it can read of the old one).
fn rebuild_here(sd: &Path, dir: &Path) -> Result<fin_store::RebuildReport, CoreError> {
    match fin_store::Store::open(sd, dir) {
        Ok(mut store) => {
            let report = store.rebuild();
            let closed = store.close();
            let report = report?;
            closed?;
            Ok(report)
        }
        Err(e) if matches!(e.code(), "DB_ERROR" | "DB_CORRUPT" | "REPLAY_FAILED") => {
            Ok(fin_store::rebuild_project(sd, dir)?)
        }
        Err(e) => Err(e.into()),
    }
}

/// `--project <dir>` (with `~` expanded), else the nearest ancestor of the working directory
/// that holds `finisher.project.json`.
pub fn project_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(paths::resolve(dir));
    }
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors()
        .find(|d| d.join(PROJECT_FILE).is_file())
        .map(Path::to_path_buf)
}

/// The snapshot hook when set, otherwise the server.
pub fn select(project: Option<&Path>) -> Result<Box<dyn Backend>, CoreError> {
    match std::env::var_os(SNAPSHOT_ENV) {
        Some(file) if !file.is_empty() => Ok(Box::new(Offline::load(Path::new(&file))?)),
        _ => Ok(Box::new(Online::new(project_dir(project)))),
    }
}
