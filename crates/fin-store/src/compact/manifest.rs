//! Compaction manifests: `.finisher/compaction/<ulid>.json` (spec §3.9), one JSON object
//! and a newline, written once like every other folder file.
//!
//! `{"magic":"finisher-compaction","formatVersion":1,"minReaderVersion":1,"projectId",
//! "writerId","epoch","branch","replaced":[names],"packs":[names],"snapshot":name|null,
//! "createdMs","kind"}`. `kind` is the storage kind the writer detected; the grace period is
//! the longer of the writer's and the reader's (a sync folder anywhere: 24 h). `createdMs` is
//! informational: grace is never measured from it.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fsio;
use crate::names::{ManifestName, SegmentName, SnapName};
use crate::storage_kind::StorageKind;

pub(super) const MAGIC: &str = "finisher-compaction";
pub(super) const FORMAT_VERSION: u32 = 1;
pub(super) const READER_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Manifest {
    pub(super) magic: String,
    pub(super) format_version: u32,
    pub(super) min_reader_version: u32,
    pub(super) project_id: String,
    pub(super) writer_id: String,
    pub(super) epoch: u64,
    pub(super) branch: String,
    /// Segment file names in `segments/<branch>/` that the packs replace.
    pub(super) replaced: Vec<String>,
    /// Pack file names in `segments/<branch>/`.
    pub(super) packs: Vec<String>,
    /// The snapshot file name in `snapshots/<branch>/`.
    pub(super) snapshot: Option<String>,
    pub(super) created_ms: i64,
    pub(super) kind: String,
}

impl Manifest {
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut b = serde_json::to_vec(self).expect("a manifest holds only strings and numbers");
        b.push(b'\n');
        b
    }

    /// Why this manifest must never be acted on, if it must not: another project, a newer
    /// format, a name that is not strictly one of ours (nothing outside `segments/<branch>/`
    /// can ever be named), an empty or overlapping list.
    pub(super) fn invalid(&self, project_id: &str) -> Option<String> {
        if self.magic != MAGIC {
            return Some("not a compaction manifest".into());
        }
        if self.min_reader_version > READER_VERSION {
            return Some(format!(
                "needs Finisher reader version {}",
                self.min_reader_version
            ));
        }
        if self.project_id != project_id {
            return Some(format!("names project {}", self.project_id));
        }
        if !crate::scan::valid_branch(&self.branch) {
            return Some(format!("names branch {:?}", self.branch));
        }
        if self.replaced.is_empty() || self.packs.is_empty() {
            return Some("lists no replaced segments or no packs".into());
        }
        let all_segments = self
            .replaced
            .iter()
            .chain(&self.packs)
            .all(|n| SegmentName::parse(n).is_some_and(|p| p.to_string() == *n));
        if !all_segments {
            return Some("lists a name that is not a segment name".into());
        }
        if self.packs.iter().any(|p| self.replaced.contains(p)) {
            return Some("lists a pack among the replaced segments".into());
        }
        if self
            .snapshot
            .as_deref()
            .is_some_and(|n| SnapName::parse(n).is_none_or(|p| p.to_string() != n))
        {
            return Some("names a snapshot that is not a snapshot name".into());
        }
        None
    }

    /// The writer's storage kind; one that does not parse counts as a sync folder (the
    /// longest grace).
    pub(super) fn writer_kind(&self) -> StorageKind {
        self.kind
            .parse()
            .unwrap_or(StorageKind::Sync(crate::conflict_names::Provider::Unknown))
    }
}

/// What reading a manifest gave.
pub(super) enum Read {
    Ok(Manifest),
    /// Absent (deleted meanwhile).
    Gone,
    /// Cannot be read now, or does not parse (it may still be arriving on a sync folder).
    Unreadable(String),
}

pub(super) fn read(path: &Path) -> Read {
    match fsio::read_uncached(path) {
        Ok(b) => match serde_json::from_slice::<Manifest>(&b) {
            Ok(m) => Read::Ok(m),
            Err(e) => Read::Unreadable(format!("does not parse: {e}")),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Read::Gone,
        Err(e) => Read::Unreadable(e.to_string()),
    }
}

/// The manifests in `dir`, by name.
pub(super) fn list(dir: &Path) -> io::Result<Vec<ManifestName>> {
    crate::reserved::no_links(dir)?;
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut out: Vec<ManifestName> = rd
        .filter_map(|e| ManifestName::parse(e.ok()?.file_name().to_str()?))
        .collect();
    out.sort();
    Ok(out)
}
