//! The files a rebuild moves (`super`, step 6): the swap, kept copies, orphans and leftovers.
//! Everything runs under the exclusive `db.lock`. Nothing that may hold records is deleted:
//! the old database and an orphan `-wal` are renamed aside; only `-shm` files (an index of a
//! WAL, rebuilt from it) and finished `rebuild-*` builds (salvaged first) are removed.

use std::path::{Path, PathBuf};

use crate::{StoreError, fsio};

/// How many `project.fdb.corrupt-*` copies are kept (each with its `-wal`/`-shm`).
const KEEP_CORRUPT: usize = 3;

fn side(p: &Path, ext: &str) -> PathBuf {
    PathBuf::from(format!("{}{ext}", p.display()))
}

/// The timestamp of a strict `<prefix><YYYYMMDDTHHMMSSmmmZ>` name.
fn ts_of<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let ts = name.strip_prefix(prefix)?;
    let b = ts.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    (b.len() == 19 && digits(0..8) && b[8] == b'T' && digits(9..18) && b[18] == b'Z').then_some(ts)
}

/// A free `project.fdb.corrupt-<UTC ts>` name in `dir`.
pub(super) fn corrupt_name(dir: &Path) -> PathBuf {
    let mut ms = crate::commit::now_ms();
    loop {
        let c = dir.join(format!(
            "project.fdb.corrupt-{}",
            crate::backup::timestamp(ms)
        ));
        if !c.exists() && !side(&c, "-wal").exists() {
            return c;
        }
        ms += 1;
    }
}

/// The kept damaged copies in `dir`, oldest first.
pub(super) fn corrupt_copies(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let ts = ts_of(&name, "project.fdb.corrupt-")?.to_string();
            Some((ts, e.path()))
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

/// Unfinished builds (`project.fdb.rebuild-*`, without `-wal`/`-shm`) in `dir`. Under the
/// exclusive lock none is in progress.
pub(super) fn stale_builds(dir: &Path, except: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p != except)
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("project.fdb.rebuild-")
                    && !n.ends_with("-wal")
                    && !n.ends_with("-shm")
            })
        })
        .collect()
}

pub(super) fn remove_with_sides(p: &Path) {
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(side(p, ext));
    }
}

/// Keeps the newest [`KEEP_CORRUPT`] damaged copies, never `new`.
pub(super) fn prune_corrupt(dir: &Path, new: Option<&Path>) {
    let all: Vec<PathBuf> = corrupt_copies(dir)
        .into_iter()
        .filter(|p| Some(p.as_path()) != new)
        .collect();
    let keep_others = KEEP_CORRUPT.saturating_sub(usize::from(new.is_some()));
    let excess = all.len().saturating_sub(keep_others);
    for p in all.into_iter().take(excess) {
        remove_with_sides(&p);
    }
}

fn rename_if_present(from: &Path, to: &Path) -> Result<bool, StoreError> {
    match fsio::rename_noreplace(from, to) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(StoreError::io(from)(e)),
    }
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static FAIL_SWAP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test hook: the next swap on this thread fails after moving the old database away.
#[cfg(any(test, feature = "test-hooks"))]
pub fn fail_next_rebuild_swap_for_test() {
    FAIL_SWAP.with(|f| f.set(true));
}

/// Moves `tmp` into place at `db_path`. With an old database, it goes to `kept` first, with
/// its `-wal`/`-shm` (so that WAL is never replayed onto the new file); if linking the new
/// file then fails, the old one is moved back. Without one, an orphan `-wal` is renamed to
/// `project.fdb.orphan-<ts>-wal` and a stale `-shm` removed. Returns whether the old database
/// went to `kept`.
pub(super) fn swap(db_path: &Path, tmp: &Path, kept: Option<&Path>) -> Result<bool, StoreError> {
    let dir = db_path.parent().unwrap_or(Path::new("."));
    let mut moved = false;
    if let Some(kept) = kept {
        moved = rename_if_present(db_path, kept)?;
        for ext in ["-wal", "-shm"] {
            rename_if_present(&side(db_path, ext), &side(kept, ext))?;
        }
    } else {
        let wal = side(db_path, "-wal");
        if wal.exists() {
            let ts = crate::backup::timestamp(crate::commit::now_ms());
            rename_if_present(&wal, &dir.join(format!("project.fdb.orphan-{ts}-wal")))?;
        }
        match std::fs::remove_file(side(db_path, "-shm")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(StoreError::io(&side(db_path, "-shm"))(e)),
        }
    }
    let linked = (|| -> Result<(), StoreError> {
        #[cfg(any(test, feature = "test-hooks"))]
        if FAIL_SWAP.with(|f| f.replace(false)) {
            return Err(fin_core::CoreError::new("TEST_FAULT", "injected swap failure").into());
        }
        fsio::rename_noreplace(tmp, db_path).map_err(StoreError::io(db_path))
    })();
    if let Err(e) = linked {
        if let Some(kept) = kept.filter(|_| moved) {
            let _ = fsio::rename_noreplace(kept, db_path);
            for ext in ["-wal", "-shm"] {
                let _ = fsio::rename_noreplace(&side(kept, ext), &side(db_path, ext));
            }
        }
        return Err(e);
    }
    fsio::sync_dir(dir).map_err(StoreError::io(dir))?;
    Ok(moved)
}
