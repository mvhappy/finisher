//! Who wrote a quarantined record, and the names of recovered branches (plan 4B Decision 27):
//! `recovered-<server8>-<YYYYMMDD-HHMMSS>`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::groups::Row;
use crate::{StoreError, db};

/// The writer of each row's record, for names (Decision 27).
pub(crate) struct Writers {
    ours: String,
    project_dir: PathBuf,
    folder: HashMap<String, String>,
}

impl Writers {
    /// Scans the folder now, before any write lock, when a row may be the folder's.
    pub(super) fn new(
        conn: &Connection,
        ours: &str,
        project_dir: &Path,
    ) -> Result<Writers, StoreError> {
        let folder_rows = conn
            .prepare_cached(
                "SELECT 1 FROM quarantine WHERE current = 1 AND origin = 'folder' LIMIT 1",
            )?
            .exists([])?;
        let mut folder = HashMap::new();
        if folder_rows {
            for e in crate::scan::scan(project_dir).unwrap_or_default() {
                if let crate::scan::Entry::Segment { seg, .. } = e {
                    for b in &seg.records {
                        folder.insert(db::hash_bytes(b), seg.header.writer_id.clone());
                    }
                }
            }
        }
        Ok(Writers {
            ours: ours.to_string(),
            project_dir: project_dir.to_path_buf(),
            folder,
        })
    }

    pub(super) fn of(&self, row: &Row) -> Option<String> {
        match row.origin.as_str() {
            "local" | "salvage" => Some(self.ours.clone()),
            "foreign" => {
                let path = PathBuf::from(&row.detail);
                let path = if path.is_absolute() {
                    path
                } else {
                    self.project_dir.join(path)
                };
                let seg = crate::segment::decode(&std::fs::read(path).ok()?).ok()?;
                seg.records
                    .iter()
                    .any(|b| db::hash_bytes(b) == row.hash)
                    .then_some(seg.header.writer_id)
            }
            _ => self.folder.get(&row.hash).cloned(),
        }
    }
}

/// `<server8>` of a writer id: its first 8 characters when they are letters or digits.
pub(super) fn server8(writer: Option<&str>) -> String {
    match writer {
        Some(w)
            if w.len() >= 8
                && w.is_char_boundary(8)
                && w[..8].bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            w[..8].to_ascii_lowercase()
        }
        _ => "unknown".to_string(),
    }
}

/// `YYYYMMDD-HHMMSS` (UTC) of unix ms `ms`.
pub(super) fn utc_stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = crate::backup::civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
