//! The self-sandbox entry (Decision 9a). A sandbox that a process must apply to itself after
//! `exec` (seccomp on Linux, Seatbelt `sandbox_init` on macOS) is requested by the host's
//! `Spawner` through [`ENV`] (JSON, no secret) and applied by the function sub-project 7's
//! `fin-sandbox` registers here, before `hello`. It fails closed: a request with no function
//! registered, an unreadable request, or a function that fails makes the process exit
//! [`crate::EXIT_SELF_SANDBOX`] before `hello`.

use std::sync::OnceLock;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The variable that carries a [`SelfSandbox`] request.
pub const ENV: &str = "FIN_SELF_SANDBOX";

/// A requested self-sandbox: a profile name and its arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SelfSandbox {
    pub profile: String,
    #[serde(default)]
    pub args: Value,
}

/// Applies a sandbox to the calling process.
pub type ApplyFn = fn(&SelfSandbox) -> Result<(), String>;

static APPLY: OnceLock<ApplyFn> = OnceLock::new();

/// Registers the apply function: once, in `main`, before `run()`. A later call is ignored.
pub fn register(f: ApplyFn) {
    let _ = APPLY.set(f);
}

/// Applies the request `raw` (the value of [`ENV`], if set). `Ok(None)`: nothing requested;
/// `Ok(Some(profile))`: applied; `Err`: the process must exit 78.
pub(crate) fn enter(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw else { return Ok(None) };
    let req: SelfSandbox =
        serde_json::from_str(raw).map_err(|e| format!("unreadable {ENV}: {e}"))?;
    let apply = APPLY.get().ok_or_else(|| {
        format!(
            "{ENV} requests {:?} but no sandbox is registered",
            req.profile
        )
    })?;
    apply(&req).map_err(|e| format!("self-sandbox {:?} failed: {e}", req.profile))?;
    Ok(Some(req.profile))
}

/// The request in this process's environment.
pub(crate) fn from_env() -> Option<String> {
    std::env::var(ENV).ok()
}
