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
    /// updates the cache and queues the event when it changed. Needs the writer lease, as
    /// any folder write: the cached state, then a full lease check before the rewrite and
    /// the in-memory one before the rename (`PROJECT_READ_ONLY`, reason `LEASE_LOST`, when
    /// the lease went since the last tick).
    pub fn set_collect(&mut self, policy: CollectPolicy) -> Result<(), StoreError> {
        if let Some(reason) = self.read_only() {
            return Err(crate::import::read_only_error(&self.project_id, reason));
        }
        let lost =
            |s: &Store, e: &fin_core::CoreError| crate::import::lease_lost_error(&s.project_id, e);
        // A folder write takes a full check (the cached state may be a tick old).
        let epoch = self.export.lease.check().map_err(|e| lost(self, &e))?;
        let mut f = project_file::read_project_file(&self.project_dir)?;
        f.collect = match policy {
            CollectPolicy::Never => Some(Value::String("never".into())),
            CollectPolicy::Default => None,
        };
        let path = crate::paths::project_file(&self.project_dir);
        let mut bytes = serde_json::to_vec_pretty(&f)?;
        bytes.push(b'\n');
        let tmp = self.project_dir.join(crate::names::temp_name());
        let io = |e: std::io::Error| crate::unwritable(&path)(e);
        let written = (|| -> Result<(), StoreError> {
            let mut file = std::fs::File::create(&tmp).map_err(io)?;
            std::io::Write::write_all(&mut file, &bytes).map_err(io)?;
            crate::fsio::sync_file(&file).map_err(io)?;
            drop(file);
            match self.export.lease.quick_check() {
                Ok(e) if e == epoch => {}
                Ok(e) => {
                    let cause = fin_core::CoreError::new(
                        "LEASE_LOST",
                        format!("the lease changed from epoch {epoch} to {e} during the write"),
                    );
                    return Err(lost(self, &cause));
                }
                Err(e) => return Err(lost(self, &e)),
            }
            std::fs::rename(&tmp, &path).map_err(io)?;
            crate::fsio::sync_dir(&self.project_dir).map_err(io)?;
            Ok(())
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written?;
        self.observe_project_file(&f);
        Ok(())
    }
}
