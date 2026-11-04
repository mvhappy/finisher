//! Scan of a project folder's `.finisher/` directory (sub-project 4 spec §3.4, §3.6, §3.7).
//!
//! Layout read:
//! - `.finisher/*`: files only. Our own names (`lease.json`, claims, probes) are skipped, as
//!   are directories. Anything else is foreign, so a conflict copy of `lease.json` shows up.
//! - `.finisher/segments/<branch>/*` and `.finisher/snapshots/<branch>/*`: every file.
//!
//! `scan` is stateless. It re-reads and re-decodes every file on every call, because sync
//! roots hydrate out of order and a file that looks damaged now may be valid on the next
//! poll. Deciding when damage is real (spec: unchanged for 60 s) is the caller's job; that is
//! why [`Entry::Damaged`] carries the file's length and modification time.
//!
//! A placeholder is never read: its status comes from the directory entry's own metadata.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::conflict_names::{Provider, classify};
use crate::fsio;
use crate::names::{self, SegmentName, SnapName};
use crate::segment::{self, SegError, Segment};
use crate::snapfile::{self, SnapHeader};

/// Why a file is not usable yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotArrivedWhy {
    /// A cloud-sync placeholder; hydrate it by reading it (the caller decides when).
    Placeholder,
    /// Short, or no trailer yet.
    Incomplete,
    /// A `.~fin-*.tmp` file: a write in progress or a crash leftover.
    Temp,
}

/// One classified file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A verified segment whose name, directory and header agree.
    Segment {
        branch: String,
        name: SegmentName,
        seg: Segment,
    },
    /// A verified snapshot. The body was decoded to check the trailer, then dropped.
    Snapshot {
        branch: String,
        name: SnapName,
        header: SnapHeader,
        /// `blake3:<64 hex>` of the file.
        hash: String,
    },
    NotArrived {
        path: PathBuf,
        why: NotArrivedWhy,
    },
    /// Anything else. `conflict_copy` is `Some` for a known provider pattern, or
    /// `Some(Unknown)` for a Finisher file stored under a name that is not its own.
    Foreign {
        path: PathBuf,
        conflict_copy: Option<Provider>,
    },
    /// Needs a newer Finisher (`minReaderVersion` above ours).
    Newer {
        path: PathBuf,
        min_reader: u32,
    },
    /// A canonically named file that decodes as `Corrupt`. Not quarantined here: the spec
    /// treats damage as not-arrived until the file stops changing for 60 s, which needs state
    /// across scans. `len` and `modified` let the caller track that.
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
}

/// Scans `<project_dir>/.finisher/`. A missing directory is an empty result. Entries are
/// ordered by directory, then name.
pub fn scan(project_dir: &Path) -> io::Result<Vec<Entry>> {
    let base = project_dir.join(".finisher");
    let mut out = Vec::new();
    for (path, meta) in list(&base)? {
        if !meta.is_dir() {
            file(&path, &meta, Place::Root, &mut out)?;
        }
    }
    for (place, sub) in [
        (Place::Segments, "segments"),
        (Place::Snapshots, "snapshots"),
    ] {
        for (path, meta) in list(&base.join(sub))? {
            if !meta.is_dir() {
                file(&path, &meta, Place::Loose, &mut out)?;
                continue;
            }
            let branch = path.file_name().unwrap().to_string_lossy().into_owned();
            for (path, meta) in list(&path)? {
                if !meta.is_dir() {
                    file(&path, &meta, place.in_branch(&branch), &mut out)?;
                }
            }
        }
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum Place<'a> {
    /// Directly in `.finisher/`.
    Root,
    /// Directly in `segments/` or `snapshots/`, outside any branch directory.
    Loose,
    Segments,
    Snapshots,
    Seg(&'a str),
    Snap(&'a str),
}

impl<'a> Place<'a> {
    fn in_branch(self, branch: &'a str) -> Place<'a> {
        match self {
            Place::Segments => Place::Seg(branch),
            _ => Place::Snap(branch),
        }
    }
}

/// The directory's entries with their `DirEntry::metadata()` (symlinks are not followed),
/// sorted by name.
fn list(dir: &Path) -> io::Result<Vec<(PathBuf, fs::Metadata)>> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound || is_not_dir(&e) => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut v = Vec::new();
    for e in rd {
        let e = e?;
        v.push((e.path(), e.metadata()?));
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(v)
}

fn is_not_dir(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::NotADirectory
}

/// Classifies one regular file (anything but a directory) and pushes its entry.
fn file(path: &Path, meta: &fs::Metadata, place: Place, out: &mut Vec<Entry>) -> io::Result<()> {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let not_arrived = |why| Entry::NotArrived {
        path: path.to_path_buf(),
        why,
    };
    if names::is_temp(&name) {
        out.push(not_arrived(NotArrivedWhy::Temp));
        return Ok(());
    }
    // Legacy iCloud stub: `.<name>.icloud`.
    if name.starts_with('.') && name.ends_with(".icloud") {
        out.push(not_arrived(NotArrivedWhy::Placeholder));
        return Ok(());
    }
    // Directory-entry metadata (passed in): on Windows `fs::metadata` would open the file.
    if is_placeholder(path, meta) {
        out.push(not_arrived(NotArrivedWhy::Placeholder));
        return Ok(());
    }
    let hostnames: [String; 0] = [];
    let foreign = |conflict_copy| Entry::Foreign {
        path: path.to_path_buf(),
        conflict_copy,
    };
    match place {
        Place::Root => {
            if !crate::conflict_names::is_own_name(&name) {
                out.push(foreign(classify(&name, &hostnames)));
            }
        }
        Place::Seg(branch) | Place::Snap(branch) => {
            let seg = matches!(place, Place::Seg(_));
            let canonical = if seg {
                SegmentName::parse(&name).is_some()
            } else {
                SnapName::parse(&name).is_some()
            };
            if canonical {
                return canonical_file(path, &name, branch, seg, meta, out);
            }
            if let Some(p) = classify(&name, &hostnames) {
                out.push(foreign(Some(p)));
            } else if (name.contains(".fseg") || name.contains(".fsnap")) && decodes(path)? {
                out.push(foreign(Some(Provider::Unknown)));
            } else {
                out.push(foreign(None));
            }
        }
        Place::Loose | Place::Segments | Place::Snapshots => {
            out.push(foreign(classify(&name, &hostnames)));
        }
    }
    Ok(())
}

/// A file with a canonical name in its branch directory.
fn canonical_file(
    path: &Path,
    name: &str,
    branch: &str,
    seg: bool,
    meta: &fs::Metadata,
    out: &mut Vec<Entry>,
) -> io::Result<()> {
    let Some(bytes) = read(path)? else {
        return Ok(());
    };
    let Some(bytes) = bytes else {
        out.push(Entry::NotArrived {
            path: path.to_path_buf(),
            why: NotArrivedWhy::Incomplete,
        });
        return Ok(());
    };
    let wrong_name = || Entry::Foreign {
        path: path.to_path_buf(),
        conflict_copy: Some(Provider::Unknown),
    };
    let result = if seg {
        segment::decode(&bytes).map(|s| {
            let n = SegmentName::parse(name).unwrap();
            let h = &s.header;
            let ok = h.branch == branch
                && n.epoch() == h.epoch
                && n.first_seq() == h.first_seq
                && n.last_seq() == h.last_seq
                && names::hash12(&s.hash) == Some(n.hash12());
            if ok {
                Entry::Segment {
                    branch: branch.to_string(),
                    name: n,
                    seg: s,
                }
            } else {
                wrong_name()
            }
        })
    } else {
        // The body (possibly large) is dropped right after the trailer check.
        snapfile::decode(&bytes).map(|s| {
            let n = SnapName::parse(name).unwrap();
            let ok = s.header.branch == branch
                && n.seq() == s.header.seq
                && names::hash12(&s.hash) == Some(n.hash12());
            if ok {
                Entry::Snapshot {
                    branch: branch.to_string(),
                    name: n,
                    header: s.header,
                    hash: s.hash,
                }
            } else {
                wrong_name()
            }
        })
    };
    out.push(match result {
        Ok(e) => e,
        Err(SegError::Incomplete) => Entry::NotArrived {
            path: path.to_path_buf(),
            why: NotArrivedWhy::Incomplete,
        },
        Err(SegError::Corrupt(reason)) => Entry::Damaged {
            path: path.to_path_buf(),
            reason,
            len: meta.len(),
            modified: meta.modified().ok(),
        },
        Err(SegError::Newer { min_reader }) => Entry::Newer {
            path: path.to_path_buf(),
            min_reader,
        },
        Err(SegError::UnsupportedHash) => Entry::UnsupportedHash {
            path: path.to_path_buf(),
        },
    });
    Ok(())
}

/// Whether the file decodes as a segment or snapshot (it is a Finisher file under a foreign
/// name). A file that cannot be read counts as not decoding.
fn decodes(path: &Path) -> io::Result<bool> {
    Ok(matches!(read(path)?, Some(Some(b))
        if segment::decode(&b).is_ok() || snapfile::decode(&b).is_ok()))
}

/// `Ok(None)`: the file vanished. `Ok(Some(None))`: a transient failure (a sync client holds
/// it), treated as not arrived.
fn read(path: &Path) -> io::Result<Option<Option<Vec<u8>>>> {
    match fsio::read_uncached(path) {
        Ok(b) => Ok(Some(Some(b))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) if fsio::is_transient(&e) => Ok(Some(None)),
        Err(e) => Err(e),
    }
}

fn is_placeholder(path: &Path, meta: &fs::Metadata) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    if HOOKED.with(|h| h.borrow().iter().any(|p| p == path)) {
        return true;
    }
    fsio::is_placeholder(path, meta)
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static HOOKED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test hook: [`scan`] on this thread treats `path` as a cloud-sync placeholder.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn mark_placeholder(path: &Path) {
    HOOKED.with(|h| h.borrow_mut().push(path.to_path_buf()));
}

/// Test hook: forgets every [`mark_placeholder`] on this thread.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn clear_placeholders() {
    HOOKED.with(|h| h.borrow_mut().clear());
}
