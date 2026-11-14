//! Steps 1–3 of an import pass and the lease-file read (module docs): the folder I/O, run
//! without the store ([`ImportPlan::read`], `crate::tick`'s unlocked phase) between
//! [`Store::import_plan`] and [`Store::import_apply`].

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use fin_core::CoreError;
use serde_json::json;

use super::{ImportReport, hydrate, rewind};
use crate::chain::SegRef;
use crate::export::Fence;
use crate::lease::MonoClock;
use crate::project_file::ProjectFile;
use crate::scan::{Entry, ForeignContent, NotArrivedWhy, Scanner};
use crate::segment::Segment;
use crate::storage_kind::StorageKind;
use crate::{Store, StoreError};

/// The lease-file read's key: the segment hashes listed and the lease held then.
pub(super) type LeaseSeen = (BTreeSet<String>, Option<u64>);

/// What an import pass takes out of the store to read the folder with.
pub(crate) struct ImportPlan {
    project_dir: PathBuf,
    kind: StorageKind,
    scanner: Scanner,
    segrefs: HashMap<String, SegRef>,
    hydrate_from: Option<PathBuf>,
    genesis: String,
    fence: Fence,
    forked: bool,
    lease_seen: Option<LeaseSeen>,
    lease_top: Option<u64>,
    clock: Arc<dyn MonoClock>,
    /// [`crate::tick::Phase::io_gen`] when planned.
    pub(super) io_gen: u64,
    #[cfg(any(test, feature = "test-hooks"))]
    hooks: crate::tick::hooks::TickHooks,
}

/// A foreign file: its path, conflict-copy pattern and, if it decodes as one, the segment.
pub(super) type Foreign = (
    PathBuf,
    Option<crate::conflict_names::Provider>,
    Option<Arc<Segment>>,
);

/// What the folder held (steps 1–3).
pub(super) struct Folder {
    /// The report so far: `hydrated` and `not_arrived`.
    pub(super) report: ImportReport,
    pub(super) by_branch: HashMap<String, Vec<SegRef>>,
    pub(super) present: HashMap<String, SegRef>,
    pub(super) foreign: Vec<Foreign>,
    pub(super) newer: bool,
    pub(super) foreign_genesis: Vec<String>,
    /// Damaged, incomplete and locked files, and when they were seen (verify's 60 s rule).
    pub(super) damaged: Vec<(PathBuf, u64, Option<SystemTime>)>,
    pub(super) seen_at: Instant,
    /// The lease held right after the scan (a full check).
    pub(super) held: Option<u64>,
    pub(super) lease_seen: LeaseSeen,
    pub(super) lease_top: Option<u64>,
}

/// What [`ImportPlan::read`] hands back to [`Store::import_apply`].
pub(crate) struct ImportRead {
    pub(super) scanner: Scanner,
    pub(super) hydrate_from: Option<PathBuf>,
    pub(super) genesis: String,
    pub(super) fence: Fence,
    pub(super) io_gen: u64,
    /// The folder could not be listed (`IO_ERROR`): nothing else was read.
    pub(super) folder: Result<Folder, CoreError>,
    /// `finisher.project.json` as read before the scan (when it could be).
    pub(crate) project_file: Option<ProjectFile>,
    /// Conflict copies of the project file beside it (status).
    pub(crate) project_file_copies: Vec<String>,
}

impl Store {
    /// Takes out what an import pass reads the folder with (under the store lock).
    pub(crate) fn import_plan(&mut self) -> Result<ImportPlan, StoreError> {
        let genesis = crate::project_file::db_genesis_hash(&self.conn)?;
        Ok(ImportPlan {
            project_dir: self.project_dir.clone(),
            kind: self.import.kind,
            scanner: std::mem::take(&mut self.import.scanner),
            // Cloned (shared segments): status and compaction read it meanwhile.
            segrefs: self.import.segrefs.clone(),
            hydrate_from: self.import.hydrate_from.clone(),
            genesis,
            fence: self.export.lease.fence(),
            forked: self.import.forked,
            lease_seen: self.import.lease_seen.clone(),
            lease_top: self.import.lease_top,
            clock: self.ctl.params.clock.clone(),
            io_gen: self.phase.io_gen,
            #[cfg(any(test, feature = "test-hooks"))]
            hooks: self.phase.hooks.clone(),
        })
    }
}

impl ImportPlan {
    /// Reads the folder (no store, no database).
    pub(crate) fn read(mut self) -> ImportRead {
        let project_file = crate::project_file::read_project_file_once(&self.project_dir).ok();
        let project_file_copies = crate::status::project_file_copies(&self.project_dir);
        let folder = self.folder();
        #[cfg(any(test, feature = "test-hooks"))]
        self.hooks.at(crate::tick::hooks::TickPoint::ImportRead);
        ImportRead {
            scanner: self.scanner,
            hydrate_from: self.hydrate_from,
            genesis: self.genesis,
            fence: self.fence,
            io_gen: self.io_gen,
            folder,
            project_file,
            project_file_copies,
        }
    }

    fn folder(&mut self) -> Result<Folder, CoreError> {
        let mut report = ImportReport::default();
        let dot = self.project_dir.join(".finisher");
        let listed = if dot.is_dir() {
            self.scanner.scan_for(&self.project_dir, self.kind)
        } else {
            // The folder vanished (unmounted, renamed): never read that as "empty".
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is missing", dot.display()),
            ))
        };
        let entries = listed.map_err(|e| {
            CoreError::new(
                "IO_ERROR",
                format!("cannot list {}: {e}", self.project_dir.display()),
            )
            .with_details(json!({ "path": self.project_dir.to_string_lossy() }))
        })?;
        report.hydrated = hydrate::run(&self.project_dir, &entries, &mut self.hydrate_from);

        // Classify. A segment written under another genesis (`genesisHash`) is another
        // project's history under this id (a hand-made copy, an edited project): it is not
        // imported, and the pass reports `GENESIS_MISMATCH` (read-only `IMPORT_ERROR`) until it
        // is moved out. It is not quarantined: its records belong to no history of ours.
        let mut foreign_genesis: Vec<String> = vec![];
        let mut by_branch: HashMap<String, Vec<SegRef>> = HashMap::new();
        let mut present = HashMap::new();
        let mut foreign = vec![];
        let mut newer = false;
        for entry in &entries {
            match entry {
                Entry::Segment { branch, seg, name } if seg.header.genesis_hash != self.genesis => {
                    let path = dot.join("segments").join(branch).join(name.to_string());
                    foreign_genesis.push(path.to_string_lossy().into_owned());
                }
                Entry::Segment { branch, seg, .. } => {
                    let r = self
                        .segrefs
                        .get(&seg.hash)
                        .cloned()
                        .unwrap_or_else(|| SegRef::new(seg.clone()));
                    by_branch.entry(branch.clone()).or_default().push(r.clone());
                    present.insert(seg.hash.clone(), r);
                }
                Entry::Newer { .. } | Entry::UnsupportedHash { .. } => newer = true,
                Entry::NotArrived { path, why } => report.not_arrived.push((path.clone(), *why)),
                Entry::Foreign {
                    path,
                    conflict_copy,
                    content,
                } => {
                    if matches!(
                        content,
                        Some(ForeignContent::Newer { .. } | ForeignContent::UnsupportedHash)
                    ) {
                        newer = true;
                    }
                    let seg = match content {
                        Some(ForeignContent::Segment(s)) => Some(s.clone()),
                        _ => None,
                    };
                    foreign.push((path.clone(), *conflict_copy, seg));
                }
                _ => {}
            }
        }
        let damaged: Vec<_> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Damaged {
                    path,
                    len,
                    modified,
                    ..
                } => Some((path.clone(), *len, *modified)),
                Entry::NotArrived {
                    path,
                    why: NotArrivedWhy::Incomplete | NotArrivedWhy::Locked,
                } => {
                    let m = std::fs::metadata(path).ok();
                    Some((
                        path.clone(),
                        m.as_ref().map_or(0, |m| m.len()),
                        m.and_then(|m| m.modified().ok()),
                    ))
                }
                _ => None,
            })
            .collect();
        let seen_at = self.clock.now();
        for segs in by_branch.values_mut() {
            segs.sort_by(|a, b| a.hash().cmp(b.hash()));
        }
        let held = self.fence.check().ok();
        // Lease files for the rewind rule, never under the database lock (module docs).
        let lease_seen: LeaseSeen = (present.keys().cloned().collect(), held);
        let lease_top = if self.forked || self.lease_seen.as_ref() != Some(&lease_seen) {
            rewind::lease_top(&dot)
        } else {
            self.lease_top
        };
        Ok(Folder {
            report,
            by_branch,
            present,
            foreign,
            newer,
            foreign_genesis,
            damaged,
            seen_at,
            held,
            lease_seen,
            lease_top,
        })
    }
}
