//! The media cache (sub-project 6 spec §7.2; 6B-1 plan Task 13): `<server-data>/cache/media/`,
//! its index ([`super::index`]), commits ([`super::commit`]), the adoption at open
//! ([`super::adopt`]) and eviction ([`super::evict`]). Napi-free: `fin-node`'s cache thread
//! owns one [`Cache`] and runs every call on it in order (`mod.rs`), and the tests drive it
//! directly. Workers never touch the cache.
//!
//! Pins: holders are `job:<id>` (an entry a running job reads, released by the kind's end hook)
//! and `project:<id>` (entries serving a project open on this server: [`Cache::pin_project`]
//! when it opens, [`Cache::unpin_project`] when it closes; an entry committed for an open
//! project is pinned for it at once). No pin outlives a server run: the open clears them, and
//! the server pins again what it opens ("rebuilt at start for open projects").

// The cache's own directories under server data, never a library path.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fin_core::CoreError;
use fin_media::cache::{CacheGcResult, CacheKey, CacheStatus, EntryMeta};
use serde_json::json;

use super::adopt::{Adopted, adopt};
use super::evict::{default_cap, entry_dir, evict_to, volume_space};
use super::index::{Index, cache_io};

/// The clock (unix ms); tests pass their own.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Test faults (`cfg(test)` or fin-node's `test-hooks` feature only).
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Default)]
pub struct Faults {
    /// A commit stops after its rename, before the row is inserted (as a crash there would).
    pub stop_after_rename: bool,
    /// What a commit's free-space check before the move reads instead of the volume. The
    /// check after a failed move always reads the volume.
    pub free_bytes: Option<u64>,
}

pub struct Cache {
    pub(super) root: PathBuf,
    pub(super) index: Index,
    pub(super) cap: u64,
    /// The cap when no setting names one (`min(50 GiB, 15 % of the volume)`).
    default_cap: u64,
    open: Vec<String>,
    clock: Clock,
    /// A counter for unique staging and trash names within a run.
    pub(super) seq: u64,
    #[cfg(any(test, feature = "test-hooks"))]
    pub faults: Faults,
}

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn bad(field: &str, message: &str) -> CoreError {
    CoreError::new("INVALID_OP", message.to_string())
        .with_details(json!({ "field": field, "reason": "mediaCache" }))
}

/// Project ids as given, each `[A-Za-z0-9_-]{1,64}`, deduplicated.
pub fn check_projects(projects: &[String]) -> Result<Vec<String>, CoreError> {
    let mut out: Vec<String> = Vec::with_capacity(projects.len());
    for p in projects {
        if !valid_id(p) {
            return Err(bad("projects", "a project id is [A-Za-z0-9_-]{1,64}"));
        }
        if !out.contains(p) {
            out.push(p.clone());
        }
    }
    Ok(out)
}

/// A pin holder: `job:<id>`, `project:<id>` or `http:<id>` (an entry file being served, 6B-1 Task 17).
pub fn check_holder(holder: &str) -> Result<(), CoreError> {
    let ok = ["job:", "project:", "http:"]
        .iter()
        .find_map(|p| holder.strip_prefix(p))
        .is_some_and(valid_id);
    if ok {
        Ok(())
    } else {
        Err(bad(
            "holder",
            "a pin holder is job:<id>, project:<id> or http:<id>",
        ))
    }
}

impl Cache {
    /// The cache root of `server_data`.
    pub fn root_of(server_data: &Path) -> PathBuf {
        server_data.join("cache").join("media")
    }

    /// Opens (creating) the cache of `server_data` with `cap` bytes (`None`: the default), and
    /// adopts what a crash left (`adopt.rs`).
    pub fn open(
        server_data: &Path,
        cap: Option<u64>,
        clock: Option<Clock>,
    ) -> Result<(Cache, Adopted), CoreError> {
        let root = Self::root_of(server_data);
        std::fs::create_dir_all(&root)
            .map_err(|e| cache_io("io", format!("cannot make {}: {e}", root.display())))?;
        // Resolved once, as job directories are: every open below it refuses a link.
        let root = std::fs::canonicalize(&root)
            .map_err(|e| cache_io("io", format!("cannot resolve {}: {e}", root.display())))?;
        let (mut index, replaced) = Index::open(&root.join("index.db"))?;
        index.clear_pins()?;
        let clock = clock.unwrap_or_else(|| Arc::new(system_now));
        let mut adopted = adopt(&root, &mut index, clock())?;
        adopted.index_replaced = replaced;
        let default_cap = default_cap(volume_space(&root).ok().map(|(total, _)| total));
        let cache = Cache {
            root,
            index,
            cap: cap.unwrap_or(default_cap),
            default_cap,
            open: Vec::new(),
            clock,
            seq: 0,
            #[cfg(any(test, feature = "test-hooks"))]
            faults: Faults::default(),
        };
        Ok((cache, adopted))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn now(&self) -> i64 {
        (self.clock)()
    }

    pub(super) fn open_projects(&self) -> Vec<String> {
        self.open.clone()
    }

    /// The free bytes a commit checks before it moves anything (a test fault may replace it).
    pub(super) fn free_bytes_precheck(&self) -> Result<u64, CoreError> {
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(free) = self.faults.free_bytes {
            return Ok(free);
        }
        volume_space(&self.root).map(|(_, free)| free).map_err(|e| {
            cache_io(
                "io",
                format!("cannot read the cache volume's free space: {e}"),
            )
        })
    }

    /// The entry directory of `key` (whether or not it exists).
    pub fn entry_dir(&self, key: &CacheKey) -> PathBuf {
        entry_dir(&self.root, key)
    }

    /// The entry `key`, its last use set to now; `None` when there is none, or when its
    /// directory has gone (the row is dropped then).
    pub fn lookup(&mut self, key: &str) -> Result<Option<EntryMeta>, CoreError> {
        let Ok(key) = CacheKey::parse(key) else {
            return Ok(None);
        };
        let Some(meta) = self.index.get(key.as_str())? else {
            return Ok(None);
        };
        let present = std::fs::symlink_metadata(self.entry_dir(&key)).is_ok_and(|m| m.is_dir());
        if !present {
            self.index.delete(key.as_str())?;
            return Ok(None);
        }
        self.index.touch(key.as_str(), self.now())?;
        Ok(Some(meta))
    }

    /// Pins `key` for `holder`; false when there is no such entry.
    pub fn pin(&mut self, key: &str, holder: &str) -> Result<bool, CoreError> {
        check_holder(holder)?;
        match CacheKey::parse(key) {
            Ok(k) => self.index.pin(k.as_str(), holder),
            Err(_) => Ok(false),
        }
    }

    /// Records that the entry `key` serves `projects` (a consumer's hit: only a commit records
    /// them otherwise) and pins it for those of them open here; false when there is no entry.
    pub fn serve(&mut self, key: &str, projects: &[String]) -> Result<bool, CoreError> {
        let projects = check_projects(projects)?;
        let Ok(key) = CacheKey::parse(key) else {
            return Ok(false);
        };
        if !self.index.contains(key.as_str())? {
            return Ok(false);
        }
        let open = self.open_projects();
        self.index.add_projects(key.as_str(), &projects, &open)?;
        Ok(true)
    }

    /// Removes `holder`'s pin of `key`; whether there was one.
    pub fn unpin(&mut self, key: &str, holder: &str) -> Result<bool, CoreError> {
        check_holder(holder)?;
        Ok(self.index.unpin(Some(key), holder)? > 0)
    }

    /// Removes every pin `holder` has (a job's end); how many.
    pub fn release(&mut self, holder: &str) -> Result<u64, CoreError> {
        check_holder(holder)?;
        self.index.unpin(None, holder)
    }

    /// `project` is open: every entry serving it is pinned for `project:<id>` until
    /// [`Cache::unpin_project`], and so is each entry committed for it meanwhile.
    pub fn pin_project(&mut self, project: &str) -> Result<u64, CoreError> {
        let project = check_projects(&[project.to_string()])?.remove(0);
        if !self.open.contains(&project) {
            self.open.push(project.clone());
        }
        self.index.pin_project(&project)
    }

    pub fn unpin_project(&mut self, project: &str) -> Result<u64, CoreError> {
        let project = check_projects(&[project.to_string()])?.remove(0);
        self.open.retain(|p| p != &project);
        self.index.unpin(None, &format!("project:{project}"))
    }

    /// The holders of `key`'s pins.
    pub fn pins_of(&self, key: &str) -> Result<Vec<String>, CoreError> {
        self.index.pinned(key)
    }

    /// Entries made from the source with `fingerprint`.
    pub fn entries_for(&self, fingerprint: &str) -> Result<Vec<EntryMeta>, CoreError> {
        self.index.entries_for(fingerprint)
    }

    /// The projects `key` serves (reach for `media.url`, spec §7.3).
    pub fn projects_of(&self, key: &str) -> Result<Vec<String>, CoreError> {
        self.index.projects_of(key)
    }

    /// Sets the cap (`None`: the default) and evicts down to it.
    pub fn set_cap(&mut self, cap: Option<u64>) -> Result<CacheGcResult, CoreError> {
        self.cap = cap.unwrap_or(self.default_cap);
        self.evict(self.cap, None)
    }

    pub fn status(&self) -> Result<CacheStatus, CoreError> {
        let (entries, pinned, by_recipe) = self.index.usage()?;
        Ok(CacheStatus {
            bytes: self.index.total_bytes()?,
            entries,
            cap_bytes: self.cap,
            pinned,
            by_recipe,
        })
    }

    /// Evicts down to `target` bytes (at most the cap; the cap without one).
    pub fn gc(&mut self, target: Option<u64>) -> Result<CacheGcResult, CoreError> {
        let target = target.map_or(self.cap, |t| t.min(self.cap));
        self.evict(target, None)
    }

    pub(super) fn evict(
        &mut self,
        target: u64,
        keep: Option<&str>,
    ) -> Result<CacheGcResult, CoreError> {
        evict_to(&self.root, &self.index, target, keep, &mut self.seq)
    }
}
