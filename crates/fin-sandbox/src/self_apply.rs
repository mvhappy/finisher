//! The self-sandbox entry (plan 7C Decision 3): the function `fin-workers` registers with
//! `fin_jobs::sandbox::register`, run before `hello` when the host's `Spawner` asks for it
//! through `FIN_SELF_SANDBOX`. Only `worker` is applied this way; the request's `args` are a
//! [`WorkerInput`], from which [`crate::profile::worker`] builds the policy. Every failure
//! is an `Err`, which `fin-jobs` turns into exit 78 before `hello` (fail closed). The one
//! exception is F2: where the mechanism is disabled on this host ([`crate::status::mechanism`]
//! says `Disabled {landlockDisabled}`), nothing is applied and the worker's `hello` reports
//! `selfSandbox: "none"`, so it runs, unconfined and visibly so.

use std::sync::atomic::{AtomicBool, Ordering};

use fin_jobs::sandbox::{SelfApplied, SelfSandbox};

use crate::inputs::WorkerInput;
use crate::policy::SandboxPolicy;
use crate::status::{DisabledReason, Mechanism, mechanism};

static APPLIED: AtomicBool = AtomicBool::new(false);

/// Builds the requested profile's policy and applies it to the calling process.
pub fn self_apply(req: &SelfSandbox) -> Result<SelfApplied, String> {
    let policy = policy(req)?;
    match mechanism() {
        Mechanism::Disabled {
            reason: DisabledReason::LandlockDisabled,
        } => return Ok(SelfApplied::Nothing),
        Mechanism::Disabled { reason } => {
            return Err(format!("no self-sandbox on this host ({reason:?})"));
        }
        Mechanism::Unavailable { need } => return Err(format!("the sandbox needs {need}")),
        Mechanism::Enforced { .. } => {}
    }
    apply(&policy)?;
    APPLIED.store(true, Ordering::SeqCst);
    Ok(SelfApplied::Applied)
}

/// Whether [`self_apply`] restricted this process (`media.capabilities.sandbox`).
pub fn applied() -> bool {
    APPLIED.load(Ordering::SeqCst)
}

/// The policy a request names, built and validated.
pub fn policy(req: &SelfSandbox) -> Result<SandboxPolicy, String> {
    match req.profile.as_str() {
        "worker" => {
            let input: WorkerInput = serde_json::from_value(req.args.clone())
                .map_err(|e| format!("unreadable worker input: {e}"))?;
            crate::profile::worker(&input).map_err(|e| e.to_string())
        }
        other => Err(format!(
            "the {other:?} profile is not applied by the process itself"
        )),
    }
}

#[cfg(target_os = "linux")]
fn apply(policy: &SandboxPolicy) -> Result<(), String> {
    crate::linux::apply::restrict_self(policy)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn apply(policy: &SandboxPolicy) -> Result<(), String> {
    // The worker keeps running its own binary.
    let exe = std::env::current_exe().map_err(|e| format!("the worker's own binary: {e}"))?;
    crate::macos::apply::restrict_self(policy, Some(&exe)).map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn apply(_policy: &SandboxPolicy) -> Result<(), String> {
    // Windows workers get their token from the `Spawner` (Task 9).
    Err(format!(
        "no self-sandbox is implemented on {:?} yet",
        crate::policy::Os::current()
    ))
}
