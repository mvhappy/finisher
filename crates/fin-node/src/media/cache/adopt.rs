//! Checking an entry directory, and the adoption at start (6B-1 plan Decision 10, review
//! focus 5: the cache never serves a half-written or foreign entry).
//!
//! [`verify_dir`] is the one check of a directory, for a commit (after its files are moved
//! into a staging directory the host owns) and for adoption: `meta.json` opened through
//! `fin_shm::checked_open` (no link at any segment, one link, at most [`META_CAP`]), parsed
//! strictly and [`EntryMeta::check`]ed (the key recomputes), its key the directory's name; the
//! directory holds exactly `meta.json` and the files listed, each a regular file of the size
//! listed whose BLAKE3 matches.
//!
//! [`adopt`] at open: the staging directory and the trash are emptied (best effort: what will
//! not go is a warning in [`Adopted`]); a row whose directory is gone is dropped; a directory
//! under a shard without a row is adopted (its row inserted, serving no project until a commit
//! or a hit's `serve` names one) when [`verify_dir`] passes, and deleted
//! otherwise, as is anything else in a shard (a file, a link, a name that is not a key of that
//! shard).

// The cache's own directories under server data, never a library path.
#![allow(clippy::disallowed_methods)]

use std::io::Read;
use std::path::Path;

use fin_core::CoreError;
use fin_media::cache::{CacheKey, EntryMeta, META_FILE};
use serde_json::json;

use super::evict::{entry_dir, remove_any, trash_dir};
use super::index::{Index, cache_io};

/// The largest `meta.json` read (a proxy's `MediaInfo` and its file list fit many times over).
pub const META_CAP: u64 = fin_media::cache::MAX_META_BYTES;

/// What the adoption did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Adopted {
    /// Directories without a row that passed the checks and got one.
    pub adopted: u64,
    /// Directories (or other names) under a shard that were deleted.
    pub deleted: u64,
    /// Rows whose directory was gone.
    pub dropped: u64,
    /// The index did not open (another schema version, a damaged file) and was made anew.
    pub index_replaced: bool,
    /// What could not be done and was left (leftovers that would not go).
    pub warnings: Vec<String>,
}

pub fn mismatch(message: impl Into<String>, file: &str) -> CoreError {
    CoreError::new("PLAN_MISMATCH", message)
        .with_details(json!({ "reason": "cacheEntry", "file": file }))
}

/// Reads and checks `dir/meta.json` (module docs).
pub fn read_meta(dir: &Path) -> Result<(EntryMeta, u64), CoreError> {
    let path = dir.join(META_FILE);
    let mut file = fin_shm::checked_open(&path, META_CAP)?;
    let mut text = Vec::new();
    file.read_to_end(&mut text)
        .map_err(|e| mismatch(format!("meta.json could not be read: {e}"), META_FILE))?;
    let meta: EntryMeta = serde_json::from_slice(&text)
        .map_err(|e| mismatch(format!("meta.json is not an entry's: {e}"), META_FILE))?;
    meta.check()?;
    Ok((meta, text.len() as u64))
}

/// BLAKE3 and length of a checked file, read to its end.
fn hash_file(path: &Path, cap: u64) -> Result<(String, u64), CoreError> {
    let file = fin_shm::checked_open(path, cap)?;
    let mut hasher = blake3::Hasher::new();
    let mut reader = file.take(cap.saturating_add(1));
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let got = reader
            .read(&mut buf)
            .map_err(|e| cache_io("io", format!("an entry file could not be read: {e}")))?;
        if got == 0 {
            break;
        }
        n += got as u64;
        hasher.update(&buf[..got]);
    }
    Ok((hasher.finalize().to_hex().to_string(), n))
}

/// The names in `dir` (which must be a real directory, not a link), each with whether it is a
/// regular file (links are not followed).
pub fn list_dir(dir: &Path) -> Result<Vec<(String, bool)>, CoreError> {
    let meta = std::fs::symlink_metadata(dir)
        .map_err(|e| mismatch(format!("the entry directory is missing: {e}"), "."))?;
    if !meta.is_dir() {
        return Err(mismatch("the entry is not a directory", "."));
    }
    let mut out = Vec::new();
    let read = std::fs::read_dir(dir)
        .map_err(|e| cache_io("io", format!("an entry directory could not be listed: {e}")))?;
    for e in read {
        let e =
            e.map_err(|e| cache_io("io", format!("an entry directory could not be listed: {e}")))?;
        let regular = e.file_type().map(|t| t.is_file()).unwrap_or(false);
        out.push((e.file_name().to_string_lossy().into_owned(), regular));
    }
    out.sort();
    Ok(out)
}

/// The directory's names are exactly `meta.json` and the files `meta` lists, all regular.
pub fn check_listing(dir: &Path, meta: &EntryMeta) -> Result<(), CoreError> {
    let listed = list_dir(dir)?;
    for (name, regular) in &listed {
        let known = name == META_FILE || meta.files.iter().any(|f| &f.name == name);
        if !known {
            return Err(mismatch(
                "the entry holds a file meta.json does not list",
                name,
            ));
        }
        if !regular {
            return Err(mismatch("an entry file is not a regular file", name));
        }
    }
    if let Some(f) = meta
        .files
        .iter()
        .find(|f| !listed.iter().any(|(n, _)| n == &f.name))
    {
        return Err(mismatch("a file meta.json lists is missing", &f.name));
    }
    Ok(())
}

/// Each listed file has its size and BLAKE3.
pub fn check_hashes(dir: &Path, meta: &EntryMeta) -> Result<(), CoreError> {
    for f in &meta.files {
        let (hash, n) = hash_file(&dir.join(&f.name), f.bytes)?;
        if n != f.bytes {
            return Err(mismatch(
                format!("{} bytes, meta.json says {}", n, f.bytes),
                &f.name,
            ));
        }
        if hash != f.blake3 {
            return Err(mismatch(
                "the file's BLAKE3 differs from meta.json's",
                &f.name,
            ));
        }
    }
    Ok(())
}

/// Every check of an entry directory named `key` (module docs); the meta and the entry's
/// bytes (files and `meta.json`).
pub fn verify_dir(dir: &Path, key: &CacheKey) -> Result<(EntryMeta, u64), CoreError> {
    let (meta, meta_len) = read_meta(dir)?;
    if meta.key != key.as_str() {
        return Err(
            CoreError::new("PLAN_MISMATCH", "meta.json names another key")
                .with_details(json!({ "reason": "cacheKey", "key": key.as_str() })),
        );
    }
    check_listing(dir, &meta)?;
    check_hashes(dir, &meta)?;
    let bytes = meta
        .bytes()?
        .checked_add(meta_len)
        .ok_or_else(|| mismatch("the entry's size overflows", META_FILE))?;
    Ok((meta, bytes))
}

fn is_shard(name: &str) -> bool {
    name.len() == 2 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The adoption at start (module docs). `now` stamps adopted rows' last use.
pub fn adopt(root: &Path, index: &mut Index, now: i64) -> Result<Adopted, CoreError> {
    let mut out = Adopted::default();
    // Best effort: a leftover that will not go is reported, never a reason not to open (commits
    // and evictions use names unique to the run).
    for scratch in [root.join(".staging"), trash_dir(root)] {
        if let Err(e) = remove_any(&scratch) {
            out.warnings
                .push(format!("cannot empty {}: {e}", scratch.display()));
        }
    }
    for key in index.keys()? {
        let present = CacheKey::parse(&key)
            .ok()
            .and_then(|k| std::fs::symlink_metadata(entry_dir(root, &k)).ok())
            .is_some_and(|m| m.is_dir());
        if !present {
            index.delete(&key)?;
            out.dropped += 1;
        }
    }
    let read = std::fs::read_dir(root)
        .map_err(|e| cache_io("io", format!("the cache could not be listed: {e}")))?;
    for shard in read.flatten() {
        let name = shard.file_name().to_string_lossy().into_owned();
        if !is_shard(&name) {
            continue;
        }
        let path = shard.path();
        if !shard.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let _ = remove_any(&path);
            out.deleted += 1;
            continue;
        }
        let Ok(children) = std::fs::read_dir(&path) else {
            continue;
        };
        for child in children.flatten() {
            let child_name = child.file_name().to_string_lossy().into_owned();
            let key = CacheKey::parse(&child_name)
                .ok()
                .filter(|k| k.shard() == name);
            let keep = match &key {
                Some(k) if index.contains(k.as_str())? => true,
                Some(k) => match verify_dir(&child.path(), k) {
                    Ok((meta, bytes)) => {
                        index.insert(&meta, bytes, now, &[], &[])?;
                        out.adopted += 1;
                        true
                    }
                    Err(_) => false,
                },
                None => false,
            };
            if !keep {
                let _ = remove_any(&child.path());
                out.deleted += 1;
            }
        }
    }
    Ok(out)
}
