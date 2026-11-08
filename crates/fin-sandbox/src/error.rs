//! Why a policy or a profile input was refused.

use std::path::PathBuf;

use crate::policy::Profile;

/// A policy that cannot be built or is not well formed. Every variant refuses the start: the
/// sandbox fails closed (plan 7C Global Constraints), and the caller reports it as
/// `SANDBOX_UNAVAILABLE {profile, os, need}` with [`PolicyError::to_string`] as the detail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// A path is relative. `field` is the policy field or the input field (camelCase).
    #[error("{field}: {} is not absolute", path.display())]
    NotAbsolute { field: String, path: PathBuf },
    /// A path holds `.` or `..`, or a repeated or trailing separator. Callers canonicalise
    /// (links resolved) before building inputs; the policy never resolves a link itself.
    #[error("{field}: {} is not normalised", path.display())]
    NotNormalised { field: String, path: PathBuf },
    /// A write root equals a protected root or lies inside one.
    #[error("write root {} is inside the protected root {}", write.display(), protect.display())]
    WriteUnderProtect { write: PathBuf, protect: PathBuf },
    /// The same path is both granted (`read` or `write`) and in `deny_read`.
    #[error("{field}: {} is both granted and denied", path.display())]
    GrantedAndDenied { field: String, path: PathBuf },
    /// A device rule at, under or above a denied or protected root (device rules are granted
    /// as they are, not enumerated, so the overlap would re-grant the root).
    #[error("device {} overlaps the denied or protected root {}", device.display(), root.display())]
    DeviceOverlaps { device: PathBuf, root: PathBuf },
    /// A rule the profile does not allow (`net`, `unix_sockets`, `devices`, `limits`, `mach`).
    #[error("profile {profile:?}: {reason}")]
    Profile { profile: Profile, reason: String },
    /// A profile input that is inconsistent (a workspace outside its project, a toolchain
    /// inside server data, a memory limit above the cap, …).
    #[error("{field}: {reason}")]
    Input { field: String, reason: String },
    /// A directory the enumeration had to list could not be listed.
    #[error("{} could not be listed: {error}", path.display())]
    Enumerate { path: PathBuf, error: String },
}

impl PolicyError {
    pub(crate) fn input(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Input {
            field: field.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn profile(profile: Profile, reason: impl Into<String>) -> Self {
        Self::Profile {
            profile,
            reason: reason.into(),
        }
    }
}

/// A policy that could not be applied to a process. Every variant but [`ApplyError::Disabled`]
/// refuses the start as `SANDBOX_UNAVAILABLE {profile, os, need}`; `Disabled` is F2's "the
/// mechanism is off on this host" (spec §8.9), which the caller reports and decides on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    /// The OS mechanism is turned off (Linux: Landlock left out of `lsm=`, or a container's
    /// seccomp profile refusing the Landlock calls).
    #[error("the sandbox mechanism is disabled on this host ({reason})")]
    Disabled { reason: String },
    /// The OS lacks what the profile needs (Linux: Landlock ABI below 3).
    #[error("the sandbox needs {need}")]
    Unavailable { need: String },
    #[error(transparent)]
    Policy(#[from] PolicyError),
    /// A rule could not be built for a path that exists.
    #[error("{}: {error}", path.display())]
    Rule { path: PathBuf, error: String },
    /// The process has threads the restriction would not reach.
    #[error("the process has {threads} threads; the sandbox must be applied while it has one")]
    Threads { threads: usize },
    /// A call of the mechanism failed.
    #[error("{call}: {error}")]
    Os { call: String, error: String },
    /// The profile is not applied this way on this OS (yet).
    #[error("{0}")]
    NotSupported(String),
}
