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
        if policy == Some(CollectPolicy::Never) && before != Some(Some(CollectPolicy::Never)) {
            self.pending_purges.insert(id);
            self.retry_purges();
        }
    }

    /// Runs the `never` purges not done yet; one that fails suspends the journal and stays
    /// pending, retried from `maintain` (each flush and tick) once the suspension's retry is
    /// due.
    pub fn retry_purges(&mut self) {
        while self.suspended.is_none()
            && let Some(id) = self.pending_purges.first().cloned()
        {
            match self.purge(&PurgeSelector::Project(id.clone())) {
                Ok(_) => {
                    self.pending_purges.remove(&id);
                }
                Err(e) => self.suspend(&e),
            }
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

/// A project open on this server, for the journal: the policy its file states while at least
/// one scope of it is alive, unknown once the last one drops. Engines that share a project
/// (two windows, a follower beside its leader) each hold one, so closing one does not make the
/// project unknown for the other. [`ProjectScope::update`] follows the `collect` event.
#[derive(Debug)]
pub struct ProjectScope {
    journal: Journal,
    id: Option<String>,
}

impl ProjectScope {
    /// The project's flag changed (`collect` event); every scope of it sees the same file.
    pub fn update(&self, policy: CollectPolicy) {
        if let (Some(h), Some(id)) = (&self.journal.h, &self.id) {
            h.control(Control::Project {
                id: id.clone(),
                policy: Some(policy),
            });
        }
    }

    /// The project id, lower-cased, or `None` when it was not one (the scope does nothing).
    pub fn project(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

impl Drop for ProjectScope {
    fn drop(&mut self) {
        let (Some(h), Some(id)) = (&self.journal.h, &self.id) else {
            return;
        };
        let mut scopes = h.sh.scopes.lock().unwrap_or_else(|e| e.into_inner());
        let last = match scopes.get_mut(id) {
            Some(n) if *n > 1 => {
                *n -= 1;
                false
            }
            Some(_) => {
                scopes.remove(id);
                true
            }
            None => false,
        };
        if last {
            h.control(Control::Project {
                id: id.clone(),
                policy: None,
            });
        }
    }
}

impl Journal {
    /// Opens `project` for the journal under `policy` (its file's flag at engine open) and
    /// returns the scope that keeps it known; see [`ProjectScope`]. Setting `never` purges
    /// as [`Journal::set_project_policy`] does.
    pub fn open_project(&self, project: &str, policy: CollectPolicy) -> ProjectScope {
        let id = self.h.as_ref().and(project_key(project));
        if let (Some(h), Some(id)) = (&self.h, &id) {
            let mut scopes = h.sh.scopes.lock().unwrap_or_else(|e| e.into_inner());
            *scopes.entry(id.clone()).or_default() += 1;
            h.control(Control::Project {
                id: id.clone(),
                policy: Some(policy),
            });
        }
        ProjectScope {
            journal: self.clone(),
            id,
        }
    }
}
