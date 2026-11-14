//! The test overlay (feature `test-hooks` only; plan 5D Task 4): a `.fin-fake.json` at a
//! **real** root makes the native source report some of its real paths as placeholders,
//! automounts or nested mounts, delays the first stat, listings and entries, injects errors
//! and unmounts, and appends every [`crate::open_for_read`] of a listed path to a log file
//! outside the root. Worker tests (plan Task 6) run the real `fin-workers` binary over a
//! temporary tree with it. Release builds never read it: the module does not exist without
//! the feature, and `scripts/check-test-hooks.mjs` fails a shipped build that enables it.
//!
//! ```json
//! { "openLog": "/tmp/…/opens.ndjson",
//!   "statDelayMs": 0, "statError": null,
//!   "paths": {
//!     "cloud":   { "placeholder": "dataless" },
//!     "a/b.mov": { "placeholder": "recallOnData" },
//!     "net":     { "automount": true },
//!     "disk":    { "mountRoot": true },
//!     "gone":    { "unmounted": true },
//!     "slow":    { "readDelayMs": 50, "entryDelayMs": 5 },
//!     "bad":     { "error": "io", "openError": null },
//!     "poison":  { "crashOnOpen": true } } }
//! ```
//!
//! Paths are `/`-joined display names relative to the root (`""` is the root). A placeholder
//! (`dataless`, `recallOnOpen`, `recallOnData`, `offline`, `unpinned`) sets the matching macOS
//! or Windows attribute bit on the entry, whatever the host, and covers everything below it;
//! `crashOnOpen` aborts the process at an [`crate::open_for_read`] of the path;
//! `unmounted` reports the directory as no mount root, listed empty (the bare mount point);
//! `error`/`openError` fail its listing or its open (error names as in the fake source).
//! Log lines are `{"op": "open" | "openDir" | "readDir", "path", "hydrate"?, "refused"?}`:
//! `open` for every `open_for_read` of a listed path (a placeholder is refused `CloudOnly`
//! unless `hydrate`), `openDir`/`readDir` for every open or listing of a placeholder or
//! automount directory. A correct walk and probe pass leave no line about a placeholder.

// The overlay reads its own file by path and appends to its log: test builds only, never a
// library file (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::placeholder::{SF_DATALESS, win};
use crate::raw::RawRel;
use crate::source::fake::spec::{check_error, error};
use crate::source::{Attrs, DirHandle, Listing, RawEntry, RootStat, Source};
use crate::types::{OpenError, Placeholder, VolumeIdentity};

/// The overlay's file name at a root (a dot-file: every scan excludes it).
pub const FILE: &str = ".fin-fake.json";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PathSpec {
    #[serde(default)]
    placeholder: Option<Placeholder>,
    #[serde(default)]
    read_delay_ms: u64,
    #[serde(default)]
    entry_delay_ms: u64,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    open_error: Option<String>,
    #[serde(default)]
    unmounted: bool,
    #[serde(default)]
    mount_root: bool,
    #[serde(default)]
    automount: bool,
    /// Aborts the process when the path is opened for reading (worker isolation tests).
    #[serde(default)]
    crash_on_open: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Spec {
    #[serde(default)]
    open_log: Option<PathBuf>,
    #[serde(default)]
    stat_delay_ms: u64,
    #[serde(default)]
    stat_error: Option<String>,
    #[serde(default)]
    paths: BTreeMap<String, PathSpec>,
}

/// A root's overlay.
#[derive(Debug, Clone)]
pub struct Overlay {
    spec: Spec,
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn key(rel: &RawRel) -> String {
    rel.display()
}

impl Overlay {
    /// The overlay at `root`, if it has one.
    pub fn load(root: &Path) -> io::Result<Option<Overlay>> {
        // test-hooks only: the overlay's own file, by path.
        let text = match std::fs::read_to_string(root.join(FILE)) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let spec: Spec =
            serde_json::from_str(&text).map_err(|e| invalid(format!("{FILE}: {e}")))?;
        let errors = spec
            .paths
            .values()
            .flat_map(|p| [&p.error, &p.open_error])
            .chain([&spec.stat_error]);
        for e in errors {
            check_error(e).map_err(|e| invalid(e.to_string()))?;
        }
        if let Some(log) = &spec.open_log
            && (!log.is_absolute() || log.starts_with(root))
        {
            return Err(invalid(format!(
                "{FILE}: openLog must be absolute and outside the root"
            )));
        }
        Ok(Some(Overlay { spec }))
    }

    fn get(&self, rel: &RawRel) -> Option<&PathSpec> {
        self.spec.paths.get(&key(rel))
    }

    /// The placeholder `rel` or one of its directories is marked as.
    fn placeholder(&self, rel: &RawRel) -> Option<Placeholder> {
        let mut cur = Some(rel.clone());
        while let Some(r) = cur {
            if let Some(p) = self.get(&r).and_then(|p| p.placeholder) {
                return Some(p);
            }
            cur = r.parent();
        }
        None
    }

    fn flagged(&self, rel: &RawRel) -> bool {
        self.placeholder(rel).is_some() || self.get(rel).is_some_and(|p| p.automount)
    }

    fn log(&self, op: &str, rel: &RawRel, hydrate: Option<bool>, refused: Option<bool>) {
        let Some(path) = &self.spec.open_log else {
            return;
        };
        let mut line = serde_json::json!({ "op": op, "path": key(rel) });
        if let Some(h) = hydrate {
            line["hydrate"] = h.into();
        }
        if let Some(r) = refused {
            line["refused"] = r.into();
        }
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = res {
            panic!("overlay open log: {e}");
        }
    }

    /// `e` (at `rel`) as the overlay reports it.
    fn apply(&self, rel: &RawRel, mut e: RawEntry) -> RawEntry {
        let Some(p) = self.get(rel) else {
            return e;
        };
        if let Some(ph) = p.placeholder {
            e.attrs = attrs(ph);
        }
        e.automount |= p.automount;
        e.mount_root = (e.mount_root || p.mount_root) && !p.unmounted;
        e
    }
}

fn attrs(p: Placeholder) -> Attrs {
    let windows = |attributes| Attrs::Windows { attributes };
    match p {
        Placeholder::RecallOnOpen => windows(win::FILE_ATTRIBUTE_RECALL_ON_OPEN),
        Placeholder::RecallOnData => windows(win::FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS),
        Placeholder::Offline => windows(win::FILE_ATTRIBUTE_OFFLINE),
        Placeholder::Unpinned => windows(win::FILE_ATTRIBUTE_UNPINNED),
        // Stubs are names, not attributes (a real stub file needs no overlay): dataless.
        _ => Attrs::Macos { flags: SF_DATALESS },
    }
}

/// The hook of [`crate::open_for_read`]: logs an open of a listed path and refuses a marked
/// placeholder unless `hydrate`. `None`: go on with the real open.
pub(crate) fn on_open(root: &Path, rel: &RawRel, hydrate: bool) -> Option<OpenError> {
    let ov = match Overlay::load(root) {
        Ok(Some(o)) => o,
        Ok(None) => return None,
        Err(e) => return Some(OpenError::Io(e)),
    };
    if ov.get(rel).is_some_and(|p| p.crash_on_open) {
        std::process::abort();
    }
    let placeholder = ov.placeholder(rel).is_some();
    let refuse = placeholder && !hydrate;
    if placeholder || ov.get(rel).is_some() {
        ov.log("open", rel, Some(hydrate), Some(refuse));
    }
    refuse.then_some(OpenError::CloudOnly)
}

/// A source with the overlay of its root applied.
pub struct OverlaySource {
    inner: Arc<dyn Source>,
    ov: Overlay,
}

impl fmt::Debug for OverlaySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlaySource")
            .field("ov", &self.ov)
            .finish_non_exhaustive()
    }
}

/// `inner` (the native source of `root`) with `root`'s overlay, or `inner` itself when the
/// root has none.
pub fn wrap(root: &Path, inner: Arc<dyn Source>) -> io::Result<Arc<dyn Source>> {
    Ok(match Overlay::load(root)? {
        Some(ov) => Arc::new(OverlaySource { inner, ov }),
        None => inner,
    })
}

fn sleep_ms(ms: u64) {
    if ms > 0 {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

impl OverlaySource {
    fn opening(&self, rel: &RawRel) -> io::Result<()> {
        if self.ov.flagged(rel) {
            self.ov.log("openDir", rel, None, None);
        }
        match self.ov.get(rel).and_then(|p| p.open_error.as_deref()) {
            Some(e) => Err(error(e)),
            None => Ok(()),
        }
    }
}

impl Source for OverlaySource {
    fn stat_root(&self) -> io::Result<RootStat> {
        sleep_ms(self.ov.spec.stat_delay_ms);
        if let Some(e) = &self.ov.spec.stat_error {
            return Err(error(e));
        }
        self.inner.stat_root()
    }

    fn open_dir(&self, rel: &RawRel) -> io::Result<DirHandle> {
        self.opening(rel)?;
        self.inner.open_dir(rel)
    }

    fn open_child(&self, parent: &DirHandle, name: &[u8]) -> io::Result<DirHandle> {
        self.opening(&parent.rel().join(name))?;
        self.inner.open_child(parent, name)
    }

    fn read_dir(&self, dir: &DirHandle) -> io::Result<Vec<RawEntry>> {
        self.read_dir_with(dir, &|| {})?.strict()
    }

    fn read_dir_with(&self, dir: &DirHandle, progress: &dyn Fn()) -> io::Result<Listing> {
        let rel = dir.rel();
        if self.ov.flagged(rel) {
            self.ov.log("readDir", rel, None, None);
        }
        let p = self.ov.get(rel).cloned().unwrap_or_default();
        sleep_ms(p.read_delay_ms);
        if let Some(e) = &p.error {
            return Err(error(e));
        }
        if p.unmounted {
            return Ok(Listing::default());
        }
        let mut raw = self.inner.read_dir_with(dir, progress)?;
        raw.entries = std::mem::take(&mut raw.entries)
            .into_iter()
            .map(|e| {
                sleep_ms(p.entry_delay_ms);
                progress();
                self.ov.apply(&rel.join(&e.name), e)
            })
            .collect();
        Ok(raw)
    }

    fn stat_child(&self, parent: &DirHandle, name: &[u8]) -> io::Result<Option<RawEntry>> {
        let child = parent.rel().join(name);
        Ok(self
            .inner
            .stat_child(parent, name)?
            .map(|e| self.ov.apply(&child, e)))
    }

    fn dir_mtime(&self, dir: &DirHandle) -> io::Result<Option<i64>> {
        self.inner.dir_mtime(dir)
    }

    /// The native identity, with the path's `mountRoot`/`unmounted` mark applied as
    /// [`Overlay::apply`] applies it to the entry: the walker checks a known mount point by both.
    fn volume(&self, dir: &DirHandle) -> io::Result<VolumeIdentity> {
        let mut v = self.inner.volume(dir)?;
        if let Some(p) = self.ov.get(dir.rel()) {
            v.mount_root = (v.mount_root || p.mount_root) && !p.unmounted;
        }
        Ok(v)
    }
}
