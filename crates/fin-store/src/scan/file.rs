//! Classification of one file (see the module docs of `scan`).

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

use super::{Cached, Entry, ForeignContent, NotArrivedWhy, Place, SEGMENT_CAP, SNAPSHOT_CAP, Walk};
use crate::conflict_names::{Provider, classify, is_lease_name};
use crate::fsio;
use crate::names::{self, ManifestName, SegmentName, SnapName};
use crate::segment::{self, SegError};
use crate::snapfile;

enum Read {
    Gone,
    Transient,
    Failed(io::Error),
    Bytes(Vec<u8>),
}

fn read(path: &Path) -> Read {
    super::note_read();
    if super::hooked_locked(path) {
        return Read::Transient;
    }
    match fsio::read_uncached(path) {
        Ok(b) => Read::Bytes(b),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Read::Gone,
        Err(e) if fsio::is_transient(&e) => Read::Transient,
        Err(e) => Read::Failed(e),
    }
}

/// OS and sync-client files that are never ours and never worth reporting.
fn is_litter(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        ".ds_store" | "desktop.ini" | "thumbs.db" | ".dropbox" | ".dropbox.attr"
    ) || name.starts_with("._")
        || name.starts_with(".nfs")
        || name.starts_with(".fuse_hidden")
        || name.starts_with(".smbdelete")
}

fn is_sync_temp(name: &str) -> bool {
    name.ends_with(".tmp") && (name.starts_with(".syncthing.") || name.starts_with("~syncthing~"))
}

/// Classifies `path` and pushes its entry (or none, for litter and vanished files).
pub(super) fn entry(w: &mut Walk, path: &Path, meta: &fs::Metadata, place: Place) {
    let foreign = |w: &mut Walk| {
        w.out.push(Entry::Foreign {
            path: path.to_path_buf(),
            conflict_copy: None,
            content: None,
        });
    };
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return foreign(w);
    };
    let not_arrived = |w: &mut Walk, why| {
        w.out.push(Entry::NotArrived {
            path: path.to_path_buf(),
            why,
        });
    };
    if names::is_temp(name) || is_sync_temp(name) {
        return not_arrived(w, NotArrivedWhy::Temp);
    }
    if is_litter(name) {
        return;
    }
    // A symlink, FIFO, socket or device is never opened: a link could leave the project, a
    // FIFO would block forever.
    if !meta.is_file() {
        return foreign(w);
    }
    // Legacy iCloud stub `.<name>.icloud`; then real placeholders, from the directory
    // entry's metadata (on Windows `fs::metadata` would open the file).
    if (name.starts_with('.') && name.ends_with(".icloud"))
        || super::hooked_placeholder(path)
        || fsio::is_placeholder(path, meta)
    {
        return not_arrived(w, NotArrivedWhy::Placeholder);
    }
    match place {
        Place::Root => {
            if !is_lease_name(name) {
                let decode = name.contains(".fseg") || name.contains(".fsnap");
                foreign_file(w, path, name, meta, decode);
            }
        }
        Place::Loose => foreign_file(w, path, name, meta, true),
        Place::Seg(branch) if SegmentName::parse(name).is_some() => {
            canonical(w, path, name, branch, true, meta);
        }
        Place::Snap(branch) if SnapName::parse(name).is_some() => {
            canonical(w, path, name, branch, false, meta);
        }
        Place::Seg(_) | Place::Snap(_) => foreign_file(w, path, name, meta, true),
        Place::Compaction => match ManifestName::parse(name) {
            Some(name) => w.out.push(Entry::Manifest {
                path: path.to_path_buf(),
                name,
            }),
            None => foreign_file(w, path, name, meta, false),
        },
    }
}

fn unreadable(w: &mut Walk, path: &Path, e: &io::Error) {
    w.out.push(Entry::Unreadable {
        path: path.to_path_buf(),
        error: e.to_string(),
    });
}

fn foreign_file(w: &mut Walk, path: &Path, name: &str, meta: &fs::Metadata, decode: bool) {
    let content = if decode && meta.len() <= SEGMENT_CAP {
        match read(path) {
            Read::Bytes(b) => decode_any(&b),
            Read::Gone => return,
            Read::Transient => None,
            Read::Failed(e) => return unreadable(w, path, &e),
        }
    } else {
        None
    };
    let conflict_copy = classify(name).or(content.as_ref().map(|_| Provider::Unknown));
    w.out.push(Entry::Foreign {
        path: path.to_path_buf(),
        conflict_copy,
        content,
    });
}

/// Decodes a foreign file as a segment or snapshot. Files without a Finisher magic are not
/// ours: `None`, not `Damaged`.
fn decode_any(bytes: &[u8]) -> Option<ForeignContent> {
    let e1 = match segment::decode(bytes) {
        Ok(s) => return Some(ForeignContent::Segment(Arc::new(s))),
        Err(e) => e,
    };
    let e2 = match snapfile::decode(bytes) {
        Ok(s) => return Some(ForeignContent::Snapshot(s.header)),
        Err(e) => e,
    };
    for e in [&e1, &e2] {
        match e {
            SegError::Newer { min_reader } => {
                return Some(ForeignContent::Newer {
                    min_reader: *min_reader,
                });
            }
            SegError::UnsupportedHash => return Some(ForeignContent::UnsupportedHash),
            _ => {}
        }
    }
    let head = &bytes[..bytes.len().min(512)];
    let has = |m: &[u8]| head.windows(m.len()).any(|w| w == m);
    if has(b"\"magic\":\"finisher-snapshot\"") {
        Some(ForeignContent::Damaged(e2.to_string()))
    } else if has(b"\"magic\":\"finisher-segment\"") {
        Some(ForeignContent::Damaged(e1.to_string()))
    } else {
        None
    }
}

/// A file with a canonical name in its branch directory.
fn canonical(w: &mut Walk, path: &Path, name: &str, branch: &str, seg: bool, meta: &fs::Metadata) {
    let modified = meta.modified().ok();
    let damaged = |reason: String| Entry::Damaged {
        path: path.to_path_buf(),
        reason,
        len: meta.len(),
        modified,
    };
    if meta.len() > if seg { SEGMENT_CAP } else { SNAPSHOT_CAP } {
        return w.out.push(damaged("too large".into()));
    }
    if let (Some(c), Some(m)) = (w.old.remove(path), modified)
        && c.len == meta.len()
        && c.modified == m
    {
        w.out.push(c.entry.clone());
        w.new.insert(path.to_path_buf(), c);
        return;
    }
    let bytes = match read(path) {
        Read::Bytes(b) => b,
        Read::Gone => return,
        Read::Transient => {
            return w.out.push(Entry::NotArrived {
                path: path.to_path_buf(),
                why: NotArrivedWhy::Locked,
            });
        }
        Read::Failed(e) => return unreadable(w, path, &e),
    };
    let wrong_name = |content| Entry::Foreign {
        path: path.to_path_buf(),
        conflict_copy: Some(Provider::Unknown),
        content,
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
                    seg: Arc::new(s),
                }
            } else {
                wrong_name(Some(ForeignContent::Segment(Arc::new(s))))
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
                wrong_name(Some(ForeignContent::Snapshot(s.header)))
            }
        })
    };
    let e = match result {
        Ok(e) => e,
        Err(SegError::Incomplete) => Entry::NotArrived {
            path: path.to_path_buf(),
            why: NotArrivedWhy::Incomplete,
        },
        Err(SegError::Corrupt(reason)) => damaged(reason),
        Err(SegError::Newer { min_reader }) => Entry::Newer {
            path: path.to_path_buf(),
            min_reader,
        },
        Err(SegError::UnsupportedHash) => Entry::UnsupportedHash {
            path: path.to_path_buf(),
        },
    };
    if let (Some(modified), true) = (
        modified,
        matches!(e, Entry::Segment { .. } | Entry::Snapshot { .. }),
    ) {
        w.new.insert(
            path.to_path_buf(),
            Cached {
                len: meta.len(),
                modified,
                entry: e.clone(),
            },
        );
    }
    w.out.push(e);
}
