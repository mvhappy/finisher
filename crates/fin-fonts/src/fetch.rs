//! The fetcher (4C Decisions 33 and 34): fills the cache with catalog files.
//!
//! - [`Fetcher::request`] queues a file in the background unless it is cached, queued,
//!   fetching or backing off; with [`FetchOptions::offline`] it never touches the network
//!   (owner answer 1: the off switch, `FIN_FONTS_OFFLINE=1` or `fonts.autoFetch: false`).
//! - [`Fetcher::fetch_now`] fetches at once and blocks (`fin fonts fetch`, the explicit
//!   command, which the off switch does not stop).
//! - Per source, [`FetchOptions::backoff`] attempts (waiting 1 s, 4 s, 16 s after each), then
//!   the next source. A total failure is `FONT_FETCH_FAILED`, retried automatically after
//!   [`FetchOptions::retry_after`] (a background request) or at once by `fetch_now`.
//! - At most [`crate::MAX_DOWNLOADS`] downloads at a time per process; across processes one
//!   `<sha256>.lock` per file, so a file is downloaded once.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use fin_core::CoreError;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::cache::FontCache;
use crate::catalog::{CatalogSource, CatalogStyle, FontCatalog};
use crate::download::{Target, download};
use crate::http::{Failure, Http};
use crate::slot::Slot;
use crate::{archive, lock, not_in_catalog, now_ms};

#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// Never fetch in the background (`FIN_FONTS_OFFLINE=1`, `fonts.autoFetch: false`).
    pub offline: bool,
    /// Plain `http` to loopback; honoured only in `test-hooks` builds.
    pub allow_loopback_http: bool,
    /// Proxies from `ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY` (and the Windows
    /// system proxy). On by default.
    pub proxy_from_env: bool,
    /// The wait after each failed attempt at a source; its length is the attempts per source.
    pub backoff: Vec<Duration>,
    /// How long a failed file waits before the next automatic attempt.
    pub retry_after: Duration,
    /// How long to wait for another fetcher's lock on the same file.
    pub lock_wait: Duration,
}

impl Default for FetchOptions {
    fn default() -> Self {
        FetchOptions {
            offline: false,
            allow_loopback_http: false,
            proxy_from_env: true,
            backoff: [1, 4, 16].map(Duration::from_secs).to_vec(),
            retry_after: Duration::from_secs(10 * 60),
            lock_wait: Duration::from_secs(10 * 60),
        }
    }
}

/// A catalog file's state in this server (Decision 34).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum FileState {
    Cached,
    /// Not cached and not queued: never requested, or background fetching is off.
    Missing,
    Queued,
    Fetching {
        bytes: u64,
        total: u64,
    },
    #[serde(rename_all = "camelCase")]
    Failed {
        reason: String,
        retry_at_ms: i64,
    },
}

pub struct Fetcher {
    inner: Arc<Inner>,
}

struct Inner {
    cache: Arc<FontCache>,
    catalog: Arc<FontCatalog>,
    opts: FetchOptions,
    /// [`FetchOptions::offline`], changeable at run time ([`Fetcher::set_offline`]).
    offline: AtomicBool,
    http: Http,
    /// Queued, fetching and failed files (cached ones are the cache's).
    states: Mutex<HashMap<String, FileState>>,
    subs: Mutex<Vec<Sender<(String, FileState)>>>,
}

fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Fetcher {
    pub fn new(cache: Arc<FontCache>, catalog: Arc<FontCatalog>, opts: FetchOptions) -> Fetcher {
        let http = Http::new(opts.allow_loopback_http, opts.proxy_from_env);
        Fetcher {
            inner: Arc::new(Inner {
                cache,
                catalog,
                offline: AtomicBool::new(opts.offline),
                opts,
                http,
                states: Mutex::new(HashMap::new()),
                subs: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Turns background fetching off or on (`fonts.autoFetch`). Queued and running fetches
    /// finish; background retries stop while it is off.
    pub fn set_offline(&self, offline: bool) {
        self.inner.offline.store(offline, Ordering::SeqCst);
    }

    pub fn offline(&self) -> bool {
        self.inner.offline.load(Ordering::SeqCst)
    }

    pub fn cache(&self) -> &Arc<FontCache> {
        &self.inner.cache
    }

    pub fn catalog(&self) -> &Arc<FontCatalog> {
        &self.inner.catalog
    }

    /// Queues a background fetch unless the file is cached, queued, fetching or backing
    /// off, is not in the catalog, or background fetching is off.
    pub fn request(&self, sha256: &str) {
        let inner = &self.inner;
        if inner.offline.load(Ordering::SeqCst)
            || inner.catalog.by_sha(sha256).is_none()
            || inner.cache.has(sha256)
        {
            return;
        }
        {
            let mut states = guard(&inner.states);
            match states.get(sha256) {
                Some(FileState::Queued | FileState::Fetching { .. }) => return,
                Some(FileState::Failed { retry_at_ms, .. }) if now_ms() < *retry_at_ms => return,
                _ => {}
            }
            states.insert(sha256.to_string(), FileState::Queued);
        }
        inner.publish(sha256, FileState::Queued);
        let weak = Arc::downgrade(inner);
        let sha = sha256.to_string();
        std::thread::spawn(move || background(&weak, &sha));
    }

    /// Fetches the file now, waiting for a download slot and for any other fetcher of it.
    /// `FONT_NOT_IN_CATALOG`, or `FONT_FETCH_FAILED {key, entry, style, sha256, reason,
    /// retryAt}`.
    pub fn fetch_now(&self, sha256: &str) -> Result<(), CoreError> {
        if self.inner.catalog.by_sha(sha256).is_none() {
            return Err(not_in_catalog(sha256));
        }
        if self.inner.cache.has(sha256) {
            return Ok(());
        }
        guard(&self.inner.states).insert(sha256.to_string(), FileState::Queued);
        self.inner.run(sha256)
    }

    pub fn state(&self, sha256: &str) -> FileState {
        let inner = &self.inner;
        let held = guard(&inner.states).get(sha256).cloned();
        if let Some(s @ (FileState::Queued | FileState::Fetching { .. })) = held {
            return s;
        }
        if inner.cache.has(sha256) {
            return FileState::Cached;
        }
        if let Some(s @ FileState::Failed { .. }) = held {
            return s;
        }
        match inner.cache.mismatch(sha256) {
            Some(at) => FileState::Failed {
                reason: "hashMismatch".into(),
                retry_at_ms: at,
            },
            None => FileState::Missing,
        }
    }

    /// Every later state change, as `(sha256, state)`.
    pub fn subscribe(&self) -> Receiver<(String, FileState)> {
        let (tx, rx) = channel();
        guard(&self.inner.subs).push(tx);
        rx
    }
}

/// A background request: fetch, and after a failure wait out `retry_after` and try again
/// while the fetcher lives and nobody else picked the file up meanwhile.
fn background(weak: &Weak<Inner>, sha: &str) {
    loop {
        let Some(inner) = weak.upgrade() else {
            return;
        };
        if inner.run(sha).is_ok() {
            return;
        }
        let failed = guard(&inner.states).get(sha).cloned();
        let Some(FileState::Failed { retry_at_ms, .. }) = failed else {
            return;
        };
        drop(inner);
        let wait = u64::try_from(retry_at_ms - now_ms()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(wait));
        let Some(inner) = weak.upgrade() else {
            return;
        };
        {
            let mut states = guard(&inner.states);
            if states.get(sha) != failed.as_ref()
                || inner.cache.has(sha)
                || inner.offline.load(Ordering::SeqCst)
            {
                return;
            }
            states.insert(sha.to_string(), FileState::Queued);
        }
        inner.publish(sha, FileState::Queued);
    }
}

impl Inner {
    fn publish(&self, sha: &str, state: FileState) {
        guard(&self.subs).retain(|tx| tx.send((sha.to_string(), state.clone())).is_ok());
    }

    fn set(&self, sha: &str, state: FileState) {
        guard(&self.states).insert(sha.to_string(), state.clone());
        self.publish(sha, state);
    }

    fn run(&self, sha: &str) -> Result<(), CoreError> {
        let Some((entry, style)) = self.catalog.by_sha(sha) else {
            return Err(not_in_catalog(sha));
        };
        match self.fetch_file(style) {
            Ok(()) => {
                guard(&self.states).remove(sha);
                self.publish(sha, FileState::Cached);
                Ok(())
            }
            Err(f) => {
                let retry_at_ms = now_ms()
                    + i64::try_from(self.opts.retry_after.as_millis()).unwrap_or(i64::MAX / 2);
                self.set(
                    sha,
                    FileState::Failed {
                        reason: f.reason.clone(),
                        retry_at_ms,
                    },
                );
                let face = style.face();
                let key = format!(
                    "{}|{}|{}",
                    face.family,
                    face.weight,
                    if face.italic { "italic" } else { "normal" }
                );
                Err(CoreError::new(
                    "FONT_FETCH_FAILED",
                    format!("cannot fetch {}:{}: {}", entry.id, style.id, f.message),
                )
                .with_details(json!({
                    "key": key, "entry": entry.id, "style": style.id, "sha256": sha,
                    "reason": f.reason, "retryAt": retry_at_ms,
                }))
                .with_fix(format!(
                    "check the network (behind a proxy, set HTTPS_PROXY), then: fin fonts fetch {}",
                    entry.id
                )))
            }
        }
    }

    /// Under the file's lock: done if another fetcher placed it meanwhile; else each source
    /// in turn, each with its attempts and backoff. A download slot is taken per attempt,
    /// after every lock it needs, so a fetch waiting on another's lock never holds one.
    fn fetch_file(&self, style: &CatalogStyle) -> Result<(), Failure> {
        let sha = style.file.sha256.as_str();
        let _lock = self.lock(sha)?;
        if self.cache.has(sha) {
            return Ok(());
        }
        self.cache.discard_bad(sha);
        let attempts = self.opts.backoff.len().max(1);
        let mut last = Failure::new("noSource", "no sources", false);
        for (i, src) in style.sources.iter().enumerate() {
            for attempt in 0..attempts {
                match self.try_source(style, src) {
                    Ok(()) => return Ok(()),
                    Err(f) => last = f,
                }
                let more = attempt + 1 < attempts || i + 1 < style.sources.len();
                if !last.retry {
                    break;
                }
                if more {
                    let wait = self.opts.backoff.get(attempt).copied().unwrap_or_default();
                    std::thread::sleep(wait);
                }
            }
        }
        Err(last)
    }

    /// Waits for one of the process's download slots, then reports `Fetching`.
    fn slot(&self, sha: &str, total: u64) -> Slot {
        let slot = Slot::take();
        self.set(sha, FileState::Fetching { bytes: 0, total });
        slot
    }

    fn lock(&self, sha: &str) -> Result<lock::Held, Failure> {
        match lock::acquire(&self.cache.lock_path(sha), self.opts.lock_wait) {
            Ok(Some(held)) => Ok(held),
            Ok(None) => Err(Failure::new(
                "lockTimeout",
                format!("another fetcher held {sha} too long"),
                true,
            )),
            Err(e) => Err(Failure::new("io", e.to_string(), false)),
        }
    }

    fn try_source(&self, style: &CatalogStyle, src: &CatalogSource) -> Result<(), Failure> {
        let file = &style.file;
        let mut last = 0u64;
        let mut progress = |bytes: u64, total: u64| {
            if bytes == total || bytes >= last + 64 * 1024 {
                last = bytes;
                self.set(&file.sha256, FileState::Fetching { bytes, total });
            }
        };
        let (Some(m), Some(asha), Some(asize)) = (&src.archive, &src.sha256, src.size) else {
            let target = Target {
                url: &src.url,
                sha256: &file.sha256,
                size: file.size,
            };
            let _slot = self.slot(&file.sha256, file.size);
            let part = download(&self.http, &self.cache, &target, &mut progress)?;
            let placed = self.cache.place(&file.sha256, &part);
            let _ = std::fs::remove_file(self.cache.part_json_path(&file.sha256));
            return placed.map_err(|e| Failure::new("io", e.to_string(), false));
        };
        let _archive_lock = self.lock(asha)?;
        // A sibling's fetch may have extracted this file while we waited for the archive.
        if self.cache.has(&file.sha256) {
            return Ok(());
        }
        let _slot = self.slot(&file.sha256, asize);
        let target = Target {
            url: &src.url,
            sha256: asha,
            size: asize,
        };
        let zip = download(&self.http, &self.cache, &target, &mut progress)?;
        let result = archive::extract(&self.cache, &zip, &m.member, &file.sha256, file.size);
        // The archive's other catalog members, so their own fetch needs no download. They are
        // placed under the archive's lock, not their own: a sibling fetch holding its lock is
        // waiting for this archive, and finds its file cached once it gets the archive's lock.
        // `FontCache::place` never replaces (see its rename fallback).
        for (other, member) in self.catalog.archive_members(asha) {
            let sha = &other.file.sha256;
            if result.is_ok() && sha != &file.sha256 && !self.cache.has(sha) {
                let _ = archive::extract(&self.cache, &zip, &member.member, sha, other.file.size);
            }
        }
        let _ = std::fs::remove_file(&zip);
        let _ = std::fs::remove_file(self.cache.part_json_path(asha));
        result
    }
}
