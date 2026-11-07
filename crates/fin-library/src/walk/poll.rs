//! What each walk mode lists (Decisions 25, 27, 28): the known directories of the catalog
//! ([`KnownDirs`]) and the per-mode choice for every directory the walk reaches.
//!
//! - **Full** lists every directory; with `known`, a directory already listed completely in
//!   this generation (a resumed job) is **skipped but descended** through its known
//!   subdirectories.
//! - **Dirs** lists exactly the given directories; a subdirectory one of them reports that the
//!   catalog does not know yet (a tree moved in) is listed too, recursively, when `known` is
//!   given.
//! - **Poll** re-stats every known directory from its parent's handle and re-lists those whose
//!   mtime changed; a known directory with an unchanged mtime is not opened when it has no
//!   known subdirectory (a leaf costs one stat), and opened only to stat its known children
//!   otherwise. A new directory is listed, recursively. The root's own mtime is read from its
//!   handle.
//!
//! Every mode applies, with `known`: a directory that was a nested mount root and no longer is
//! gets `ROOT_UNMOUNTED` and is not descended; a listing that drops from N > 0 entries to 0
//! is `DIR_EMPTY_SUSPECT` (incomplete, so it marks nothing).

use std::collections::{BTreeMap, HashSet};
use std::io::{self, BufRead};

use crate::exclude::Excludes;
use crate::names;
use crate::raw::RawRel;
use crate::types::KnownDir;

/// The catalog's directories, by raw chain, with each one's known subdirectories.
#[derive(Debug, Clone, Default)]
pub struct KnownDirs {
    dirs: BTreeMap<RawRel, KnownDir>,
    children: BTreeMap<RawRel, Vec<Vec<u8>>>,
}

impl KnownDirs {
    pub fn new(dirs: impl IntoIterator<Item = KnownDir>) -> Self {
        let mut k = Self::default();
        for d in dirs {
            k.insert(d);
        }
        k
    }

    /// Reads `in/dirs.ndjson` (one [`KnownDir`] per line; blank lines ignored).
    pub fn from_ndjson(r: impl BufRead) -> io::Result<Self> {
        let mut k = Self::default();
        for line in r.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let d: KnownDir = serde_json::from_str(&line)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            d.dir
                .validate()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            k.insert(d);
        }
        Ok(k)
    }

    pub fn insert(&mut self, d: KnownDir) {
        if let (Some(parent), Some(name)) = (d.dir.parent(), d.dir.name()) {
            let names = self.children.entry(parent).or_default();
            if let Err(at) = names.binary_search_by(|n| n.as_slice().cmp(name)) {
                names.insert(at, name.to_vec());
            }
        }
        self.dirs.insert(d.dir.clone(), d);
    }

    pub fn get(&self, dir: &RawRel) -> Option<&KnownDir> {
        self.dirs.get(dir)
    }

    /// The known subdirectories' names of `dir`, sorted by raw name.
    pub fn children(&self, dir: &RawRel) -> &[Vec<u8>] {
        self.children.get(dir).map_or(&[], Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
}

/// What the walker does with one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Work {
    /// Resumed full scan: already listed in this generation; descend through known children.
    Skip,
    /// List it. `mtime` is its mtime as its parent's listing or stat saw it (read from its
    /// handle before listing when `None`).
    List { mtime: Option<i64> },
    /// Poll, mtime unchanged: stat its known subdirectories; list it after all when one is
    /// gone or no longer a directory.
    Check {
        mtime: Option<i64>,
        children: Vec<Vec<u8>>,
    },
    /// Poll of the root: read its mtime from its handle, then `Check` or `List`.
    Probe {
        known_mtime: Option<i64>,
        children: Vec<Vec<u8>>,
    },
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Full { generation: u64 },
    Dirs,
    Poll,
}

/// The mode's decisions over the known directories.
#[derive(Debug)]
pub(crate) struct Plan {
    kind: Kind,
    known: KnownDirs,
    has_known: bool,
    /// Dirs mode: what is queued already (targets and new subdirectories).
    queued: HashSet<RawRel>,
}

impl Plan {
    /// The plan and the directories the walk starts from, in walk order.
    pub(crate) fn new(mode: super::WalkMode, excludes: &Excludes) -> (Plan, Vec<(RawRel, Work)>) {
        use super::WalkMode;
        let (kind, known, targets) = match mode {
            WalkMode::Full { generation, known } => (Kind::Full { generation }, known, Vec::new()),
            WalkMode::Dirs { dirs, known } => (Kind::Dirs, known, dirs),
            WalkMode::Poll { known } => (Kind::Poll, Some(known), Vec::new()),
        };
        let mut plan = Plan {
            kind,
            has_known: known.is_some(),
            known: known.unwrap_or_default(),
            queued: HashSet::new(),
        };
        let root = RawRel::root();
        let seeds = match kind {
            Kind::Full { .. } => vec![(root.clone(), plan.full(&root, None))],
            Kind::Poll => vec![match plan.known.get(&root) {
                Some(k) => (
                    root.clone(),
                    Work::Probe {
                        known_mtime: k.mtime_ns,
                        children: plan.known.children(&root).to_vec(),
                    },
                ),
                None => (root, Work::List { mtime: None }),
            }],
            Kind::Dirs => plan.targets(targets, excludes),
        };
        (plan, seeds)
    }

    /// The seeds of a Dirs walk: the targets by depth then raw chain, deduplicated, without
    /// those a component of which the default rules or the library's globs exclude.
    fn targets(&mut self, dirs: Vec<RawRel>, excludes: &Excludes) -> Vec<(RawRel, Work)> {
        let mut dirs: Vec<RawRel> = dirs
            .into_iter()
            .filter(|d| d.validate().is_ok() && !excluded(d, excludes))
            .collect();
        dirs.sort_by(|a, b| a.depth().cmp(&b.depth()).then_with(|| a.cmp(b)));
        dirs.dedup();
        self.queued.extend(dirs.iter().cloned());
        dirs.into_iter()
            .map(|d| (d, Work::List { mtime: None }))
            .collect()
    }

    /// Whether `dir` was a nested mount root at its last listing.
    pub(crate) fn was_mount(&self, dir: &RawRel) -> bool {
        self.known.get(dir).is_some_and(|k| k.mount_root)
    }

    /// The entry count of `dir`'s last listing (0 when unknown).
    pub(crate) fn known_entries(&self, dir: &RawRel) -> u32 {
        self.known.get(dir).map_or(0, |k| k.entries)
    }

    fn full(&self, dir: &RawRel, mtime: Option<i64>) -> Work {
        match (self.kind, self.known.get(dir)) {
            (Kind::Full { generation }, Some(k)) if k.listed_gen == Some(generation) => Work::Skip,
            _ => Work::List { mtime },
        }
    }

    /// A subdirectory a listing or a stat found, with its mtime now; `None`: not walked.
    pub(crate) fn child(&mut self, dir: RawRel, mtime: i64) -> Option<(RawRel, Work)> {
        let work = match self.kind {
            Kind::Full { .. } => self.full(&dir, Some(mtime)),
            Kind::Dirs => {
                if !self.has_known
                    || self.known.get(&dir).is_some()
                    || !self.queued.insert(dir.clone())
                {
                    return None;
                }
                Work::List { mtime: Some(mtime) }
            }
            Kind::Poll => match self.known.get(&dir) {
                Some(k) if k.mtime_ns == Some(mtime) => Work::Check {
                    mtime: Some(mtime),
                    children: self.known.children(&dir).to_vec(),
                },
                _ => Work::List { mtime: Some(mtime) },
            },
        };
        Some((dir, work))
    }

    /// The known subdirectories of a skipped directory (resumed full scan).
    pub(crate) fn after_skip(&self, dir: &RawRel) -> Vec<(RawRel, Work)> {
        self.known
            .children(dir)
            .iter()
            .map(|n| {
                let c = dir.join(n);
                let w = self.full(&c, None);
                (c, w)
            })
            .collect()
    }
}

/// Whether any component of `dir` is excluded by name or glob (the listing rules that need
/// siblings, such as project folders, are left to the listing of its parent).
fn excluded(dir: &RawRel, excludes: &Excludes) -> bool {
    let cs = excludes.case_sensitive();
    let mut prefix = RawRel::root();
    for c in dir.components() {
        let key = names::dir_key(&prefix, cs);
        let n = names::nfc(c);
        let rel_key = names::rel_key(&key, &n.nfc, cs);
        if excludes.check(&rel_key, &n.nfc, true).is_some() {
            return true;
        }
        prefix = prefix.join(c);
    }
    false
}
