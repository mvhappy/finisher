//! `library.scan` (sub-project 5 spec §3.4, 5D plan Decision 50): one per full scan, targeted
//! scan, watch rescan, and poll that found changes (a poll that found nothing is only counted in
//! `library.status`, so a 60 s poll does not write 1,440 events a day). Emitted by `finisherd`.
//!
//! No field names a library: no id, name, root, alias or path. Sizes are log₂ buckets
//! ([`log2_bucket`](super::plugin::log2_bucket)); the change counts are plain numbers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;
use crate::events::class::Code;

use super::jobs::JobOutcome;

/// `library.scan` (`kv` 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryScan {
    pub storage: LibraryStorageName,
    /// A full scan (else a poll, a targeted scan or a watch rescan).
    pub full: bool,
    pub trigger: ScanTrigger,
    pub outcome: JobOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
    /// The catalog's live files after the scan, as a log₂ bucket.
    pub files: u8,
    /// The catalog's live directories after the scan, as a log₂ bucket.
    pub dirs: u8,
    pub added: u32,
    pub changed: u32,
    pub missing: u32,
    /// Rows that kept their history under a new path (renames by file id).
    pub moved: u32,
    /// Catalogued cloud-only files and directories after the scan.
    pub placeholders: u32,
    pub micros: u64,
}
approx_struct!(LibraryScan {
    storage,
    full,
    trigger,
    outcome,
    code,
    files,
    dirs,
    added,
    changed,
    missing,
    moved,
    placeholders,
    micros
});

/// A library's effective storage kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LibraryStorageName {
    Local,
    Network,
    Sync,
}

/// What started a scan: `add` (the first scan), `schedule` (the poll cadence or the full-scan
/// schedule), `demand` (someone asked, a freshness pass, a file found changed), `watch` (a local
/// root's watcher), `mount` (the root came back online).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ScanTrigger {
    Add,
    Schedule,
    Demand,
    Watch,
    Mount,
}
