//! An in-memory [`Source`] for tests (feature `test-hooks`): a tree with raw names, OS
//! attributes, reparse tags, nested mounts and automounts, per-directory delays and errors,
//! and counters that prove what a walk touched ([`FakeStats`]).
//!
//! JSON spec ([`FakeSource::from_json`]), camelCase, unknown fields refused:
//! ```json
//! { "volume": { "key": "k1", "kind": "ext4", "devId": "1", "parentDevId": "0",
//!               "mountRootAttr": null, "caseSensitive": true, "remote": false,
//!               "rootFileId": "01" },
//!   "mtimeNs": 0, "statDelayMs": 0, "statError": null, "readDelayMs": 0, "error": null,
//!   "children": [
//!     { "name": "a.mov", "size": 10, "mtimeNs": 5, "fileId": "0a" },
//!     { "nameHex": "ff2e6d6f76" },
//!     { "nameWide": [55296, 46, 109] },
//!     { "name": "cloud", "type": "dir", "macosFlags": 1073741824, "children": [] } ] }
//! ```
//! A node's `type` defaults to `dir` when it has `children`, else `file`; ids and reparse
//! tags are hex; `dev` defaults to its directory's. `windowsAttributes`, `reparseTag`,
//! `mountRoot`, `automount`, `content` (a file's bytes; `size` defaults to its length),
//! `openError` (for `open_dir`), `error` and `readDelayMs` (for `read_dir`) are optional.
//! Errors: `notFound`, `permissionDenied`, `timedOut`, `io`, `unsupported`,
//! `invalidParameter`.

use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::Duration;

pub(crate) mod spec;

pub use spec::FakeError;
use spec::{Node, Tree, error, parse};

use super::{DirHandle, RawEntry, RootStat, Source};
use crate::raw::RawRel;
use crate::types::{EntryType, Expect, OpenError, VolumeIdentity};

/// What the source was asked to do. `placeholder_touches` counts every `open_dir` or
/// `read_dir` of a placeholder directory and every file open that reached a placeholder's
/// data; `automount_triggers` every `open_dir` of an automount point. A correct walk leaves
/// both at zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeStats {
    pub stat_roots: u64,
    pub dir_opens: u64,
    pub dir_reads: u64,
    pub file_opens: u64,
    pub placeholder_touches: u64,
    pub automount_triggers: u64,
    /// `stat_child` calls (polls of unchanged directories).
    pub entry_stats: u64,
    /// `open_dir` calls per directory.
    pub opens_by_dir: BTreeMap<RawRel, u64>,
    /// `read_dir` calls per directory.
    pub reads_by_dir: BTreeMap<RawRel, u64>,
}

/// The fake filesystem. [`FakeSource::reload`] swaps the tree (renames, deletions, unmounts
/// between walks); counters survive it.
#[derive(Debug)]
pub struct FakeSource {
    tree: Mutex<Tree>,
    stats: Mutex<FakeStats>,
}

fn is_placeholder(e: &RawEntry) -> bool {
    crate::placeholder::from_attrs(e.attrs).is_some()
}

/// The nodes along `rel` (the last one is `rel`'s), or `NotFound`.
fn walk<'t>(children: &'t [Node], rel: &RawRel) -> io::Result<Vec<&'t Node>> {
    let mut level = children;
    let mut path = Vec::with_capacity(rel.depth());
    for c in rel.components() {
        let n = level
            .iter()
            .find(|n| &n.entry.name == c)
            .ok_or_else(|| error("notFound"))?;
        path.push(n);
        level = &n.children;
    }
    Ok(path)
}

/// The root's facts from the tree (no counting, no delay).
fn root_stat(t: &Tree) -> RootStat {
    let v = &t.root.volume;
    let entries: Vec<RawEntry> = t.children.iter().map(|n| n.entry.clone()).collect();
    RootStat {
        key: v.key.clone(),
        kind: v.kind.clone(),
        dev_id: v.dev_id.clone(),
        parent_dev_id: v.parent_dev_id.clone(),
        mount_root_attr: v.mount_root_attr,
        case_sensitive: v.case_sensitive,
        enumeration: "fake".into(),
        remote: v.remote,
        root_file_id: t.root_file_id.clone(),
        sample: crate::volume::sample(&entries),
        entries: entries.len() as u64,
    }
}

/// The fake's directory marker.
#[derive(Debug, Clone, Copy)]
struct FakeDir;

impl FakeSource {
    pub fn from_json(spec: &str) -> Result<FakeSource, FakeError> {
        Ok(FakeSource {
            tree: Mutex::new(parse(spec)?),
            stats: Mutex::new(FakeStats::default()),
        })
    }

    /// Replaces the tree; the counters are kept.
    pub fn reload(&self, spec: &str) -> Result<(), FakeError> {
        *self.tree.lock().unwrap() = parse(spec)?;
        Ok(())
    }

    pub fn stats(&self) -> FakeStats {
        self.stats.lock().unwrap().clone()
    }

    fn count(&self, f: impl FnOnce(&mut FakeStats)) {
        f(&mut self.stats.lock().unwrap());
    }

    /// The fake's `open_for_read` (Decision 19), for tests of code that reads files: a
    /// placeholder is `CloudOnly` unless `hydrate`; a link component or file is
    /// `NameSurrogate`; a different size, mtime or file id is `Changed`. The reader yields
    /// the node's `content`, or `size` zero bytes.
    pub fn open_file(
        &self,
        rel: &RawRel,
        expect: &Expect,
        hydrate: bool,
    ) -> Result<FakeFile, OpenError> {
        rel.validate().map_err(|_| OpenError::Escapes)?;
        let tree = self.tree.lock().unwrap();
        let path = walk(&tree.children, rel).map_err(|_| OpenError::NotFound)?;
        let Some(file) = path.last() else {
            return Err(OpenError::Io(io::Error::from(io::ErrorKind::IsADirectory)));
        };
        let link = |n: &Node| {
            n.entry.entry_type == EntryType::Symlink
                || n.entry
                    .reparse_tag
                    .is_some_and(crate::reparse::is_name_surrogate)
        };
        if path.iter().any(|n| link(n)) {
            return Err(OpenError::NameSurrogate);
        }
        if path[..path.len() - 1]
            .iter()
            .any(|n| is_placeholder(&n.entry))
            && !hydrate
        {
            return Err(OpenError::CloudOnly);
        }
        match file.entry.entry_type {
            EntryType::File => {}
            EntryType::Dir => {
                return Err(OpenError::Io(io::Error::from(io::ErrorKind::IsADirectory)));
            }
            _ => return Err(OpenError::NotFound),
        }
        let placeholder = is_placeholder(&file.entry);
        if placeholder && !hydrate {
            return Err(OpenError::CloudOnly);
        }
        let e = &file.entry;
        if !expect.matches(e.size, e.mtime_ns, e.file_id.as_deref()) {
            return Err(OpenError::Changed);
        }
        let f = FakeFile {
            data: file.content.clone(),
            len: e.size,
            pos: 0,
        };
        drop(tree);
        self.count(|s| {
            s.file_opens += 1;
            s.placeholder_touches += u64::from(placeholder);
        });
        Ok(f)
    }
}

impl Source for FakeSource {
    fn stat_root(&self) -> io::Result<RootStat> {
        self.count(|s| s.stat_roots += 1);
        let (delay, err) = {
            let t = self.tree.lock().unwrap();
            (
                Duration::from_millis(t.root.stat_delay_ms),
                t.root.stat_error.clone(),
            )
        };
        std::thread::sleep(delay);
        if let Some(e) = err {
            return Err(error(&e));
        }
        Ok(root_stat(&self.tree.lock().unwrap()))
    }

    fn open_dir(&self, rel: &RawRel) -> io::Result<DirHandle> {
        let tree = self.tree.lock().unwrap();
        let path = walk(&tree.children, rel);
        let (placeholder, automount, outcome) = match &path {
            Err(_) => (false, false, Err(error("notFound"))),
            Ok(p) => {
                let last = p.last().map(|n| &n.entry);
                let not_dir = p.iter().any(|n| {
                    n.entry.entry_type != EntryType::Dir
                        || n.entry
                            .reparse_tag
                            .is_some_and(crate::reparse::is_name_surrogate)
                });
                let open_error = match p.last() {
                    Some(n) => n.open_error.clone(),
                    None => None,
                };
                let outcome = if not_dir {
                    Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "fake notADirectory",
                    ))
                } else if let Some(e) = open_error {
                    Err(error(&e))
                } else {
                    Ok(())
                };
                (
                    p.iter().any(|n| is_placeholder(&n.entry)),
                    last.is_some_and(|e| e.automount),
                    outcome,
                )
            }
        };
        drop(tree);
        self.count(|s| {
            s.dir_opens += 1;
            *s.opens_by_dir.entry(rel.clone()).or_default() += 1;
            s.placeholder_touches += u64::from(placeholder);
            s.automount_triggers += u64::from(automount);
        });
        outcome.map(|()| DirHandle::new(rel.clone(), FakeDir))
    }

    fn read_dir(&self, dir: &DirHandle) -> io::Result<Vec<RawEntry>> {
        if dir.os::<FakeDir>().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a fake handle",
            ));
        }
        let (delay, outcome, placeholder) = {
            let t = self.tree.lock().unwrap();
            if dir.rel().is_root() {
                let entries = t.children.iter().map(|n| n.entry.clone()).collect();
                let out = match &t.root.error {
                    Some(e) => Err(error(e)),
                    None => Ok(entries),
                };
                (Duration::from_millis(t.root.read_delay_ms), out, false)
            } else {
                match walk(&t.children, dir.rel()) {
                    Err(e) => (Duration::ZERO, Err(e), false),
                    Ok(p) => {
                        let n = p[p.len() - 1];
                        let out = match &n.error {
                            Some(e) => Err(error(e)),
                            None => Ok(n.children.iter().map(|c| c.entry.clone()).collect()),
                        };
                        (
                            n.read_delay,
                            out,
                            p.iter().any(|n| is_placeholder(&n.entry)),
                        )
                    }
                }
            }
        };
        self.count(|s| {
            s.dir_reads += 1;
            *s.reads_by_dir.entry(dir.rel().clone()).or_default() += 1;
            s.placeholder_touches += u64::from(placeholder);
        });
        std::thread::sleep(delay);
        outcome
    }

    fn stat_child(&self, parent: &DirHandle, name: &[u8]) -> io::Result<Option<RawEntry>> {
        self.count(|s| s.entry_stats += 1);
        let t = self.tree.lock().unwrap();
        let level = if parent.rel().is_root() {
            &t.children
        } else {
            &walk(&t.children, parent.rel())?
                .last()
                .expect("not the root")
                .children
        };
        Ok(level
            .iter()
            .find(|n| n.entry.name == name)
            .map(|n| n.entry.clone()))
    }

    fn dir_mtime(&self, dir: &DirHandle) -> io::Result<Option<i64>> {
        let t = self.tree.lock().unwrap();
        Ok(Some(match walk(&t.children, dir.rel())?.last() {
            Some(n) => n.entry.mtime_ns,
            None => t.root.mtime_ns,
        }))
    }

    fn volume(&self, dir: &DirHandle) -> io::Result<VolumeIdentity> {
        let t = self.tree.lock().unwrap();
        let mut id = root_stat(&t).identity();
        if let Some(n) = walk(&t.children, dir.rel())?.last() {
            id.dev_id = n.entry.dev.clone().unwrap_or_default();
            id.mount_root = n.entry.mount_root;
        }
        Ok(id)
    }
}

/// A file opened by [`FakeSource::open_file`].
#[derive(Debug, Clone)]
pub struct FakeFile {
    data: Option<Vec<u8>>,
    len: u64,
    pos: u64,
}

impl Read for FakeFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        let n = (buf.len() as u64).min(left) as usize;
        match &self.data {
            Some(d) => {
                let start = self.pos as usize;
                let avail = d.len().saturating_sub(start).min(n);
                buf[..avail].copy_from_slice(&d[start..start + avail]);
                buf[avail..n].fill(0);
            }
            None => buf[..n].fill(0),
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for FakeFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.pos)
    }
}
