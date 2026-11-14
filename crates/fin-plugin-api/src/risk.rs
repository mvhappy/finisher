//! The risks people see before granting permissions (spec §4.3, 8A Decision 5), computed
//! over the whole set so that combinations count.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::permission::{FsAccess, Permission};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum Risk {
    Low,
    Medium,
    High,
}

/// The risk of `p` inside the set `all`:
/// - low: `project:read`, `media:read`, `fs pluginData read`;
/// - medium: `project:write`, `jobs:submit`, `fs pluginData readWrite`, `gpu` (kernel GPU
///   drivers become reachable), `hooks:observe` without `net`;
/// - high: `net`, `secrets`, `media:file`, `hooks:modify`, and `hooks:observe` with `net`
///   (hook payloads carry prompts, commands and ops, and `net` can send them away).
pub fn risk_of(p: &Permission, all: &[Permission]) -> Risk {
    let has_net = all.iter().any(|q| matches!(q, Permission::Net { .. }));
    match p {
        Permission::ProjectRead | Permission::MediaRead => Risk::Low,
        Permission::Fs {
            access: FsAccess::Read,
            ..
        } => Risk::Low,
        Permission::ProjectWrite | Permission::JobsSubmit | Permission::Gpu => Risk::Medium,
        Permission::Fs {
            access: FsAccess::ReadWrite,
            ..
        } => Risk::Medium,
        Permission::HooksObserve { .. } if has_net => Risk::High,
        Permission::HooksObserve { .. } => Risk::Medium,
        Permission::Net { .. }
        | Permission::Secrets { .. }
        | Permission::MediaFile
        | Permission::HooksModify { .. } => Risk::High,
    }
}

/// Each permission with its risk in the set, in manifest order.
pub fn risks(perms: &[Permission]) -> Vec<(Permission, Risk)> {
    perms
        .iter()
        .map(|p| (p.clone(), risk_of(p, perms)))
        .collect()
}

/// The plugin's risk: the highest of its permissions, at least medium when it contributes
/// agent-facing content (spec §4.2), low with nothing.
pub fn overall(perms: &[Permission], agent_facing: bool) -> Risk {
    let floor = if agent_facing {
        Risk::Medium
    } else {
        Risk::Low
    };
    perms
        .iter()
        .map(|p| risk_of(p, perms))
        .fold(floor, Risk::max)
}
