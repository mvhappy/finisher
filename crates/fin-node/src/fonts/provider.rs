//! Font bytes for the shared measurer (4C Decisions 36 and 37): the server's font cache,
//! else an open project's font asset file whose SHA-256 matches. And the measurer that
//! asks the fetcher for what a measurement found missing (Decision 34).

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::SystemTime;

use fin_core::CoreError;
use fin_core::layout::{Measured, TextMeasurer, TextRequest};
use fin_fonts::{FontCache, MAX_FILE_SIZE, sha256_hex};
use fin_text::FontBytes;

use super::Manager;
use crate::engine::lock;

/// Why a font asset file cannot serve its pinned bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssetProblem {
    NotFound,
    HashMismatch,
}

impl AssetProblem {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            AssetProblem::NotFound => "notFound",
            AssetProblem::HashMismatch => "hashMismatch",
        }
    }
}

/// What an asset file looked like when it was last hashed.
type Stamp = (u64, Option<SystemTime>);

#[derive(Default)]
pub(crate) struct Provider {
    cache: Option<Arc<FontCache>>,
    /// Font asset files of the open projects, by the SHA-256 their asset records.
    assets: Mutex<HashMap<String, BTreeSet<PathBuf>>>,
    /// The SHA-256 each asset file had at its stamp.
    hashed: Mutex<HashMap<PathBuf, (Stamp, String)>>,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    meta.is_file().then(|| (meta.len(), meta.modified().ok()))
}

impl Provider {
    pub(crate) fn new(cache: Option<Arc<FontCache>>) -> Provider {
        Provider {
            cache,
            ..Provider::default()
        }
    }

    /// Replaces the asset files the open projects hold.
    pub(crate) fn set_assets(&self, assets: HashMap<String, BTreeSet<PathBuf>>) {
        *lock(&self.assets) = assets;
    }

    /// The file's bytes when they hash to `sha256` now.
    fn asset_bytes(&self, path: &Path, sha256: &str) -> Result<Arc<[u8]>, AssetProblem> {
        let before = stamp(path).ok_or(AssetProblem::NotFound)?;
        if before.0 > MAX_FILE_SIZE {
            return Err(AssetProblem::HashMismatch);
        }
        let bytes = fs::read(path).map_err(|_| AssetProblem::NotFound)?;
        let sha = sha256_hex(&bytes);
        // Memoised only when the file did not change while it was read.
        if stamp(path) == Some(before) {
            lock(&self.hashed).insert(path.to_path_buf(), (before, sha.clone()));
        }
        if sha == sha256 {
            Ok(bytes.into())
        } else {
            Err(AssetProblem::HashMismatch)
        }
    }

    /// Whether the asset file holds `sha256` now (hashed again only when its length or
    /// mtime changed since it last was).
    pub(crate) fn check_asset(&self, path: &Path, sha256: &str) -> Result<(), AssetProblem> {
        let now = stamp(path).ok_or(AssetProblem::NotFound)?;
        if let Some((st, sha)) = lock(&self.hashed).get(path)
            && *st == now
        {
            return if sha == sha256 {
                Ok(())
            } else {
                Err(AssetProblem::HashMismatch)
            };
        }
        self.asset_bytes(path, sha256).map(drop)
    }
}

impl FontBytes for Provider {
    fn get(&self, sha256: &str) -> Option<Arc<[u8]>> {
        if let Some(bytes) = self.cache.as_ref().and_then(|c| c.get(sha256)) {
            return Some(bytes);
        }
        let paths = lock(&self.assets).get(sha256).cloned().unwrap_or_default();
        paths.iter().find_map(|p| self.asset_bytes(p, sha256).ok())
    }
}

/// The one measurer of a server's data: `fin-text` over [`Provider`]. A measurement that
/// finds bytes missing asks the manager to fetch them (in the background, unless fetching
/// is off); the answer stays `Unavailable` until they arrive.
pub(crate) struct Measurer {
    text: fin_text::Measurer,
    manager: Weak<Manager>,
}

impl Measurer {
    pub(crate) fn new(provider: Arc<Provider>, manager: Weak<Manager>) -> Measurer {
        Measurer {
            text: fin_text::Measurer::new(provider),
            manager,
        }
    }
}

impl TextMeasurer for Measurer {
    fn measure(&self, req: &TextRequest) -> Result<Measured, CoreError> {
        let measured = self.text.measure(req)?;
        if let Measured::Unavailable { missing } = &measured
            && let Some(m) = self.manager.upgrade()
        {
            m.request(missing.iter().map(String::as_str));
        }
        Ok(measured)
    }

    fn fingerprint(&self) -> String {
        self.text.fingerprint()
    }
}
