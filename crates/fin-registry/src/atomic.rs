//! Durable, atomic file writes: a temp file in the same directory, `fsync`, rename, then
//! `fsync` of the directory (Unix). `tough`'s own datastore writes are neither (8A Task 1,
//! gap 1), so nothing trusted is written through it.

use std::fs;
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
    write_with(path, bytes, |tmp| {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(tmp)
    })
}

/// [`write`] for secrets: the temp file is created owner-only ([`crate::private`]: mode 0600
/// on Unix, a protected owner-only DACL on Windows), so the bytes are never readable by
/// others, not even before the rename, and the file keeps that once renamed.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_with(path, bytes, crate::private::create_new)
}

fn write_with(
    path: &Path,
    bytes: &[u8],
    create: impl FnOnce(&Path) -> std::io::Result<fs::File>,
) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    let r = (|| {
        let mut f = create(&tmp)?;
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
        fs::File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_files_are_owner_only_from_creation() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("k.pem");
        super::write_private(&p, b"secret").unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Replacing a key keeps it owner-only too.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::write_private(&p, b"other").unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"other");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 1;

    fn listing(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// A reader that opened the file the way `std` does (sharing delete) does not stop the
    /// replace: it keeps reading the old bytes, a new open reads the new ones.
    #[test]
    fn replaces_a_file_open_for_reading() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("current");
        super::write(&p, b"1").unwrap();
        let mut held = std::fs::File::open(&p).unwrap();
        super::write(&p, b"2").unwrap();
        super::write_private(&p, b"3").unwrap();
        let mut old = String::new();
        held.read_to_string(&mut old).unwrap();
        assert_eq!(old, "1");
        assert_eq!(std::fs::read(&p).unwrap(), b"3");
        drop(held);
        assert_eq!(listing(tmp.path()), ["current"]);
    }

    /// A program that opened the file without sharing delete makes the rename fail: the write
    /// is refused, the old bytes stay and no temp file is left.
    #[test]
    fn a_file_held_without_share_delete_is_left_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("current");
        super::write(&p, b"1").unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&p)
            .unwrap();
        let e = super::write(&p, b"2").unwrap_err();
        assert!(
            matches!(e.raw_os_error(), Some(5 | 32)),
            "access denied or sharing violation, got {e:?}"
        );
        drop(held);
        assert_eq!(std::fs::read(&p).unwrap(), b"1");
        assert_eq!(listing(tmp.path()), ["current"]);
        super::write(&p, b"2").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"2");
    }
}
