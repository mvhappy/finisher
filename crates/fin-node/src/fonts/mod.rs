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
//!
//! Nothing here is written to a record, a diff or a snapshot, and nothing here blocks an edit.

mod files;
pub(crate) mod napi;
mod provider;
pub(crate) mod scan;
mod status;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use fin_core::CoreError;
use fin_core::layout::TextMeasurer;
use fin_fonts::{FetchOptions, Fetcher, FontCache, FontCatalog};
use fin_store::{EngineEvent, Store};
use serde_json::Value;

use crate::engine::{Inner, lock};
use provider::Provider;
use scan::{BranchFonts, Need};
pub(crate) use status::Summary;

/// The shortest gap between two status events that only report download progress.
pub(crate) const PROGRESS_EVERY: Duration = Duration::from_millis(250);

pub(crate) struct Manager {
    catalog: Arc<FontCatalog>,
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
}

struct Attached {
    project_id: String,
    inner: Weak<Inner>,
    branches: BTreeMap<String, BranchFonts>,
    /// Test builds: pins set by `injectFontPinsForTest`, beside the branches' (until the
    /// model's `font.*` ops exist, Task 18).
    #[cfg(feature = "test-hooks")]
    injected: BranchFonts,
    /// The last summary published (as a status event or a status answer) and when.
    published: Option<(Summary, Instant)>,
}

impl Attached {
    fn all(&self) -> impl Iterator<Item = &BranchFonts> {
        let all = self.branches.values();
        #[cfg(feature = "test-hooks")]
        let all = all.chain(std::iter::once(&self.injected));
        all
    }

    fn needs(&self) -> BTreeSet<Need> {
        self.all().flat_map(|b| b.needs.iter().cloned()).collect()
    }
}

static MANAGERS: Mutex<Vec<(PathBuf, Weak<Manager>)>> = Mutex::new(Vec::new());

/// The manager of `server_data`, made on first use.
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
        let m = Arc::new_cyclic(|weak| Manager {
            catalog,
            fetcher,
            damaged,
            env_offline,
            setting: AtomicBool::new(true),
            measurer: Arc::new(provider::Measurer::new(provider.clone(), weak.clone())),
            provider,
            engines: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
        });
        if let Some(rx) = changes {
            let weak = Arc::downgrade(&m);
            let _ = std::thread::Builder::new()
                .name("fin-node-fonts".to_string())
                .spawn(move || {
                    // Ends when the fetcher (owned by the manager) is gone.
                    while let Ok((sha, _)) = rx.recv() {
                        let Some(m) = weak.upgrade() else { return };
                        m.file_changed(&sha);
                    }
                });
        }
        m
    }

    pub(crate) fn catalog(&self) -> &FontCatalog {
        &self.catalog
    }

    /// The shared measurer (Task 20 threads it through the edit path).
    pub(crate) fn measurer(&self) -> Arc<dyn TextMeasurer> {
        self.measurer.clone()
    }

    /// Whether background fetching is on now.
    pub(crate) fn auto_fetch(&self) -> bool {
        self.fetcher.is_some() && !self.env_offline && self.setting.load(Ordering::SeqCst)
    }

    /// `fonts.autoFetch`. Turned on, the open projects' missing pins are queued.
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
            self.request_needs(needs.iter());
        }
        self.refresh_all();
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

    /// Registers an engine (at open) and queues what its pins lack.
    pub(crate) fn attach(
        self: &Arc<Self>,
        project_id: String,
        inner: Weak<Inner>,
        branches: BTreeMap<String, BranchFonts>,
    ) -> Handle {
        let key = self.next.fetch_add(1, Ordering::SeqCst);
        let needs: Vec<Need> = branches
            .values()
            .flat_map(|b| b.needs.iter().cloned())
            .collect();
        lock(&self.engines).insert(
            key,
            Attached {
                project_id,
                inner,
                branches,
                #[cfg(feature = "test-hooks")]
                injected: BranchFonts::default(),
                published: None,
            },
        );
        self.update_assets();
        self.request_needs(needs.iter());
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

    /// Reads the pins again: of `branch`, or of every branch. New needs are queued.
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
        self.request_needs(added.iter());
        self.refresh(key);
    }

    /// Test builds: sets the engine's injected pins, as a commit pinning them would.
    #[cfg(feature = "test-hooks")]
    fn inject(&self, key: u64, fonts: BranchFonts) {
        let needs: Vec<Need> = fonts.needs.iter().cloned().collect();
        if let Some(a) = lock(&self.engines).get_mut(&key) {
            a.injected = fonts;
        }
        self.update_assets();
        self.request_needs(needs.iter());
        self.refresh(key);
    }

    /// The summary of an engine's pins now (`None` once detached).
    fn summary_of(&self, key: u64) -> Option<Summary> {
        let needs = lock(&self.engines).get(&key)?.needs();
        Some(self.summary(&needs))
    }

    /// `raw` (the store's status) with the engine's font summary merged, which counts as
    /// published.
    fn merged(&self, key: u64, raw: Value) -> Value {
        let Some(s) = self.summary_of(key) else {
            return raw;
        };
        let out = status::merge(raw, &s);
        if let Some(a) = lock(&self.engines).get_mut(&key) {
            a.published = Some((s, Instant::now()));
        }
        out
    }

    /// Publishes a `status` event when the engine's summary changed since it was published
    /// (progress alone: at most every [`PROGRESS_EVERY`]).
    fn refresh(&self, key: u64) {
        let Some(s) = self.summary_of(key) else {
            return;
        };
        let inner = {
            let mut engines = lock(&self.engines);
            let Some(a) = engines.get_mut(&key) else {
                return;
            };
            let due = match &a.published {
                None => true,
                Some((p, at)) => {
                    p.shape != s.shape
                        || p.warnings != s.warnings
                        || (p.fonts != s.fonts && at.elapsed() >= PROGRESS_EVERY)
                }
            };
            if !due {
                return;
            }
            let Some(inner) = a.inner.upgrade() else {
                return;
            };
            a.published = Some((s.clone(), Instant::now()));
            inner
        };
        let raw = lock(&inner.status).clone();
        if let Some(raw) = raw {
            let status = status::merge(raw, &s);
            if let Ok(json) = serde_json::to_string(&EngineEvent::Status { status }) {
                inner.events.emit(json);
            }
        }
    }

    fn refresh_all(&self) {
        let keys: Vec<u64> = lock(&self.engines).keys().copied().collect();
        for key in keys {
            self.refresh(key);
        }
    }

    /// A file's state changed: refresh every engine that pins it.
    fn file_changed(&self, sha: &str) {
        let keys: Vec<u64> = lock(&self.engines)
            .iter()
            .filter(|(_, a)| a.all().any(|b| b.needs.iter().any(|n| n.sha256 == sha)))
            .map(|(k, _)| *k)
            .collect();
        for key in keys {
            self.refresh(key);
        }
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

    pub(crate) fn detach(&self) {
        self.manager.detach(self.key);
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn inject(&self, fonts: BranchFonts) {
        self.manager.inject(self.key, fonts);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.manager.detach(self.key);
    }
}
