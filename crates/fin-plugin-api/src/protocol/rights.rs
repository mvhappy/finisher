//! A plugin job's rights (plan 8B Decision 8; spec §3.7): the invoker's principal intersected
//! with the plugin's grants, computed once by the host when the job is submitted and stored
//! with it (5C's `jobs.rights`). The host checks them again on every call against the live
//! principal, so a revoked or narrowed invoker narrows the next call.
//!
//! The scopes a plugin can ever hold are a closed set ([`PluginScope`]): the four of spec
//! §3.7's mapping. No human-only scope, `branch.protected`, `review.decide`, `project.admin`,
//! `auth.admin` or `diagnostics.*` is representable here.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::permission::Permission;

/// A sub-project 5 scope a plugin job may hold, each from one permission (spec §3.7):
/// `project:read` → `project.read`; `project:write` → `project.write` (only the `tx.commit` of
/// the command's returned transaction); `media:read` → `library.read` (and asset reads);
/// `jobs:submit` → `jobs.submit` (the plugin's own kinds only, 8C).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum PluginScope {
    #[serde(rename = "project.read")]
    ProjectRead,
    #[serde(rename = "project.write")]
    ProjectWrite,
    #[serde(rename = "library.read")]
    LibraryRead,
    #[serde(rename = "jobs.submit")]
    JobsSubmit,
}

impl PluginScope {
    pub const ALL: [PluginScope; 4] = [
        PluginScope::ProjectRead,
        PluginScope::ProjectWrite,
        PluginScope::LibraryRead,
        PluginScope::JobsSubmit,
    ];

    /// The scope's name in sub-project 5's vocabulary.
    pub fn name(self) -> &'static str {
        match self {
            PluginScope::ProjectRead => "project.read",
            PluginScope::ProjectWrite => "project.write",
            PluginScope::LibraryRead => "library.read",
            PluginScope::JobsSubmit => "jobs.submit",
        }
    }

    /// The permission that grants it.
    pub fn permission(self) -> &'static str {
        match self {
            PluginScope::ProjectRead => "project:read",
            PluginScope::ProjectWrite => "project:write",
            PluginScope::LibraryRead => "media:read",
            PluginScope::JobsSubmit => "jobs:submit",
        }
    }
}

/// `"all"`: every project (or library) of this server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ReachAll {
    #[serde(rename = "all")]
    All,
}

/// Which projects or libraries a job reaches: `"all"` or a list of ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Reach {
    All(ReachAll),
    Only(Vec<String>),
}

impl Reach {
    pub fn includes(&self, id: &str) -> bool {
        match self {
            Reach::All(_) => true,
            Reach::Only(ids) => ids.iter().any(|i| i == id),
        }
    }
}

/// The invoking principal (sub-project 5's `{kind, id}`; the job's owner).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RightsPrincipal {
    pub kind: String,
    pub id: String,
}

/// An agent session's branch reach (sub-project 7 §5.1): its root branch and, with `subtree`,
/// the branches beneath it (less its live children's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RightsBranch {
    pub root: String,
    pub subtree: bool,
}

/// A plugin job's rights: invoker ∩ grants (Decision 8), with the authority they run under
/// (class + `rights_hash` + `generation`, spec §3.3 and Decision 28).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobRights {
    /// The invoker the plugin acts for (audit and journal name `plugin:<id>@<version>` acting
    /// for it).
    pub principal: RightsPrincipal,
    /// The principal class of the process key (spec §3.3): `person`, `agent:<root session>`,
    /// `system`, or `<kind>:<id>` for any other invoker.
    pub class: String,
    /// The invoker's scopes that the plugin's grants map to.
    pub scopes: Vec<PluginScope>,
    pub projects: Reach,
    /// Set for an agent session: its branch subtree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<RightsBranch>,
    pub libraries: Reach,
    /// The plugin's granted permissions at submit.
    pub grants: Vec<Permission>,
    /// The hash of the plugin-relevant rights above (class, scopes, projects, branch, libraries,
    /// grants; not the principal, so callers of one class with identical rights share it): 32
    /// lower-case hex characters, computed by the host (spec §3.3's authority key, Decision 28).
    pub rights_hash: String,
    /// The class's policy generation when the rights were computed (spec §3.3): it increments
    /// when a credential of the class is narrowed or revoked or its agent session tree changes.
    pub generation: u64,
}
