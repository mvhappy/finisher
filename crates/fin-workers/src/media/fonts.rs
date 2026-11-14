//! The worker's measurer (plan 6A Decision 3): the same `fin-text` measurer `fin-node` uses,
//! over the server's font cache opened read-only, then the project's font asset files the
//! manifest lists (opened through the no-follow opener and kept only when their SHA-256 is the
//! one their asset pins). The measurer and its font systems (keyed by the faces' SHA-256) stay
//! across jobs; each job replaces the sources.
//!
//! The font cache is read through `fin-library`'s no-follow opener too (plan 6B-1 Task 18, spec
//! §25.5 residual 1): `<cache>/<sha256>` must be a regular file, never a link, of at most
//! [`MAX_FILE_SIZE`] bytes, and its bytes must hash to its name. `fin-fonts`' `FontCache` opened
//! it by path and followed a link there.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use fin_fonts::{MAX_FILE_SIZE, is_sha256, sha256_hex};
use fin_library::{Expect, RawRel};
use fin_media::jobs::{ManifestFonts, OriginalFile};
use fin_text::FontBytes;

use super::open_cache::open_original;

#[derive(Default)]
struct Sources {
    cache: Option<PathBuf>,
    files: HashMap<String, Vec<OriginalFile>>,
}

#[derive(Default)]
struct Provider {
    sources: Mutex<Sources>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The modification time as the opener reads it (ns since the Unix epoch).
fn mtime_ns(m: &std::fs::Metadata) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        m.mtime() * 1_000_000_000 + m.mtime_nsec()
    }
    #[cfg(not(unix))]
    {
        let ns = |d: std::time::Duration| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX);
        match m
            .modified()
            .map(|t| t.duration_since(std::time::UNIX_EPOCH))
        {
            Ok(Ok(d)) => ns(d),
            Ok(Err(e)) => ns(e.duration()).saturating_neg(),
            Err(_) => 0,
        }
    }
}

/// `<dir>/<sha256>` from the font cache: a regular file (not a link), opened by the no-follow
/// opener against what a no-follow stat saw, whose bytes hash to its name.
fn from_cache(dir: &Path, sha256: &str) -> Option<Arc<[u8]>> {
    if !is_sha256(sha256) {
        return None;
    }
    // The server's own cache directory, never a library path (`scripts/clippy-allow.json`).
    #[allow(clippy::disallowed_methods)]
    let m = std::fs::symlink_metadata(dir.join(sha256)).ok()?;
    if !m.file_type().is_file() || m.len() > MAX_FILE_SIZE {
        return None;
    }
    let expect = Expect {
        size: m.len(),
        mtime_ns: mtime_ns(&m),
        file_id: None,
        dev: None,
    };
    let rel = RawRel::new(vec![sha256.as_bytes().to_vec()]).ok()?;
    let f = fin_library::open_for_read(dir, &rel, &expect, false).ok()?;
    let mut bytes = Vec::new();
    f.take(MAX_FILE_SIZE + 1).read_to_end(&mut bytes).ok()?;
    (sha256_hex(&bytes) == sha256).then(|| bytes.into())
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
                s.cache.clone(),
                s.files.get(sha256).cloned().unwrap_or_default(),
            )
        };
        if let Some(bytes) = cache.and_then(|d| from_cache(&d, sha256)) {
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

    /// Takes the job's font sources.
    pub fn set(&self, fonts: &ManifestFonts) {
        let mut s = lock(&self.provider.sources);
        s.cache = fonts.cache.as_ref().map(PathBuf::from);
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

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)]
    use super::*;

    #[test]
    fn the_font_cache_opens_regular_files_only() {
        let d = tempfile::tempdir().unwrap();
        let cache = d.path().join("fonts");
        std::fs::create_dir_all(&cache).unwrap();
        let font = b"a font's bytes".to_vec();
        let sha = sha256_hex(&font);
        // A regular file whose bytes hash to its name is read.
        std::fs::write(cache.join(&sha), &font).unwrap();
        assert_eq!(from_cache(&cache, &sha).as_deref(), Some(&font[..]));
        // A link there is not followed, even to the right bytes.
        std::fs::remove_file(cache.join(&sha)).unwrap();
        let elsewhere = d.path().join("elsewhere.ttf");
        std::fs::write(&elsewhere, &font).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&elsewhere, cache.join(&sha)).unwrap();
            assert_eq!(from_cache(&cache, &sha), None);
            std::fs::remove_file(cache.join(&sha)).unwrap();
        }
        // Other bytes, a bad name or a missing file: nothing.
        std::fs::write(cache.join(&sha), b"other bytes").unwrap();
        assert_eq!(from_cache(&cache, &sha), None);
        assert_eq!(from_cache(&cache, "../elsewhere.ttf"), None);
        assert_eq!(from_cache(&cache, &sha256_hex(b"absent")), None);
    }
}
