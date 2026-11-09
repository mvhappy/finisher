//! The ACL broker's napi surface (plan 7C Task 10; [`crate::acl_broker`]): `aclGrant`,
//! `aclRevoke`, `aclSweep` run on the libuv pool (a grant on a large tree propagates for
//! tens of milliseconds), one broker per server data directory (links resolved), opened by
//! the first call. `sandboxAgentGrants`/`sandboxHookGrants` answer a launch's grant table
//! (`fin_sandbox::windows::grants`, pure on every OS) and `sandboxContainerSid` a container's
//! SID (Windows; `null` elsewhere). Off Windows the broker logs but applies nothing.

// napi registers the exports; a `cargo test` build has no caller.
#![cfg_attr(test, allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use fin_core::CoreError;
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::acl_broker::{Broker, BrokerError, Grant, Options, apply::Platform};
use crate::{encode, guarded, reject};

type PlatformBroker = Broker<Platform>;

static BROKERS: OnceLock<Mutex<HashMap<PathBuf, Arc<PlatformBroker>>>> = OnceLock::new();

fn core(e: BrokerError) -> CoreError {
    CoreError::new(e.code(), e.to_string()).with_details(e.details())
}

/// The broker of `server_data`, opened (and its log replayed with `live`) on first use.
fn broker(server_data: &str, live: &HashSet<String>) -> Result<Arc<PlatformBroker>> {
    #[allow(clippy::disallowed_methods)] // server data, never a library path
    let key = std::fs::canonicalize(server_data).unwrap_or_else(|_| PathBuf::from(server_data));
    let mut all = BROKERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(b) = all.get(&key) {
        return Ok(b.clone());
    }
    let b = Broker::open(
        std::path::Path::new(server_data),
        Platform::default(),
        Options::default(),
        &|o| live.contains(o),
    )
    .map_err(|e| reject(core(e)))?;
    let b = Arc::new(b);
    all.insert(key, b.clone());
    Ok(b)
}

pub enum AclOp {
    Grant(Grant),
    Revoke {
        owner: String,
        path: Option<PathBuf>,
    },
    Sweep(HashSet<String>),
}

pub struct AclTask {
    server_data: String,
    op: AclOp,
}

impl Task for AclTask {
    type Output = u32;
    type JsValue = u32;

    fn compute(&mut self) -> Result<u32> {
        guarded(
            || {
                let none = HashSet::new();
                let live = match &self.op {
                    AclOp::Sweep(l) => l,
                    _ => &none,
                };
                let b = broker(&self.server_data, live)?;
                let n = match &self.op {
                    AclOp::Grant(g) => b.grant(g).map(|()| 0),
                    AclOp::Revoke { owner, path } => b.revoke(owner, path.as_deref()),
                    AclOp::Sweep(live) => b.sweep(live),
                }
                .map_err(|e| reject(core(e)))?;
                Ok(u32::try_from(n).unwrap_or(u32::MAX))
            },
            || {},
        )
    }

    fn resolve(&mut self, _env: Env, output: u32) -> Result<u32> {
        Ok(output)
    }
}

/// Grants `sid` `mask` on `path` for `owner` (`inherit`: the subtree inherits it). Logged
/// before it is applied; a protected root is refused first (`PATH_NOT_ALLOWED`).
#[napi(catch_unwind)]
pub fn acl_grant(
    server_data: String,
    path: String,
    sid: String,
    mask: u32,
    inherit: bool,
    owner: String,
) -> AsyncTask<AclTask> {
    AsyncTask::new(AclTask {
        server_data,
        op: AclOp::Grant(Grant {
            path: path.into(),
            sid,
            mask,
            inherit,
            owner,
        }),
    })
}

/// Revokes `owner`'s grants (only those on `path` when given); resolves to how many.
#[napi(catch_unwind)]
pub fn acl_revoke(server_data: String, owner: String, path: Option<String>) -> AsyncTask<AclTask> {
    AsyncTask::new(AclTask {
        server_data,
        op: AclOp::Revoke {
            owner,
            path: path.map(PathBuf::from),
        },
    })
}

/// Revokes every grant whose owner is not in `live` (the static owner always is); resolves
/// to how many. The first call of a server opens the broker, completing what a crash left.
#[napi(catch_unwind)]
pub fn acl_sweep(server_data: String, live: Vec<String>) -> AsyncTask<AclTask> {
    AsyncTask::new(AclTask {
        server_data,
        op: AclOp::Sweep(live.into_iter().collect()),
    })
}

fn grants_json(
    input: &str,
    table: impl FnOnce(&str) -> std::result::Result<Vec<fin_sandbox::windows::grants::AclGrant>, String>,
) -> Result<String> {
    let grants = table(input).map_err(|e| {
        reject(CoreError::new(
            "INVALID_ARGS",
            format!("no grant table: {e}"),
        ))
    })?;
    encode(&grants)
}

/// The grants of an `agent` launch (`AgentInput` JSON): `AclGrant[]` JSON.
#[napi(catch_unwind)]
pub fn sandbox_agent_grants(input: String) -> Result<String> {
    grants_json(&input, |s| {
        let i: fin_sandbox::AgentInput =
            serde_json::from_str(s).map_err(|e| format!("the agent input does not parse: {e}"))?;
        fin_sandbox::windows::grants::agent(&i)
    })
}

/// The grants of a `hook` launch (`HookInput` JSON): `AclGrant[]` JSON.
#[napi(catch_unwind)]
pub fn sandbox_hook_grants(input: String) -> Result<String> {
    grants_json(&input, |s| {
        let i: fin_sandbox::HookInput =
            serde_json::from_str(s).map_err(|e| format!("the hook input does not parse: {e}"))?;
        fin_sandbox::windows::grants::hook(&i)
    })
}

/// The AppContainer name of a project's agents and hooks (`finisher-p-<hash>`, pure).
#[napi(catch_unwind)]
pub fn sandbox_project_container(project_dir: String) -> String {
    fin_sandbox::windows::capability::project_container(std::path::Path::new(&project_dir))
}

/// The SID of the AppContainer `name` (derived; the profile need not exist); `null` off
/// Windows.
#[napi(catch_unwind)]
pub fn sandbox_container_sid(name: String) -> Result<Option<String>> {
    #[cfg(windows)]
    {
        fin_sandbox::windows::appcontainer::sid(&name)
            .map(|s| Some(s.string()))
            .map_err(|e| {
                reject(CoreError::new(
                    "INVALID_ARGS",
                    format!("no container SID for {name:?}: {e}"),
                ))
            })
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Ok(None)
    }
}

/// The broker's live grants (`{path, sid, mask, inherit, owner}[]` JSON). Test hooks only.
#[cfg(feature = "test-hooks")]
#[napi(catch_unwind)]
pub fn acl_grants_for_test(server_data: String) -> Result<String> {
    guarded(
        || encode(&broker(&server_data, &HashSet::new())?.grants()),
        || {},
    )
}
