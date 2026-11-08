//! The worker's measurer (plan 6A Decision 3): the same `fin-text` measurer `fin-node` uses,
//! over the server's font cache opened read-only, then the project's font asset files the
//! manifest lists (opened through the no-follow opener and kept only when their SHA-256 is the
//! one their asset pins). The measurer and its font systems (keyed by the faces' SHA-256) stay
//! across jobs; each job replaces the sources.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use fin_fonts::{FontCache, MAX_FILE_SIZE, sha256_hex};
use fin_media::jobs::{ManifestFonts, OriginalFile};
use fin_text::FontBytes;

use super::open_cache::open_original;

#[derive(Default)]
struct Sources {
    cache: Option<(PathBuf, Arc<FontCache>)>,
    files: HashMap<String, Vec<OriginalFile>>,
}

#[derive(Default)]
struct Provider {
    sources: Mutex<Sources>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Provider {
    fn from_file(o: &OriginalFile, sha256: &str) -> Option<Arc<[u8]>> {
        if o.size > MAX_FILE_SIZE {
            return None;
        }
        let f = open_original(o).ok()?;
        let mut bytes = Vec::new();
        f.take(MAX_FILE_SIZE + 1).read_to_end(&mut bytes).ok()?;
        (sha256_hex(&bytes) == sha256).then(|| bytes.into())
    }
}

impl FontBytes for Provider {
    fn get(&self, sha256: &str) -> Option<Arc<[u8]>> {
        let (cache, files) = {
            let s = lock(&self.sources);
            (
                s.cache.as_ref().map(|(_, c)| Arc::clone(c)),
                s.files.get(sha256).cloned().unwrap_or_default(),
            )
        };
        if let Some(bytes) = cache.and_then(|c| c.get(sha256)) {
            return Some(bytes);
        }
        files.iter().find_map(|o| Self::from_file(o, sha256))
    }
}

/// The measurer and where its bytes come from.
pub struct Fonts {
    provider: Arc<Provider>,
    measurer: fin_text::Measurer,
}

impl Fonts {
    pub fn new() -> Fonts {
        let provider = Arc::new(Provider::default());
        let bytes: Arc<dyn FontBytes> = provider.clone();
        Fonts {
            measurer: fin_text::Measurer::new(bytes),
            provider,
        }
    }

    /// Takes the job's font sources; the cache stays open while its directory is the same.
    pub fn set(&self, fonts: &ManifestFonts) {
        let mut s = lock(&self.provider.sources);
        let dir = fonts.cache.as_ref().map(PathBuf::from);
        let same = matches!((&s.cache, &dir), (Some((have, _)), Some(want)) if have == want);
        if !same {
            s.cache = dir.map(|d| {
                let cache = Arc::new(FontCache::open_read_only(&d));
                (d, cache)
            });
        }
        s.files.clear();
        for f in &fonts.files {
            s.files
                .entry(f.sha256.clone())
                .or_default()
                .push(f.original.clone());
        }
    }

    pub fn measurer(&self) -> &fin_text::Measurer {
        &self.measurer
    }
}
