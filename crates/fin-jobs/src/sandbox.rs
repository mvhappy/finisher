//! The self-sandbox entry (Decision 9a). A sandbox that a process must apply to itself after
//! `exec` (seccomp on Linux, Seatbelt `sandbox_init` on macOS) is requested by the host's
//! `Spawner` through [`ENV`] (JSON, no secret) and applied by the function sub-project 7's
//! `fin-sandbox` registers here, before `hello`. It fails closed: a request with no function
//! registered, an unreadable request, or a function that fails makes the process exit
//! [`crate::EXIT_SELF_SANDBOX`] before `hello`. Where the OS mechanism is disabled (F2, spec
//! §8.9) the function applies nothing and says so ([`SelfApplied::Nothing`]); `hello` then
//! reports [`NONE`] instead of the profile, so the host never takes the process for confined.

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

/// `hello.selfSandbox` of a process that was asked for a sandbox and applied nothing.
pub const NONE: &str = "none";

/// What the apply function did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfApplied {
    /// The requested profile is in force.
    Applied,
    /// The mechanism is disabled on this host (F2): nothing was applied and the process runs
    /// unconfined, with the warnings the server shows.
    Nothing,
}

/// Applies a sandbox to the calling process.
pub type ApplyFn = fn(&SelfSandbox) -> Result<SelfApplied, String>;

static APPLY: OnceLock<ApplyFn> = OnceLock::new();

/// Registers the apply function: once, in `main`, before `run()`. A later call is ignored.
pub fn register(f: ApplyFn) {
    let _ = APPLY.set(f);
}

/// Applies the request `raw` (the value of [`ENV`], if set). `Ok(None)`: nothing requested;
/// `Ok(Some(profile))`: applied; `Ok(Some(NONE))`: requested, nothing applied (F2); `Err`: the
/// process must exit 78.
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
    match apply(&req).map_err(|e| format!("self-sandbox {:?} failed: {e}", req.profile))? {
        SelfApplied::Applied => Ok(Some(req.profile)),
        SelfApplied::Nothing => Ok(Some(NONE.to_owned())),
    }
}

/// The request in this process's environment.
pub(crate) fn from_env() -> Option<String> {
    std::env::var(ENV).ok()
}
