//! `fin` under an agent session's credential (sub-project 7 §5.1, §5.2; plan 7A Decisions 14a,
//! 14b, 14d).
//!
//! When `FIN_CREDENTIAL` names an `agent` credential, its id is the session id and the server
//! keeps the session's branch state (`session_state`): the default branch (the session's root,
//! or a `fin try` it switched to) and its tries, read through `agent.self` and written through
//! `agent.self.try`. `fin` then neither reads nor writes `fin-config.json`, so one session's
//! `fin try` cannot move what a person's or another session's `fin` writes, and a stale file
//! cannot point an agent at `main`. The project is the session's own (`agent.self`'s
//! `projectId`): `project.open` is refused to agent principals, and a project folder naming
//! another project is `PROJECT_NOT_FOUND`. The author of its records is set by the server
//! (`agent:<role>:<session>`); `FIN_AUTHOR` is ignored.

use std::path::Path;

use fin_core::CoreError;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{self, CredentialKind};
use crate::backend::Backend;

/// The session's branch state as `agent.self` answers it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfState {
    pub project_id: String,
    #[serde(default)]
    pub root_branch: Option<String>,
    pub default_branch: String,
    #[serde(default)]
    pub tries: Vec<SessionTry>,
}

/// One recorded `fin try`: its branch and the default it replaced.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTry {
    pub branch: String,
    pub previous_default: String,
}

/// The session id when `FIN_CREDENTIAL` names an agent credential (read now); `None` for a
/// person's `fin`, an MCP credential, or a credential file that cannot be read (which fails
/// later as `UNAUTHENTICATED`, before anything is sent).
pub fn session_id() -> Option<String> {
    let file = std::env::var_os(auth::CREDENTIAL_ENV).filter(|v| !v.is_empty())?;
    let cred = auth::read_credential(&crate::paths::resolve(Path::new(&file))).ok()??;
    (cred.kind == CredentialKind::Agent)
        .then_some(cred.id)
        .flatten()
}

/// Whether this `fin` runs under an agent session's credential.
pub fn active() -> bool {
    session_id().is_some()
}

thread_local! {
    /// The state this run last read or wrote: one `agent.self` per `fin` run.
    static STATE: std::cell::RefCell<Option<SelfState>> = const { std::cell::RefCell::new(None) };
}

fn remember(state: SelfState) -> SelfState {
    STATE.with(|s| *s.borrow_mut() = Some(state.clone()));
    state
}

/// `agent.self`: the calling session's branch state (read once per run, then kept current by
/// this run's own `agent.self.try` calls).
pub fn state(backend: &dyn Backend) -> Result<SelfState, CoreError> {
    if let Some(known) = STATE.with(|s| s.borrow().clone()) {
        return Ok(known);
    }
    let v = backend.server_call("agent.self", json!({}))?;
    let state: SelfState = serde_json::from_value(v.clone()).map_err(|e| {
        crate::output::internal(
            format!("agent.self answered something fin cannot read: {e}"),
            json!({ "answer": v }),
        )
    })?;
    Ok(remember(state))
}

/// The branch a command with no `--branch` writes under an agent credential: the session's
/// default (its root, or the `fin try` it switched to), never `fin-config.json`'s.
pub fn session_default_branch(backend: &dyn Backend) -> Result<String, CoreError> {
    Ok(state(backend)?.default_branch)
}

fn try_call(backend: &dyn Backend, action: &str, branch: &str) -> Result<SelfState, CoreError> {
    let v = backend.server_call(
        "agent.self.try",
        json!({ "action": action, "branch": branch }),
    )?;
    let state: SelfState = serde_json::from_value(v.clone()).map_err(|e| {
        crate::output::internal(
            format!("agent.self.try answered something fin cannot read: {e}"),
            json!({ "answer": v }),
        )
    })?;
    Ok(remember(state))
}

/// `fin try`: records `branch` (it must be in the session's subtree, else
/// `BRANCH_NOT_ALLOWED`) and makes it the session's default.
pub fn record_try(backend: &dyn Backend, branch: &str) -> Result<(), CoreError> {
    try_call(backend, "record", branch)?;
    try_call(backend, "setDefault", branch)?;
    Ok(())
}

/// `fin branch default <b>`: makes a recorded try (or the root) the session's default.
pub fn set_default(backend: &dyn Backend, branch: &str) -> Result<SelfState, CoreError> {
    try_call(backend, "setDefault", branch)
}

/// `fin try keep|drop`: forgets the try; when it was the default, the default it replaced is
/// restored by the server.
pub fn clear_try(backend: &dyn Backend, branch: &str) -> Result<SelfState, CoreError> {
    try_call(backend, "clear", branch)
}

/// The id a project folder's `finisher.project.json` names, if it can be read.
pub fn folder_project_id(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("finisher.project.json")).ok()?;
    let doc: Value = serde_json::from_str(&text).ok()?;
    doc.get("id").and_then(Value::as_str).map(str::to_string)
}

/// `PROJECT_NOT_FOUND` for a folder that does not name the session's project (another project's,
/// or no readable `finisher.project.json`): a session works on its own project only (spec §5.6),
/// and the answer does not say what the folder is.
pub fn check_folder(dir: Option<&Path>, session: &SelfState) -> Result<(), CoreError> {
    let Some(dir) = dir else {
        return Ok(());
    };
    match folder_project_id(dir) {
        Some(id) if id == session.project_id => Ok(()),
        _ => Err(CoreError::new(
            "PROJECT_NOT_FOUND",
            "this agent session works on its own project only",
        )
        .with_details(json!({ "dir": dir.display().to_string() }))
        .with_fix("run fin from the session's workspace, without --project")),
    }
}
