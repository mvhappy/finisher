//! Durable, atomic file writes: a temp file in the same directory, `fsync`, rename, then
//! `fsync` of the directory (Unix). `tough`'s own datastore writes are neither (8A Task 1,
//! gap 1), so nothing trusted is written through it.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A name for a temp file or directory next to `path`, unique within this process.
pub(crate) fn temp_sibling(path: &Path) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.tmp-{}-{n}", std::process::id()))
}

/// Writes `bytes` to `path` atomically and durably.
pub(crate) fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    let r = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        sync_dir(path.parent().unwrap_or(Path::new(".")))
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// `fsync` of a directory, so a rename inside it survives a crash (a no-op on Windows,
/// where renames are durable once `MoveFileEx` returns and directories cannot be opened).
pub(crate) fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}
