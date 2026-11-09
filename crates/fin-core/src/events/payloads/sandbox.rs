//! `sandbox.unconfined` (sub-project 7 spec §8.9, plan 7C Decision 23): a sandbox profile runs
//! unconfined on this host. Emitted by `finisherd` once per profile per server start, and again
//! when the reason changes. Metadata only: no path, no host name, no process.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;

/// `sandbox.unconfined` (`kv` 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SandboxUnconfined {
    pub profile: SandboxProfileName,
    pub os: SandboxOs,
    pub reason: UnconfinedReason,
}
approx_struct!(SandboxUnconfined {
    profile,
    os,
    reason
});

/// A sandbox profile (spec §8.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SandboxProfileName {
    Agent,
    Hook,
    Worker,
    Plugin,
}

/// The operating system a sandbox applies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SandboxOs {
    Linux,
    Macos,
    Windows,
}

/// Why a profile runs unconfined (F2: both run, with persistent warnings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum UnconfinedReason {
    /// The OS mechanism is off on this host (Linux: `landlock` left out of `lsm=`, or a
    /// container's seccomp profile refusing the Landlock calls).
    LandlockDisabled,
    /// The profile's escape suite has not yet passed on this OS (`fin_sandbox::READY`).
    ProfileNotReady,
}
