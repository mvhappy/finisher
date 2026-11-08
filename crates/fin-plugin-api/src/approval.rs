//! Whether an update needs a person's approval again (spec §4.3 "widening", §5.3): wider
//! permissions, any change to agent-facing content, or another kind or runtime. The permission
//! part alone is [`PermissionDiff::permissions_widen`].

use serde::{Deserialize, Serialize};

use crate::contribution::ContributionKind;
use crate::manifest::{Manifest, Runtime};
use crate::permission::{PermissionDiff, diff};

/// One version of a plugin as a person approved it, or as an update offers it.
#[derive(Debug, Clone, Copy)]
pub struct Approval<'a> {
    pub manifest: &'a Manifest,
    /// Lowercase hex SHA-256 over the agent-facing content (skills, prompts, roles, rubrics) as
    /// packaged, computed by the caller (this crate does no I/O); `None` when it has none.
    pub agent_content_sha256: Option<&'a str>,
}

/// Why an update needs approval again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reapproval {
    /// [`PermissionDiff::permissions_widen`].
    Permissions,
    /// Agent-facing content was added or changed: an update can change what agents are told.
    AgentContent,
    /// `content` ↔ `code`.
    Kind,
    /// A runtime was added or removed, or its type changed (`node` ↔ `native`).
    Runtime,
}

/// Whether `m` contributes agent-facing content, typed or not (a kind this host does not
/// consume yet still counts: the approval must not depend on what this build parses).
fn agent_facing(m: &Manifest) -> bool {
    m.agent_facing()
        || m.contributes
            .unsupported()
            .iter()
            .any(|k| ContributionKind::from_key(k).is_some_and(ContributionKind::agent_facing))
}

fn runtime_type(r: &Option<Runtime>) -> Option<&'static str> {
    r.as_ref().map(|r| match r {
        Runtime::Native { .. } => "native",
        Runtime::Node { .. } => "node",
    })
}

/// Every reason `new` needs a person's approval over `old`, in [`Reapproval`] order; empty when
/// it only narrows or keeps what was approved. Removing agent-facing content needs no approval.
pub fn reapproval_reasons(old: &Approval, new: &Approval) -> Vec<Reapproval> {
    let mut out = Vec::new();
    let d: PermissionDiff = diff(&old.manifest.permissions, &new.manifest.permissions);
    if d.permissions_widen() {
        out.push(Reapproval::Permissions);
    }
    if agent_facing(new.manifest) {
        let same = agent_facing(old.manifest)
            && new.agent_content_sha256.is_some()
            && old.agent_content_sha256 == new.agent_content_sha256;
        if !same {
            out.push(Reapproval::AgentContent);
        }
    }
    if old.manifest.kind != new.manifest.kind {
        out.push(Reapproval::Kind);
    }
    if runtime_type(&old.manifest.runtime) != runtime_type(&new.manifest.runtime) {
        out.push(Reapproval::Runtime);
    }
    out
}

/// Whether `new` needs a person's approval over the approved `old` (8B's update rule).
pub fn requires_reapproval(old: &Approval, new: &Approval) -> bool {
    !reapproval_reasons(old, new).is_empty()
}
