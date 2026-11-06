//! The shared font cache `<server-data>/fonts/` (4C Decision 32).
//!
//! - Files are named by their SHA-256 (`<sha256>`) and hold exactly those bytes.
//! - A file arrives as a temp (`<sha256>.part` for a resumable download, `.~fin-<id>.part`
//!   for an extracted archive member), is verified, and is linked into place **without
//!   replacing**: an existing file is verified and kept (a bad one is removed first).
//! - Every use verifies the SHA-256; a file whose length and mtime are unchanged since it
//!   last verified is not hashed again.
//! - Stray temps (and abandoned resumable downloads nobody holds the lock for) are swept
//!   after [`STRAY_AFTER`]. No total-size limit in 4C.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fin_core::CoreError;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{MAX_FILE_SIZE, hex, is_sha256, lock, now_ms};

/// Temps older than this are swept: 24 h.
pub const STRAY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

const TEMP_PREFIX: &str = ".~fin-";

/// What a verified file looked like: length, mtime and file identity (so a file replaced by
/// another with the same length and mtime is hashed again).
type Stamp = (u64, SystemTime, FileId);

pub struct FontCache {
    dir: PathBuf,
    /// The stamp at which each file last verified.
    verified: Mutex<HashMap<String, Stamp>>,
    /// Files that failed verification, with when (ms since the epoch).
    mismatched: Mutex<HashMap<String, i64>>,
}

fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The device and inode on Unix; on Windows the creation time (std has no stable file
/// index there), which a replacing write changes.
#[cfg(unix)]
type FileId = (u64, u64);
#[cfg(not(unix))]
type FileId = Option<SystemTime>;

#[cfg(unix)]
fn file_id(meta: &fs::Metadata) -> FileId {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn file_id(meta: &fs::Metadata) -> FileId {
    meta.created().ok()
}

fn stamp(meta: &fs::Metadata) -> Option<Stamp> {
    Some((meta.len(), meta.modified().ok()?, file_id(meta)))
}

/// SHA-256 of the file at `path`.
pub(crate) fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hex(&hasher.finalize()));
        }
        hasher.update(&buf[..n]);
    }
}

impl FontCache {
    /// Opens (creating) the cache directory and sweeps stray temps.
    pub fn open(dir: &Path) -> Result<FontCache, CoreError> {
        fs::create_dir_all(dir).map_err(|e| {
            CoreError::new(
                "IO_ERROR",
                format!("cannot create the font cache {}: {e}", dir.display()),
            )
            .with_details(json!({ "path": dir.display().to_string() }))
        })?;
        let cache = FontCache {
            dir: dir.to_path_buf(),
            verified: Mutex::new(HashMap::new()),
            mismatched: Mutex::new(HashMap::new()),
        };
        cache.sweep(SystemTime::now());
        Ok(cache)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file's bytes when they hash to `sha256`; `None` when absent, larger than
    /// [`MAX_FILE_SIZE`] (never read) or mismatched (then [`FontCache::mismatch`] says when).
    pub fn get(&self, sha256: &str) -> Option<Arc<[u8]>> {
        if !is_sha256(sha256) {
            return None;
        }
        let file = File::open(self.path(sha256)).ok()?;
        let meta = file.metadata().ok()?;
        if meta.len() > MAX_FILE_SIZE {
            self.note_mismatch(sha256);
            return None;
        }
        let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
        file.take(MAX_FILE_SIZE + 1).read_to_end(&mut bytes).ok()?;
        let st = stamp(&meta);
        let known = st.is_some()
            && st.map(|s| s.0) == Some(bytes.len() as u64)
            && guard(&self.verified).get(sha256) == st.as_ref();
        if known || crate::sha256_hex(&bytes) == sha256 {
            self.mark_verified(sha256, st);
            Some(bytes.into())
        } else {
            self.note_mismatch(sha256);
            None
        }
    }

    fn note_mismatch(&self, sha256: &str) {
        guard(&self.verified).remove(sha256);
        guard(&self.mismatched).insert(sha256.to_string(), now_ms());
    }

    /// Whether a verified file is cached.
    pub fn has(&self, sha256: &str) -> bool {
        if !is_sha256(sha256) {
            return false;
        }
        let Ok(meta) = fs::metadata(self.path(sha256)) else {
            return false;
        };
        let st = stamp(&meta);
        (st.is_some() && guard(&self.verified).get(sha256) == st.as_ref())
            || self.get(sha256).is_some()
    }

    /// When the cached file last failed verification (ms since the epoch), unless it has
    /// verified since.
    pub fn mismatch(&self, sha256: &str) -> Option<i64> {
        guard(&self.mismatched).get(sha256).copied()
    }

    /// The SHA-256 names of the files present (not verified here).
    pub fn list(&self) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| is_sha256(n))
            .collect();
        out.sort();
        out
    }

    /// Removes temps older than [`STRAY_AFTER`] at `now`: `.~fin-*.part` files, and
    /// resumable `<sha256>.part`/`.part.json` pairs, each deleted while holding its file's
    /// fetch lock (one another fetcher holds is left alone). Returns how many files went.
    pub fn sweep(&self, now: SystemTime) -> usize {
        let old = |p: &Path| {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > STRAY_AFTER))
        };
        let mut removed = 0;
        for entry in fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let path = entry.path();
            let resumable = name
                .strip_suffix(".part.json")
                .or_else(|| name.strip_suffix(".part"))
                .filter(|s| is_sha256(s));
            let gone = if name.starts_with(TEMP_PREFIX) && name.ends_with(".part") {
                old(&path) && fs::remove_file(&path).is_ok()
            } else if let Some(sha) = resumable {
                old(&path)
                    && lock::try_take(&self.lock_path(sha)).is_some_and(|_held| {
                        // Re-checked under the lock: a fetcher may have resumed it meanwhile.
                        old(&path) && fs::remove_file(&path).is_ok()
                    })
            } else {
                false
            };
            if gone {
                removed += 1;
            }
        }
        removed
    }

    pub(crate) fn path(&self, sha256: &str) -> PathBuf {
        self.dir.join(sha256)
    }

    pub(crate) fn part_path(&self, sha256: &str) -> PathBuf {
        self.dir.join(format!("{sha256}.part"))
    }

    pub(crate) fn part_json_path(&self, sha256: &str) -> PathBuf {
        self.dir.join(format!("{sha256}.part.json"))
    }

    pub(crate) fn lock_path(&self, sha256: &str) -> PathBuf {
        self.dir.join(format!("{sha256}.lock"))
    }

    /// A fresh `.~fin-<id>.part` path.
    pub(crate) fn temp_path(&self) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        self.dir.join(format!(
            "{TEMP_PREFIX}{nanos:x}-{:x}-{n:x}.part",
            std::process::id()
        ))
    }

    fn mark_verified(&self, sha256: &str, st: Option<Stamp>) {
        if let Some(st) = st {
            guard(&self.verified).insert(sha256.to_string(), st);
        }
        guard(&self.mismatched).remove(sha256);
    }

    /// Removes the cached file when it does not verify (the caller holds its lock).
    pub(crate) fn discard_bad(&self, sha256: &str) {
        if self.path(sha256).exists() && !self.has(sha256) {
            let _ = fs::remove_file(self.path(sha256));
        }
    }

    /// Verifies `temp` and links it into place as `<sha256>` without replacing; an existing
    /// file that verifies is kept, a bad one is replaced. `temp` is gone afterwards.
    /// `InvalidData` when `temp` does not hash to `sha256`.
    pub(crate) fn place(&self, sha256: &str, temp: &Path) -> io::Result<()> {
        let result = self.place_inner(sha256, temp);
        let _ = fs::remove_file(temp);
        result
    }

    fn place_inner(&self, sha256: &str, temp: &Path) -> io::Result<()> {
        if hash_file(temp)? != sha256 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "hashMismatch"));
        }
        let target = self.path(sha256);
        for _ in 0..3 {
            let linked = match fs::hard_link(temp, &target) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
                // No hard links on this filesystem: check-then-rename. Fetchers hold the file's
                // lock, except for archive siblings (`fetch.rs`), which are placed under the
                // archive's lock only. Racing there can at worst rename one verified copy over
                // another (same bytes; the memo re-hashes once); Windows refuses to rename over
                // an existing file, which then verifies and is kept.
                Err(_) if !target.exists() => fs::rename(temp, &target),
                Err(e) => Err(e),
            };
            match linked {
                Ok(()) => {
                    let st = fs::metadata(&target).ok().and_then(|m| stamp(&m));
                    self.mark_verified(sha256, st);
                    return Ok(());
                }
                Err(_) if self.has(sha256) => return Ok(()),
                Err(_) => match fs::remove_file(&target) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                },
            }
        }
        Err(io::Error::other("cannot place the font file"))
    }
}

impl fin_text::FontBytes for FontCache {
    fn get(&self, sha256: &str) -> Option<Arc<[u8]>> {
        FontCache::get(self, sha256)
    }
}
