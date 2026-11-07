//! The project flag (plan D, Decision 13): each project's collection policy as this server
//! last read it from `finisher.project.json`. `default` follows the user's settings; `never`
//! drops the project's entries and purges what it has; a project not known here (not open,
//! or closed) is unknown: its new entries are dropped and nothing is purged. The writer
//! keeps the map in channel order, so an entry recorded before a change is judged by the
//! policy in force when it was recorded.

use std::collections::BTreeMap;

use fin_core::events::{CollectPolicy, ProjectId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Journal;
use crate::control::Control;
use crate::purge::PurgeSelector;
use crate::writer::Writer;

/// A project's policy as `fin diagnostics show` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProjectPolicy {
    Default,
    Never,
    Unknown,
}

impl From<Option<CollectPolicy>> for ProjectPolicy {
    fn from(p: Option<CollectPolicy>) -> ProjectPolicy {
        match p {
            Some(CollectPolicy::Default) => ProjectPolicy::Default,
            Some(CollectPolicy::Never) => ProjectPolicy::Never,
            None => ProjectPolicy::Unknown,
        }
    }
}

/// A project id as the journal compares it: lower case, or `None` when it is not one.
pub(crate) fn project_key(raw: &str) -> Option<String> {
    let p = raw.to_ascii_lowercase();
    ProjectId::parse(&p).map(|_| p)
}

impl Writer {
    /// Whether an entry of `project` may be written now.
    pub fn project_allowed(&self, project: &str) -> bool {
        self.policies.get(project) == Some(&Some(CollectPolicy::Default))
    }

    pub fn on_project(&mut self, id: String, policy: Option<CollectPolicy>) {
        let before = self.policies.insert(id.clone(), policy);
        self.publish_policies();
        if policy == Some(CollectPolicy::Never)
            && before != Some(Some(CollectPolicy::Never))
            && let Err(e) = self.purge(&PurgeSelector::Project(id))
        {
            self.suspend(&e);
        }
    }

    pub fn publish_policies(&self) {
        let mut s = self.sh.lock_status();
        s.policies = self.policies.iter().map(|(k, v)| (k.clone(), *v)).collect();
    }
}

impl Journal {
    /// Sets what this server knows of `project`'s flag: `Some` from the project file at open
    /// and from each `collect` event, `None` (unknown) when the project closes. Setting
    /// `never` purges the project's entries and their blobs (also while off: a purge only
    /// deletes). An id that is not a project id is ignored; upper case is lowered.
    pub fn set_project_policy(&self, project: &str, policy: Option<CollectPolicy>) {
        let (Some(h), Some(id)) = (&self.h, project_key(project)) else {
            return;
        };
        h.control(Control::Project { id, policy });
    }

    /// Every project the journal was told about, with its policy now (as of the last
    /// message the writer handled).
    pub fn project_policies(&self) -> BTreeMap<String, ProjectPolicy> {
        let Some(h) = &self.h else {
            return BTreeMap::new();
        };
        h.sh.lock_status()
            .policies
            .iter()
            .map(|(k, v)| (k.clone(), ProjectPolicy::from(*v)))
            .collect()
    }
}
