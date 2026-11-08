//! The self-sandbox entry (plan 7C Decision 3): the function `fin-workers` registers with
//! `fin_jobs::sandbox::register`, run before `hello` when the host's `Spawner` asks for it
//! through `FIN_SELF_SANDBOX`. Only `worker` is applied this way; the request's `args` are a
//! [`WorkerInput`], from which [`crate::profile::worker`] builds the policy. Every failure
//! is an `Err`, which `fin-jobs` turns into exit 78 before `hello` (fail closed).

use fin_jobs::sandbox::SelfSandbox;

use crate::inputs::WorkerInput;
use crate::policy::SandboxPolicy;

/// Builds the requested profile's policy and applies it to the calling process.
pub fn self_apply(req: &SelfSandbox) -> Result<(), String> {
    apply(&policy(req)?)
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

#[cfg(not(target_os = "linux"))]
fn apply(_policy: &SandboxPolicy) -> Result<(), String> {
    // macOS `sandbox_init` is Task 8; Windows workers get their token from the `Spawner`.
    Err(format!(
        "no self-sandbox is implemented on {:?} yet",
        crate::policy::Os::current()
    ))
}
