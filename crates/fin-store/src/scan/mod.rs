//! Scan of a project folder's `.finisher/` directory (sub-project 4 spec §3.4, §3.6, §3.7).
//!
//! Layout read:
//! - `.finisher/*`: regular files. Our own lease files (`lease.json`, claims, probes) are
//!   skipped; anything else is foreign, so a conflict copy of `lease.json` shows up.
//! - `.finisher/segments/<branch>/*` and `.finisher/snapshots/<branch>/*`, where `<branch>`
//!   is `main` or `br<digits>`. A directory with any other name is one `Foreign` entry and is
//!   not entered (with `conflict_copy` set when it is a sync client's copy of a branch
//!   directory).
//! - Files directly in `segments/` or `snapshots/` are foreign.
//! - `.finisher/compaction/*`: compaction manifests (spec §3.9) are listed by name only
//!   ([`Entry::Manifest`]; compaction reads them itself). Anything else there is foreign (a
//!   sync client's copy of a manifest is a conflict copy), and so is any directory in it.
//! - Not scanned: any other directory.
//!
//! A path that is not a regular file (symlink, FIFO, socket, device) is `Foreign` and never
//! opened. Known OS and sync litter is skipped: `.DS_Store`, AppleDouble `._*`,
//! `desktop.ini`, `Thumbs.db`, `.dropbox`, `.dropbox.attr`, `.nfs*`, `.fuse_hidden*`,
//! `.smbdelete*`. Sync temp files (`.syncthing.*.tmp`, `~syncthing~*.tmp`) are
//! `NotArrived { Temp }`.
//!
//! Decoding is stateless apart from the [`Scanner`] cache: only *verified* segments and
//! snapshots are cached, keyed by path and `(len, mtime)`. Names are content-addressed and
//! files are write-once, so a hit needs no read. `NotArrived`, `Damaged`, `Foreign` and
//! `Unreadable` are never cached, because sync roots hydrate out of order and a file that
//! looks bad now may be valid on the next poll. Entries whose files leave the listing are
//! evicted. A placeholder is never read; its status comes from the directory entry's own
//! metadata.
//!
//! Size caps (from `metadata().len()`, before reading): a segment above [`SEGMENT_CAP`] or a
//! snapshot above [`SNAPSHOT_CAP`] is `Damaged("too large")`; a foreign file above
//! [`SEGMENT_CAP`] is listed without decoding.

mod file;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::conflict_names::Provider;
use crate::names::{ManifestName, SegmentName, SnapName};
use crate::segment::{self, Segment};
use crate::snapfile::SnapHeader;
use crate::storage_kind::StorageKind;

/// The largest segment file accepted: twice the writer's cap plus slack for a header.
pub const SEGMENT_CAP: u64 = 2 * segment::MAX_BYTES as u64 + 64 * 1024;
/// The largest snapshot file accepted. A snapshot holds the whole model on one line.
pub const SNAPSHOT_CAP: u64 = 256 << 20;

/// Why a file is not usable yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotArrivedWhy {
    /// A cloud-sync placeholder; hydrate it by reading it (the caller decides when).
    Placeholder,
    /// Short, or no trailer yet.
    Incomplete,
    /// Momentarily unreadable: a transient error ([`crate::fsio::is_transient`]), as a sync
    /// client or another app holding it open without sharing (Windows 32), or a file deleted
    /// while another SMB client still holds it open (pending deletion: Windows 5) (spec §3.13
    /// items 13, 15).
    Locked,
    /// A `.~fin-*.tmp` or sync-client temp file.
    Temp,
}

/// What a foreign file decodes to, for import into quarantine (spec §3.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForeignContent {
    Segment(Arc<Segment>),
    Snapshot(SnapHeader),
    Newer {
        min_reader: u32,
    },
    /// Carries a Finisher magic but does not verify.
    Damaged(String),
    UnsupportedHash,
}

/// One classified file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A verified segment whose name, directory and header agree.
    Segment {
        branch: String,
        name: SegmentName,
        seg: Arc<Segment>,
    },
    /// A verified snapshot. The body was decoded to check the trailer, then dropped.
    Snapshot {
        branch: String,
        name: SnapName,
        header: SnapHeader,
        /// `blake3:<64 hex>` of the file.
        hash: String,
    },
    /// A compaction manifest (`compaction/<ulid>.json`), not read by the scan.
    Manifest {
        path: PathBuf,
        name: ManifestName,
    },
    NotArrived {
        path: PathBuf,
        why: NotArrivedWhy,
    },
    /// Anything else. `conflict_copy` is `Some` for a known provider pattern, or
    /// `Some(Unknown)` for a Finisher file stored under a name that is not its own.
    /// `content` is the decoded Finisher file, if it is one.
    Foreign {
        path: PathBuf,
        conflict_copy: Option<Provider>,
        content: Option<ForeignContent>,
    },
    /// Needs a newer Finisher (`minReaderVersion` above ours).
    Newer {
        path: PathBuf,
        min_reader: u32,
    },
    /// A canonically named file that decodes as `Corrupt` (or is too large). The spec treats
    /// damage as not arrived until the file stops changing for 60 s, which needs state across
    /// scans; `len` and `modified` let the caller track that.
    Damaged {
        path: PathBuf,
        reason: String,
        len: u64,
        modified: Option<SystemTime>,
    },
    /// A newer format revision uses a hash algorithm this code does not know
    /// (`UNSUPPORTED_HASH`).
    UnsupportedHash {
        path: PathBuf,
    },
    /// A read failed for a reason that is not transient (permissions, I/O error, a cloud
    /// file that cannot be fetched). The rest of the scan is unaffected.
    Unreadable {
        path: PathBuf,
        error: String,
    },
}

struct Cached {
    len: u64,
    modified: SystemTime,
    entry: Entry,
}

/// A scan in progress.
struct Walk {
    old: HashMap<PathBuf, Cached>,
    new: HashMap<PathBuf, Cached>,
    out: Vec<Entry>,
}

#[derive(Clone, Copy)]
enum Place<'a> {
    /// Directly in `.finisher/`.
    Root,
    /// Directly in `segments/` or `snapshots/`, outside any branch directory.
    Loose,
    Seg(&'a str),
    Snap(&'a str),
    /// Directly in `compaction/`.
    Compaction,
}

/// A scanner that remembers verified files between polls.
#[derive(Default)]
pub struct Scanner {
    cache: HashMap<PathBuf, Cached>,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scans `<project_dir>/.finisher/`. A missing directory is an empty result; only a
    /// directory that cannot be listed fails the scan. Entries are ordered by directory,
    /// then name.
    pub fn scan(&mut self, project_dir: &Path) -> io::Result<Vec<Entry>> {
        self.walk(project_dir, false)
    }

    /// [`Scanner::scan`] of a folder on `kind`, as import and follow need it: fresh. Where the
    /// client caches listings for up to a minute ([`crate::storage_kind::slow_dir_cache`]),
    /// `segments/` and each `segments/<branch>/` are refreshed ([`refresh_listing`]) right
    /// before they are listed.
    pub fn scan_for(&mut self, project_dir: &Path, kind: StorageKind) -> io::Result<Vec<Entry>> {
        self.walk(project_dir, crate::storage_kind::slow_dir_cache(kind))
    }

    /// The scan, refreshing the segment listings first when `refresh`.
    fn walk(&mut self, project_dir: &Path, refresh: bool) -> io::Result<Vec<Entry>> {
        let mut w = Walk {
            old: std::mem::take(&mut self.cache),
            new: HashMap::new(),
            out: Vec::new(),
        };
        let base = project_dir.join(".finisher");
        for (path, meta) in list(&base)? {
            if !meta.is_dir() {
                file::entry(&mut w, &path, &meta, Place::Root);
            }
        }
        for (sub, is_seg) in [("segments", true), ("snapshots", false)] {
            let refresh = refresh && is_seg;
            if refresh {
                refresh_listing(&base.join(sub));
            }
            for (path, meta) in list(&base.join(sub))? {
                if !meta.is_dir() {
                    file::entry(&mut w, &path, &meta, Place::Loose);
                    continue;
                }
                let Some(branch) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|b| valid_branch(b))
                else {
                    // A sync client's copy of a branch directory (`main (conflicted copy)`)
                    // is a conflict copy; it is not entered.
                    let conflict_copy = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .and_then(crate::conflict_names::classify_branch_dir);
                    w.out.push(Entry::Foreign {
                        path,
                        conflict_copy,
                        content: None,
                    });
                    continue;
                };
                if refresh {
                    refresh_listing(&path);
                }
                for (p, m) in list(&path)? {
                    if !m.is_dir() {
                        let place = if is_seg {
                            Place::Seg(branch)
                        } else {
                            Place::Snap(branch)
                        };
                        file::entry(&mut w, &p, &m, place);
                    }
                }
            }
        }
        for (path, meta) in list(&base.join("compaction"))? {
            if meta.is_dir() {
                w.out.push(Entry::Foreign {
                    path,
                    conflict_copy: None,
                    content: None,
                });
            } else {
                file::entry(&mut w, &path, &meta, Place::Compaction);
            }
        }
        self.cache = w.new;
        Ok(w.out)
    }
}

/// An uncached scan: see [`Scanner::scan`].
pub fn scan(project_dir: &Path) -> io::Result<Vec<Entry>> {
    Scanner::new().scan(project_dir)
}

/// Makes the client drop its cached listing of `dir` by creating and removing a file in it:
/// on macOS smbfs a change made through this machine invalidates that directory's cache at
/// once (measured; a `stat` of the directory does not). Best effort: any failure (a
/// read-only share, a missing directory) is ignored and the listing may then be up to a
/// minute old, which only delays following; the ordering rule never relies on this
/// (`tick::MACOS_DIR_CACHE_WINDOW`). The file has a temp name ([`crate::names::temp_name`]), which
/// every reader skips and the holder's temp sweep removes if a crash leaves it. Returns the
/// probe if it was created but could not be removed (outside `.finisher/` nothing sweeps it:
/// the caller does).
pub(crate) fn refresh_listing(dir: &Path) -> Option<PathBuf> {
    let path = dir.join(crate::names::temp_name());
    note_refresh();
    let created = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .is_ok();
    (created && (keep_probe() || fs::remove_file(&path).is_err())).then_some(path)
}

/// `main` or `br<digits>` (spec §3.3).
pub(crate) fn valid_branch(b: &str) -> bool {
    b == "main"
        || b.strip_prefix("br")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

/// The directory's entries with their `DirEntry::metadata()` (symlinks are not followed),
/// sorted by path. A missing directory is empty; an entry that vanishes mid-listing (a
/// writer's temp file renamed) is skipped.
fn list(dir: &Path) -> io::Result<Vec<(PathBuf, fs::Metadata)>> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(vec![]);
        }
        Err(e) => return Err(e),
    };
    let mut v = Vec::new();
    for e in rd {
        let e = e?;
        #[cfg(any(test, feature = "test-hooks"))]
        if is_hidden(&e.path()) {
            continue;
        }
        match e.metadata() {
            Ok(m) => v.push((e.path(), m)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(v)
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static HOOKED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    static LOCKED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    static READS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static REFRESHES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static KEEP_PROBES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test hook: on this thread, [`refresh_listing`] leaves its probe file in place, as when
/// removing it fails.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn keep_refresh_probes_for_test(keep: bool) {
    KEEP_PROBES.with(|k| k.set(keep));
}

fn keep_probe() -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    if KEEP_PROBES.with(std::cell::Cell::get) {
        return true;
    }
    false
}

fn hooked_placeholder(path: &Path) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    return HOOKED.with(|h| h.borrow().iter().any(|p| p == path));
    #[cfg(not(any(test, feature = "test-hooks")))]
    {
        let _ = path;
        false
    }
}

/// Whether a read of `path` fails as with a transient error (test hook [`lock_for_test`]).
fn hooked_locked(path: &Path) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    return LOCKED.with(|h| h.borrow().iter().any(|p| p == path));
    #[cfg(not(any(test, feature = "test-hooks")))]
    {
        let _ = path;
        false
    }
}

/// Test hook: on this thread, scans read `path` as momentarily locked (a transient error, as
/// a Windows sharing violation or a file pending deletion that another client holds open);
/// `false` undoes it.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn lock_for_test(path: &Path, locked: bool) {
    LOCKED.with(|h| {
        let mut h = h.borrow_mut();
        h.retain(|p| p != path);
        if locked {
            h.push(path.to_path_buf());
        }
    });
}

fn note_read() {
    #[cfg(any(test, feature = "test-hooks"))]
    READS.with(|n| n.set(n.get() + 1));
}

fn note_refresh() {
    #[cfg(any(test, feature = "test-hooks"))]
    REFRESHES.with(|n| n.set(n.get() + 1));
}

/// Test hook: scans on this thread treat `path` as a cloud-sync placeholder.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn mark_placeholder(path: &Path) {
    HOOKED.with(|h| h.borrow_mut().push(path.to_path_buf()));
}

/// A placeholder at `path` was read through, which hydrates it. Real placeholders lose their
/// placeholder state by themselves; under test hooks this forgets a [`mark_placeholder`].
pub(crate) fn hydrated(path: &Path) {
    #[cfg(any(test, feature = "test-hooks"))]
    HOOKED.with(|h| h.borrow_mut().retain(|p| p != path));
    #[cfg(not(any(test, feature = "test-hooks")))]
    let _ = path;
}

/// Test hook: forgets every [`mark_placeholder`] on this thread.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn clear_placeholders() {
    HOOKED.with(|h| h.borrow_mut().clear());
}

/// Test hook: how many file reads scans on this thread have made.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn reads() -> u32 {
    READS.with(std::cell::Cell::get)
}

/// Test hook: how many listing refreshes ([`refresh_listing`]) this thread has attempted.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn refreshes() -> u32 {
    REFRESHES.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-hooks"))]
fn hidden_set() -> &'static std::sync::Mutex<std::collections::HashSet<PathBuf>> {
    static HIDDEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    HIDDEN.get_or_init(Default::default)
}

#[cfg(any(test, feature = "test-hooks"))]
fn is_hidden(path: &Path) -> bool {
    hidden_set()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(path)
}

/// Test hook: scans (on any thread) leave `path` out of their listing until
/// [`unhide_for_test`], as a directory cache that has not seen a new file yet does (the
/// Windows SMB client: up to 10 s).
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn hide_for_test(path: &Path) {
    hidden_set()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(path.to_path_buf());
}

/// Test hook: undoes [`hide_for_test`].
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn unhide_for_test(path: &Path) {
    hidden_set()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(path);
}
