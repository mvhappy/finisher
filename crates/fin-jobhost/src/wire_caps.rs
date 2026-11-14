//! The job host's capabilities on the wire (`caps()`, `server.status`'s `jobs.caps`): split
//! from `wire.rs` to keep each file small.

use schemars::JsonSchema;
use serde::Serialize;

use crate::HostCaps;
use crate::wire::JobHostKillTree;

/// What this host can enforce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobHostCaps {
    pub kill_tree: JobHostKillTree,
    pub watchdog: bool,
    /// Group memory limits are enforced by the kernel (cgroup `memory.max` or a Job).
    pub cgroup_memory: bool,
    /// Per-process limits are enforced (leaf Jobs or leaf cgroups).
    pub leaf_limits: bool,
    /// The spawner's name (`none` unsandboxed).
    pub sandbox: String,
    /// Every profile is enforced.
    pub sandboxed: bool,
    /// What each sandbox profile gets (plan 7C Decision 4).
    pub profiles: JobHostProfiles,
    /// Warm workers can be granted per job (plan 7C Task 12): warm kinds run warm under an
    /// enforced profile only then.
    pub per_job_grants: bool,
}

/// A sandbox profile's state on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobHostProfileState {
    Enforced,
    Disabled,
    Unavailable,
}

/// [`JobHostProfileState`] per profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobHostProfiles {
    pub worker: JobHostProfileState,
    pub plugin: JobHostProfileState,
}

impl From<crate::spawner::ProfileState> for JobHostProfileState {
    fn from(s: crate::spawner::ProfileState) -> Self {
        match s {
            crate::spawner::ProfileState::Enforced => JobHostProfileState::Enforced,
            crate::spawner::ProfileState::Disabled => JobHostProfileState::Disabled,
            crate::spawner::ProfileState::Unavailable => JobHostProfileState::Unavailable,
        }
    }
}

impl From<HostCaps> for JobHostCaps {
    fn from(c: HostCaps) -> JobHostCaps {
        JobHostCaps {
            kill_tree: match c.kill_tree {
                crate::KillTree::JobObject => JobHostKillTree::JobObject,
                crate::KillTree::Cgroup => JobHostKillTree::Cgroup,
                crate::KillTree::ProcessGroup => JobHostKillTree::ProcessGroup,
            },
            watchdog: c.watchdog,
            cgroup_memory: c.cgroup_memory,
            leaf_limits: c.leaf_limits,
            sandbox: c.sandbox.to_owned(),
            sandboxed: c.sandboxed,
            profiles: JobHostProfiles {
                worker: c.profiles.worker.into(),
                plugin: c.profiles.plugin.into(),
            },
            per_job_grants: c.per_job_grants,
        }
    }
}
