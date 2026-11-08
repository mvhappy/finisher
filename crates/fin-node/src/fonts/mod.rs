//! The font manager (4C Decisions 32–37): one per server data folder, shared by every
//! engine opened on it.
//!
//! - The cache `<server-data>/fonts/` and the fetcher that fills it from the catalog
//!   (`fin-fonts`); the catalog is the built-in one (`FIN_FONT_CATALOG` in test-hooks builds).
//! - One shared measurer ([`provider::Measurer`]) over a provider that answers from the cache
//!   or from an open project's font asset file whose SHA-256 matches. Determinism comes from
//!   the request, which names exact bytes, so every project and branch shares it.
//! - Fetch triggers (Decision 34, owner answer 1: automatic, with an off switch): a project's
//!   open or import, a record that may change its pins (commit, history write, import), a
//!   measurement that found bytes missing, `fonts.plan`'s pins, and `fonts.fetch`. Background
//!   fetching is off while `fonts.autoFetch` is false or `FIN_FONTS_OFFLINE=1`
//!   ([`Manager::set_auto_fetch`] switches it at run time); `fonts.fetch` still fetches.
//! - Status: each engine's status gains `fonts {missing, fetching, failed}` and the warnings
//!   `FONT_MISSING`, `FONT_FETCH_FAILED`, `FONTS_DAMAGED` ([`status`]); a change is a
//!   `status` event on that engine (progress alone at most every [`PROGRESS_EVERY`]).
//!   Computing, comparing and publishing a summary is serialised (`publish`), so a summary
//!   computed earlier is never published after a newer one.
//! - Hashing (a cache file or font asset file not yet verified at its length and mtime) stays
//!   off the JS thread and the store's mutex: an open hashes on its worker before the engine
//!   attaches, and a rescan (which runs under the store) hands hashing, fetch requests and
//!   publishing to the `fin-node-fonts-jobs` thread ([`Job`]).
//!
//! Nothing here is written to a record, a diff or a snapshot, and nothing here blocks an edit.

mod fetch_log;
mod files;
pub(crate) mod napi;
mod provider;
mod publish;
pub(crate) mod scan;
mod status;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak, mpsc};
use std::time::{Duration, Instant};

use fin_core::CoreError;
use fin_core::layout::TextMeasurer;
use fin_fonts::{FetchOptions, Fetcher, FontCache, FontCatalog};
use fin_store::Store;
use serde_json::Value;

use crate::engine::{Inner, lock};
use provider::Provider;
use scan::{BranchFonts, Need};
pub(crate) use status::Summary;

/// The shortest gap between two status events that only report download progress.
pub(crate) const PROGRESS_EVERY: Duration = Duration::from_millis(250);

pub(crate) struct Manager {
    /// The catalog this server starts from: the built-in one (a test build's
    /// `FIN_FONT_CATALOG`). Plugin packs' catalogs are merged onto it.
    base: Arc<FontCatalog>,
    /// `base` merged with the enabled packs' catalogs ([`Manager::set_extra_catalogs`]).
    catalog: RwLock<Arc<FontCatalog>>,
    /// `None` when the cache or the catalog could not be used ([`Manager::damaged`]).
    fetcher: Option<Fetcher>,
    damaged: Option<CoreError>,
    /// `FIN_FONTS_OFFLINE=1`: background fetching stays off whatever the setting says.
    env_offline: bool,
    /// The `fonts.autoFetch` setting.
    setting: AtomicBool,
    provider: Arc<Provider>,
    measurer: Arc<provider::Measurer>,
    engines: Mutex<BTreeMap<u64, Attached>>,
    next: AtomicU64,
    /// Held while a summary is computed, compared with the published one and published.
    publish: Mutex<()>,
    /// The jobs thread's queue ([`Job`]).
    jobs: mpsc::Sender<Job>,
    /// `fonts.fetch` entries from the fetcher's state changes.
    fetches: fetch_log::FetchLog,
}

/// A pack whose catalogs [`Manager::set_extra_catalogs`] left out, and why.
pub(crate) struct Rejected {
    pub(crate) id: String,
    pub(crate) error: CoreError,
}

/// Work done on the `fin-node-fonts-jobs` thread, off the caller's locks.
enum Job {
    /// Hash the files `needs` name (so later summaries hit the memo) and queue the missing
    /// catalog ones.
    Need(Vec<Need>),
    /// Publish the engine's summary if it changed (`None`: every engine's).
    Refresh(Option<u64>),
}

struct Attached {
    project_id: String,
    inner: Weak<Inner>,
    branches: BTreeMap<String, BranchFonts>,
    /// The last summary published (as a status event or a status answer) and when.
    published: Option<(Summary, Instant)>,
}

impl Attached {
    fn all(&self) -> impl Iterator<Item = &BranchFonts> {
        self.branches.values()
    }

    fn needs(&self) -> BTreeSet<Need> {
        self.all().flat_map(|b| b.needs.iter().cloned()).collect()
    }

    /// What the branches' captions need with no pin, each key with its captions.
    fn unpinned(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (k, items) in self.all().flat_map(|b| b.unpinned.iter()) {
            out.entry(k.clone())
                .or_default()
                .extend(items.iter().cloned());
        }
        out
    }
}

static MANAGERS: Mutex<Vec<(PathBuf, Weak<Manager>)>> = Mutex::new(Vec::new());

/// The manager of `server_data`, made on first use.
#[allow(clippy::disallowed_methods)] // the server data directory
pub(crate) fn for_server(server_data: &Path) -> Arc<Manager> {
    let _ = std::fs::create_dir_all(server_data);
    let key = std::fs::canonicalize(server_data).unwrap_or_else(|_| server_data.to_path_buf());
    let mut all = lock(&MANAGERS);
    all.retain(|(_, w)| w.strong_count() > 0);
    if let Some(m) = all
        .iter()
        .find(|(p, _)| *p == key)
        .and_then(|(_, w)| w.upgrade())
    {
        return m;
    }
    let m = Manager::new(&key);
    all.push((key, Arc::downgrade(&m)));
    m
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

fn options(offline: bool) -> FetchOptions {
    #[allow(unused_mut)]
    let mut opts = FetchOptions {
        offline,
        // Honoured only in test-hooks builds of fin-fonts (the tests' loopback server).
        allow_loopback_http: cfg!(feature = "test-hooks"),
        ..FetchOptions::default()
    };
    #[cfg(feature = "test-hooks")]
    {
        let ms = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_millis)
        };
        if let Some(d) = ms("FIN_FONTS_TEST_BACKOFF_MS") {
            opts.backoff = vec![d; 3];
        }
        if let Some(d) = ms("FIN_FONTS_TEST_RETRY_MS") {
            opts.retry_after = d;
        }
        if env_flag("FIN_FONTS_TEST_NO_PROXY") {
            opts.proxy_from_env = false;
        }
    }
    opts
}

impl Manager {
    fn new(server_data: &Path) -> Arc<Manager> {
        let env_offline = env_flag("FIN_FONTS_OFFLINE");
        let (catalog, catalog_error) = match FontCatalog::active() {
            Ok(c) => (c, None),
            Err(e) => (FontCatalog::builtin().clone(), Some(e)),
        };
        let catalog = Arc::new(catalog);
        let (cache, damaged) = match FontCache::open(&server_data.join("fonts")) {
            Ok(c) => (Some(Arc::new(c)), catalog_error),
            Err(e) => (None, Some(e)),
        };
        // A damaged catalog fetches nothing (it would fetch another catalog's files).
        let fetcher = match (&cache, &damaged) {
            (Some(c), None) => Some(Fetcher::new(
                c.clone(),
                catalog.clone(),
                options(env_offline),
            )),
            _ => None,
        };
        let changes = fetcher.as_ref().map(Fetcher::subscribe);
        let provider = Arc::new(Provider::new(cache));
        let (jobs, queue) = mpsc::channel();
        let m = Arc::new_cyclic(|weak| Manager {
            base: catalog.clone(),
            catalog: RwLock::new(catalog),
            fetcher,
            damaged,
            env_offline,
            setting: AtomicBool::new(true),
            measurer: Arc::new(provider::Measurer::new(provider.clone(), weak.clone())),
            provider,
            engines: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
            publish: Mutex::new(()),
            jobs,
            fetches: fetch_log::FetchLog::new(server_data),
        });
        let weak = Arc::downgrade(&m);
        // Without the thread, `post` runs jobs inline (the queue's receiver is gone).
        let _ = std::thread::Builder::new()
            .name("fin-node-fonts-jobs".to_string())
            .spawn(move || {
                // Ends when the manager (which owns the sender) is gone.
                while let Ok(job) = queue.recv() {
                    let Some(m) = weak.upgrade() else { return };
                    m.run(job);
                }
            });
        if let Some(rx) = changes {
            let weak = Arc::downgrade(&m);
            let _ = std::thread::Builder::new()
                .name("fin-node-fonts".to_string())
                .spawn(move || {
                    // Ends when the fetcher (owned by the manager) is gone.
                    while let Ok((sha, state)) = rx.recv() {
                        let Some(m) = weak.upgrade() else { return };
                        m.fetches.changed(&m.catalog(), &sha, &state);
                        m.file_changed(&sha);
                    }
                });
        }
        m
    }

    /// The catalog in use now: the built-in one and the enabled packs' entries.
    pub(crate) fn catalog(&self) -> Arc<FontCatalog> {
        self.catalog
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Replaces the packs' catalogs (8A Task 12a): `packs` are the enabled plugin packs'
    /// font catalogs, each pack's together, in a fixed order. A pack merges whole or not at
    /// all: one whose entries clash with the built-in catalog or an earlier pack is left out
    /// and named in the answer (`catalogIdTaken`, a file listed twice), the others still
    /// merge. Pins are untouched: a pinned font keeps its cached file and stays usable
    /// whatever the catalog lists, and entries that left only stop being offered for new
    /// pins. The catalog the fetcher uses changes with it, and what open projects pin that
    /// the new catalog lists is queued for fetching again.
    pub(crate) fn set_extra_catalogs(
        &self,
        packs: Vec<(String, Vec<FontCatalog>)>,
    ) -> Vec<Rejected> {
        let mut merged = (*self.base).clone();
        let mut rejected = Vec::new();
        for (id, catalogs) in packs {
            match merged.merged(&catalogs) {
                Ok(next) => merged = next,
                Err(error) => rejected.push(Rejected { id, error }),
            }
        }
        let merged = Arc::new(merged);
        {
            let mut current = self
                .catalog
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if **current == *merged {
                return rejected;
            }
            *current = merged.clone();
        }
        if let Some(f) = &self.fetcher {
            f.set_catalog(merged);
        }
        let needs: Vec<Need> = lock(&self.engines)
            .values()
            .flat_map(Attached::needs)
            .collect();
        self.post(Job::Need(needs));
        self.post(Job::Refresh(None));
        rejected
    }

    /// The shared measurer: eval's, and every open store's (the edit path's overflow check).
    pub(crate) fn measurer(&self) -> Arc<dyn TextMeasurer> {
        self.measurer.clone()
    }

    /// Whether background fetching is on now.
    pub(crate) fn auto_fetch(&self) -> bool {
        self.fetcher.is_some() && !self.env_offline && self.setting.load(Ordering::SeqCst)
    }

    /// `fonts.autoFetch`. Turned on, the open projects' missing pins are queued (on the jobs
    /// thread, like the status events this publishes).
    pub(crate) fn set_auto_fetch(&self, on: bool) {
        self.setting.store(on, Ordering::SeqCst);
        if let Some(f) = &self.fetcher {
            f.set_offline(!self.auto_fetch());
        }
        if self.auto_fetch() {
            let needs: Vec<Need> = lock(&self.engines)
                .values()
                .flat_map(Attached::needs)
                .collect();
            self.post(Job::Need(needs));
        }
        self.post(Job::Refresh(None));
    }

    /// Queues `job` for the jobs thread (inline when there is none).
    fn post(&self, job: Job) {
        if let Err(mpsc::SendError(job)) = self.jobs.send(job) {
            self.run(job);
        }
    }

    fn run(&self, job: Job) {
        match job {
            Job::Need(needs) => self.warm(&needs),
            Job::Refresh(Some(key)) => self.refresh(key),
            Job::Refresh(None) => self.refresh_all(),
        }
    }

    /// Hashes what a summary of `needs` looks at (cache files through the fetcher's state,
    /// asset files through the provider; each memoised by length and mtime), then queues
    /// the missing catalog files.
    fn warm(&self, needs: &[Need]) {
        for n in needs {
            match &n.asset {
                Some(path) => drop(self.provider.check_asset(path, &n.sha256)),
                None => drop(self.file_state(&n.sha256)),
            }
        }
        self.request_needs(needs.iter());
    }

    /// Queues background fetches (a no-op for cached, unknown or fetching files, and while
    /// background fetching is off).
    pub(crate) fn request<'a>(&self, shas: impl Iterator<Item = &'a str>) {
        if let Some(f) = &self.fetcher {
            for sha in shas {
                f.request(sha);
            }
        }
    }

    fn request_needs<'a>(&self, needs: impl Iterator<Item = &'a Need>) {
        self.request(
            needs
                .filter(|n| n.asset.is_none())
                .map(|n| n.sha256.as_str()),
        );
    }

    /// Registers an engine (at open, on the JS thread: [`Opened::read`] already hashed and
    /// queued what its pins lack, on the worker).
    pub(crate) fn attach(
        self: &Arc<Self>,
        project_id: String,
        inner: Weak<Inner>,
        branches: BTreeMap<String, BranchFonts>,
    ) -> Handle {
        let key = self.next.fetch_add(1, Ordering::SeqCst);
        lock(&self.engines).insert(
            key,
            Attached {
                project_id,
                inner,
                branches,
                published: None,
            },
        );
        self.update_assets();
        Handle {
            manager: self.clone(),
            key,
        }
    }

    fn detach(&self, key: u64) {
        if lock(&self.engines).remove(&key).is_some() {
            self.update_assets();
        }
    }

    /// The provider's asset files: every open project's font assets.
    fn update_assets(&self) {
        let mut assets: HashMap<String, BTreeSet<PathBuf>> = HashMap::new();
        for a in lock(&self.engines).values() {
            for (sha, path) in a.all().flat_map(|b| b.assets.iter()) {
                assets.entry(sha.clone()).or_default().insert(path.clone());
            }
        }
        self.provider.set_assets(assets);
    }

    /// Reads the pins again: of `branch`, or of every branch (under the store). Hashing the
    /// new needs, queueing their fetches and publishing go to the jobs thread.
    fn rescan(&self, key: u64, store: &Store, branch: Option<&str>) {
        let fresh = match branch {
            Some(b) => match scan::branch(store, b) {
                Some(f) => BTreeMap::from([(b.to_string(), f)]),
                None => return,
            },
            None => scan::store(store),
        };
        let added: Vec<Need> = {
            let mut engines = lock(&self.engines);
            let Some(a) = engines.get_mut(&key) else {
                return;
            };
            let before = a.needs();
            let changed = match branch {
                Some(b) => a.branches.get(b) != fresh.get(b),
                None => a.branches != fresh,
            };
            if !changed {
                return;
            }
            match branch {
                Some(_) => a.branches.extend(fresh),
                None => a.branches = fresh,
            }
            a.needs().difference(&before).cloned().collect()
        };
        self.update_assets();
        self.post(Job::Need(added));
        self.post(Job::Refresh(Some(key)));
    }

    /// What the open engines of `project_id` pin.
    fn pinned_by(&self, project_id: &str) -> Option<BTreeSet<Need>> {
        let engines = lock(&self.engines);
        let mut found = false;
        let mut out = BTreeSet::new();
        for a in engines.values().filter(|a| a.project_id == project_id) {
            found = true;
            out.extend(a.needs());
        }
        found.then_some(out)
    }
}

/// What `Engine.open` read for the manager before the engine exists: its server's manager
/// and what each branch pins.
pub(crate) struct Opened {
    pub(crate) manager: Arc<Manager>,
    pub(crate) branches: BTreeMap<String, BranchFonts>,
}

impl Opened {
    /// On the open's worker, off the JS thread: what every unarchived branch pins, its files
    /// hashed and the missing ones queued (Decision 34's open trigger), so attaching hashes
    /// nothing.
    pub(crate) fn read(manager: Arc<Manager>, store: &Store) -> Opened {
        let branches = scan::store(store);
        let needs: Vec<Need> = branches
            .values()
            .flat_map(|b| b.needs.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        manager.warm(&needs);
        Opened { manager, branches }
    }
}

/// An engine's place in its server's manager; dropping it detaches the engine.
pub(crate) struct Handle {
    manager: Arc<Manager>,
    key: u64,
}

impl Handle {
    pub(crate) fn manager(&self) -> &Arc<Manager> {
        &self.manager
    }

    /// After a record landed on `branch`: reads its pins again when it may have changed them.
    pub(crate) fn committed(&self, store: &Store, c: &fin_core::CommittedTx) {
        if scan::touches(c) {
            self.manager.rescan(self.key, store, Some(&c.tx.branch));
        }
    }

    /// After a write that may touch any branch (history, rebuild, rewind): every branch.
    pub(crate) fn rescan_all(&self, store: &Store) {
        self.manager.rescan(self.key, store, None);
    }

    /// The store's status JSON with the font summary merged in.
    pub(crate) fn merged(&self, raw: Value) -> Value {
        self.manager.merged(self.key, raw)
    }

    /// Emits the store's status event (see [`Manager::emit_status`]; the caller holds the
    /// store).
    pub(crate) fn emit_status(&self, raw: Value, events: &crate::maintenance::EventSink) {
        self.manager.emit_status(self.key, raw, events);
    }

    pub(crate) fn detach(&self) {
        self.manager.detach(self.key);
    }

    /// Test builds: a refresh that pauses after computing its summary, while `during` runs
    /// (a commit changing the pins, whose refresh must publish after it).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn publish_race(&self, during: impl FnOnce()) {
        self.manager.publish_race(self.key, during);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.manager.detach(self.key);
    }
}
