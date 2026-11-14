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

#[allow(clippy::disallowed_methods)] // font asset files of projects, never a library path
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
    #[allow(clippy::disallowed_methods)] // project font assets, never a library path
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

/// How many layouts the measurer keeps (least recently used out first).
pub(crate) const LAYOUTS_CACHED: usize = 1024;

/// Layouts by request: the SHA-256 of the request's JSON, which names the caption's words,
/// its effective style (overrides applied), its pinned faces by SHA-256 and its box (the
/// timeline's resolution); the engine fingerprint is the process's. Only layouts are kept:
/// an `Unavailable` answer is asked again, so measurement resumes once bytes arrive.
#[derive(Default)]
struct Layouts {
    map: HashMap<String, (u64, Measured)>,
    tick: u64,
}

impl Layouts {
    fn get(&mut self, key: &str) -> Option<Measured> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(key).map(|(at, m)| {
            *at = tick;
            m.clone()
        })
    }

    fn put(&mut self, key: String, m: Measured) {
        self.tick += 1;
        if self.map.len() >= LAYOUTS_CACHED
            && let Some(old) = self
                .map
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
        {
            self.map.remove(&old);
        }
        self.map.insert(key, (self.tick, m));
    }
}

/// The one measurer of a server's data: `fin-text` over [`Provider`], with a layout cache
/// ([`Layouts`]: the overflow check and eval measure the same captions again and again). A
/// measurement that finds bytes missing asks the manager to fetch them (in the background,
/// unless fetching is off); the answer stays `Unavailable` until they arrive.
pub(crate) struct Measurer {
    text: fin_text::Measurer,
    manager: Weak<Manager>,
    layouts: Mutex<Layouts>,
}

impl Measurer {
    pub(crate) fn new(provider: Arc<Provider>, manager: Weak<Manager>) -> Measurer {
        Measurer {
            text: fin_text::Measurer::new(provider),
            manager,
            layouts: Mutex::new(Layouts::default()),
        }
    }
}

impl TextMeasurer for Measurer {
    fn measure(&self, req: &TextRequest) -> Result<Measured, CoreError> {
        let key = serde_json::to_vec(req).map(|j| sha256_hex(&j)).ok();
        if let Some(hit) = key.as_deref().and_then(|k| lock(&self.layouts).get(k)) {
            return Ok(hit);
        }
        let measured = self.text.measure(req)?;
        match &measured {
            Measured::Unavailable { missing } => {
                if let Some(m) = self.manager.upgrade() {
                    m.request(missing.iter().map(String::as_str));
                }
            }
            Measured::Layout(_) => {
                if let Some(k) = key {
                    lock(&self.layouts).put(k, measured.clone());
                }
            }
        }
        Ok(measured)
    }

    fn fingerprint(&self) -> String {
        self.text.fingerprint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_core::layout::TextLayout;

    fn layout(width: i32) -> Measured {
        Measured::Layout(TextLayout {
            lines: Vec::new(),
            words: Vec::new(),
            width,
            height: 0,
            overflow: false,
            missing: Vec::new(),
            faces: Vec::new(),
        })
    }

    #[test]
    fn layouts_keep_the_most_recently_used() {
        let mut c = Layouts::default();
        for i in 0..LAYOUTS_CACHED {
            c.put(format!("k{i}"), layout(i as i32));
        }
        // k0 is used again, so k1 is the least recently used when k-new comes in.
        assert_eq!(c.get("k0"), Some(layout(0)));
        c.put("k-new".into(), layout(-1));
        assert_eq!(c.map.len(), LAYOUTS_CACHED);
        assert!(c.get("k1").is_none());
        assert_eq!(c.get("k0"), Some(layout(0)));
        assert_eq!(c.get("k-new"), Some(layout(-1)));
    }

    #[test]
    fn unavailable_answers_are_not_kept() {
        // No cache and no assets: every face is missing, asked again each time.
        let m = Measurer::new(Arc::new(Provider::new(None)), Weak::new());
        let req: TextRequest = serde_json::from_value(serde_json::json!({
            "words": ["hi"],
            "style": { "family": "X", "weight": 400, "italic": false, "size": 64.0,
                       "lineHeight": 1.2, "letterSpacing": 0.0, "align": "center" },
            "faces": [{ "sha256": "a".repeat(64), "face": 0, "family": "X", "weight": 400,
                        "italic": false }],
            "maxWidth": 100.0,
            "maxLines": 2
        }))
        .unwrap();
        assert!(matches!(
            m.measure(&req).unwrap(),
            Measured::Unavailable { .. }
        ));
        assert!(lock(&m.layouts).map.is_empty());
    }
}
