//! Eviction (6B-1 plan Decision 12) and the volume facts it needs: least recently used first,
//! unpinned entries only, down to a target; the cap's default `min(50 GiB, 15 % of the
//! volume)`; an entry's removal that never leaves a row naming a half-deleted directory.

// The cache's own directories under server data, never a library path.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use fin_core::CoreError;
use fin_media::cache::{CacheGcResult, CacheKey};

use super::index::{Index, cache_io};

pub const GIB: u64 = 1 << 30;

/// The cap's upper bound without `media.cacheMaxGiB`.
pub const DEFAULT_CAP_MAX: u64 = 50 * GIB;

/// `min(50 GiB, 15 % of the volume)`; 50 GiB when the volume's size is unknown.
pub fn default_cap(volume_total: Option<u64>) -> u64 {
    match volume_total {
        Some(total) => DEFAULT_CAP_MAX.min((u128::from(total) * 15 / 100) as u64),
        None => DEFAULT_CAP_MAX,
    }
}

/// The volume holding `path`: `(total, available to this user)` in bytes.
#[cfg(unix)]
// `statvfs`'s field types differ per OS (`u32` block counts on macOS).
#[allow(clippy::unnecessary_cast)]
pub fn volume_space(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a NUL-terminated path and `st` points to writable storage for one
    // `statvfs`, which the call fills on success.
    let rc = unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so it initialised `st`.
    let st = unsafe { st.assume_init() };
    let unit = st.f_frsize as u64;
    Ok((
        (st.f_blocks as u64).saturating_mul(unit),
        (st.f_bavail as u64).saturating_mul(unit),
    ))
}

/// The volume holding `path`: `(total, available to this user)` in bytes.
#[cfg(windows)]
pub fn volume_space(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated; the out pointers are valid `u64`s and the third (total
    // free bytes) may be null.
    let ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((total, free))
}

/// Where an entry lives: `<root>/<key[0..2]>/<key>/`.
pub fn entry_dir(root: &Path, key: &CacheKey) -> PathBuf {
    root.join(key.shard()).join(key.as_str())
}

/// The trash, emptied at every open: an evicted entry's directory is renamed here before its
/// row goes, so a crash leaves a row without a directory (dropped at start), never a
/// directory that is half removed under a row.
pub fn trash_dir(root: &Path) -> PathBuf {
    root.join(".trash")
}

/// Removes a directory (or a link or file in its place) without following a link.
pub fn remove_any(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Removes the entry `key`: its directory to the trash, the row (its projects and pins with
/// it), then the trash copy. `seq` (with the process id) makes the trash name unique.
pub fn remove_entry(root: &Path, index: &Index, key: &CacheKey, seq: u64) -> Result<(), CoreError> {
    let dir = entry_dir(root, key);
    let trash = trash_dir(root);
    std::fs::create_dir_all(&trash)
        .map_err(|e| cache_io("io", format!("cannot make the cache's trash: {e}")))?;
    let gone = trash.join(format!("{key}-{}-{seq}", std::process::id()));
    match std::fs::rename(&dir, &gone) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(cache_io("io", format!("cannot move an evicted entry: {e}"))),
    }
    index.delete(key.as_str())?;
    // A failure here leaves bytes in the trash until the next open; the entry is gone.
    let _ = remove_any(&gone);
    Ok(())
}

/// Evicts unpinned entries, least recently used first, until the cache holds at most
/// `target` bytes; `keep` is never evicted (the entry a commit has just put in place).
pub fn evict_to(
    root: &Path,
    index: &Index,
    target: u64,
    keep: Option<&str>,
    seq: &mut u64,
) -> Result<CacheGcResult, CoreError> {
    let mut bytes = index.total_bytes()?;
    let mut out = CacheGcResult::default();
    if bytes > target {
        for row in index.eviction_order()? {
            if bytes <= target {
                break;
            }
            if Some(row.key.as_str()) == keep {
                continue;
            }
            let Ok(key) = CacheKey::parse(&row.key) else {
                index.delete(&row.key)?;
                continue;
            };
            *seq += 1;
            // An entry that cannot be moved now (Windows: a file still open) stays for the next
            // pass; the index failing is an error.
            match remove_entry(root, index, &key, *seq) {
                Ok(()) => {}
                Err(e) if e.details["reason"] == "index" => return Err(e),
                Err(_) => continue,
            }
            bytes = bytes.saturating_sub(row.bytes);
            out.evicted += 1;
            out.freed_bytes += row.bytes;
        }
    }
    out.bytes = bytes;
    Ok(out)
}
