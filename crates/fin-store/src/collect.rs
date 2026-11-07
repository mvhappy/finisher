//! The project's collection flag (plan D, Decision 13): `finisher.project.json` `collect`.
//! `"never"` binds every collaborator's server (the journal records nothing for the project
//! and purges what it has); absent follows each user's settings, and `"default"` is written
//! as absent. The flag lives in the project file, not the op log: it is project-wide, an
//! undo or a merge cannot change it, and it needs no new op. The file already syncs with the
//! folder and is re-read by followers, and older Finishers keep the key on rewrite
//! (`ProjectFile.other`).
//!
//! The store caches the value read at open and from each re-read; a change queues
//! [`StoreEvent::Collect`] for the next tick (fin-node feeds it to the journal). A file that
//! cannot be read changes nothing.

use serde_json::Value;

pub use fin_core::events::CollectPolicy;

use crate::project_file::{self, ProjectFile};
use crate::{Store, StoreError, StoreEvent};

/// The policy a project file states. A value this build does not know reads as `never`:
/// a newer Finisher's stricter flag is honoured, not dropped.
pub fn policy_of(f: &ProjectFile) -> CollectPolicy {
    match &f.collect {
        None => CollectPolicy::Default,
        Some(Value::String(s)) if s == "default" => CollectPolicy::Default,
        Some(_) => CollectPolicy::Never,
    }
}

impl Store {
    /// The project's collection policy as last read (at open, then by every re-read).
    pub fn collect_policy(&self) -> CollectPolicy {
        self.collect
    }

    /// What a read of the project file says: a newer format makes the project read-only
    /// (`NEWER_FORMAT`), and a changed `collect` queues [`StoreEvent::Collect`].
    pub fn observe_project_file(&mut self, f: &ProjectFile) {
        self.file_newer = f.is_newer();
        let p = policy_of(f);
        if p != self.collect {
            self.collect = p;
            self.follow.queued.push(StoreEvent::Collect(p));
        }
    }

    /// Writes the flag into the project file (`never` as `"never"`, `default` as no key),
    /// updates the cache and queues the event when it changed. Needs the writer lease:
    /// `PROJECT_READ_ONLY` otherwise, as for any write.
    pub fn set_collect(&mut self, policy: CollectPolicy) -> Result<(), StoreError> {
        if let Some(reason) = self.read_only() {
            return Err(crate::import::read_only_error(&self.project_id, reason));
        }
        let mut f = project_file::read_project_file(&self.project_dir)?;
        f.collect = match policy {
            CollectPolicy::Never => Some(Value::String("never".into())),
            CollectPolicy::Default => None,
        };
        project_file::write(&self.project_dir, &f)?;
        self.observe_project_file(&f);
        Ok(())
    }
}
