//! What kind of storage a project folder lives on (sub-project 4 spec §3.5):
//! `local | network | sync(provider)`. The kind picks the lease parameters, the poll cadence,
//! the compaction grace period and whether chain resolution may apply the epoch rule.
//!
//! Detection (`detect`) arrives with plan Task 10; chain resolution (Task 6) needs only the
//! enum.

use crate::conflict_names::Provider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    /// A local disk.
    Local,
    /// A network share (SMB, NFS, AFP, WebDAV).
    Network,
    /// A sync folder (iCloud Drive, Dropbox, OneDrive, Google Drive, Syncthing). Two offline
    /// writers can legitimately hold different epochs, so diverged lines are never resolved
    /// automatically.
    Sync(Provider),
}

impl StorageKind {
    pub fn is_sync(self) -> bool {
        matches!(self, StorageKind::Sync(_))
    }
}
