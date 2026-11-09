//! The napi `MediaCache` (6B-1 plan Task 13; sub-project 6 spec §7.2): the server's media cache
//! at `<server-data>/cache/media/`, written by `fin-node` only. Every call runs in order on the
//! cache's own thread (`fin-node-cache`) and settles a promise; values cross as JSON. The
//! cache itself is napi-free ([`state::Cache`]; `tests/media_cache.rs` drives it directly).
//!
//! - `mediaCacheOpen(serverData, capBytes?)` opens it (the index, then the adoption at start)
//!   and resolves to a `MediaCache`; `openReport()` says what the adoption did.
//! - `lookup(key)` (touches the last use), `commit(jobOutDir, key, projects)` (rename, then
//!   insert; `commit.rs`), `pin`/`unpin(key, holder)`, `release(holder)`,
//!   `pinProject`/`unpinProject(projectId)`, `entriesFor(fingerprint)`, `projectsOf(key)`,
//!   `status()`, `gc(targetBytes?)`, `setCap(capBytes?)`, `entryDir(key)`.
//!
//! Eviction runs after each commit (here) and every 10 minutes (finisherd's timer calls `gc`).

mod adopt;
mod commit;
mod evict;
mod index;
mod state;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fin_core::CoreError;
use fin_media::cache::CacheKey;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::json;

use crate::worker::Worker;
use crate::{guarded, reject};
use state::Cache;

/// The server's media cache (module docs).
#[napi]
pub struct MediaCache {
    cache: Arc<Mutex<Cache>>,
    worker: Worker,
    root: PathBuf,
    report: String,
}

fn bytes_arg(field: &str, v: Option<f64>) -> Result<Option<u64>> {
    match v {
        None => Ok(None),
        Some(n) if n.is_finite() && n >= 0.0 => Ok(Some(n as u64)),
        Some(_) => Err(reject(
            CoreError::new(
                "INVALID_OP",
                format!("{field} is a number of bytes, 0 or more"),
            )
            .with_details(json!({ "field": field, "reason": "mediaCache" })),
        )),
    }
}

impl MediaCache {
    /// Runs `f` on the cache thread with the cache and settles the promise with its value.
    fn run<'e, T: ToNapiValue + Send + 'static>(
        &self,
        env: &'e Env,
        f: impl FnOnce(&mut Cache) -> std::result::Result<T, CoreError> + Send + 'static,
    ) -> Result<Object<'e>> {
        let (deferred, promise) = env.create_deferred::<T, _>()?;
        let cache = self.cache.clone();
        self.worker.submit(Box::new(move || {
            let done = guarded(
                || {
                    let mut c = cache.lock().unwrap_or_else(|p| p.into_inner());
                    f(&mut c).map_err(reject)
                },
                || {},
            );
            match done {
                Ok(v) => deferred.resolve(move |_env| Ok(v)),
                Err(e) => deferred.reject(e),
            }
        }))?;
        Ok(promise)
    }
}

fn json_of(value: &impl serde::Serialize) -> std::result::Result<String, CoreError> {
    serde_json::to_string(value).map_err(|e| CoreError::new("INTERNAL", e.to_string()))
}

/// Opens the media cache of `serverData` (`cache/media/`, created) with a cap of `capBytes`
/// (absent: `min(50 GiB, 15 % of the volume)`): the index (replaced when damaged or of another
/// version), then the adoption of what a crash left. Rejects with `CACHE_IO` when the
/// directory or the index cannot be made.
#[napi(catch_unwind, ts_return_type = "Promise<MediaCache>")]
pub fn media_cache_open<'e>(
    env: &'e Env,
    server_data: String,
    cap_bytes: Option<f64>,
) -> Result<Object<'e>> {
    let cap = bytes_arg("capBytes", cap_bytes)?;
    let worker = Worker::spawn_named("fin-node-cache")?;
    let (deferred, promise) = env.create_deferred::<MediaCache, _>()?;
    let keep = worker.clone();
    worker.submit(Box::new(move || {
        let opened = guarded(
            || {
                let (cache, adopted) =
                    Cache::open(std::path::Path::new(&server_data), cap, None).map_err(reject)?;
                let report = json!({
                    "adopted": adopted.adopted,
                    "deleted": adopted.deleted,
                    "dropped": adopted.dropped,
                    "indexReplaced": adopted.index_replaced,
                })
                .to_string();
                Ok((cache, report))
            },
            || {},
        );
        match opened {
            Ok((cache, report)) => deferred.resolve(move |_env| {
                Ok(MediaCache {
                    root: cache.root().to_path_buf(),
                    cache: Arc::new(Mutex::new(cache)),
                    worker: keep,
                    report,
                })
            }),
            Err(e) => deferred.reject(e),
        }
    }))?;
    Ok(promise)
}

#[napi]
impl MediaCache {
    /// What the adoption at open did: `{adopted, deleted, dropped, indexReplaced}` JSON.
    #[napi(catch_unwind)]
    pub fn open_report(&self) -> String {
        self.report.clone()
    }

    /// The directory of the entry `key` (whether or not it exists); `null` for a malformed key.
    #[napi(catch_unwind)]
    pub fn entry_dir(&self, key: String) -> Option<String> {
        CacheKey::parse(&key).ok().map(|k| {
            self.root
                .join(k.shard())
                .join(k.as_str())
                .to_string_lossy()
                .into_owned()
        })
    }

    /// The entry's `EntryMeta` JSON, its last use set to now; `null` when there is none.
    #[napi(catch_unwind, ts_return_type = "Promise<string | null>")]
    pub fn lookup<'e>(&self, env: &'e Env, key: String) -> Result<Object<'e>> {
        self.run(env, move |c| {
            c.lookup(&key)?.map(|m| json_of(&m)).transpose()
        })
    }

    /// Commits `<jobOutDir>/<key>/` (fin-shm's checks, meta and file hashes verified, rename into
    /// place, then the row); resolves to the entry's `EntryMeta` JSON (the existing entry's when
    /// the key is already cached: first wins). `projects`: the projects the entry serves.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn commit<'e>(
        &self,
        env: &'e Env,
        job_out_dir: String,
        key: String,
        projects: Vec<String>,
    ) -> Result<Object<'e>> {
        self.run(env, move |c| {
            json_of(&c.commit(std::path::Path::new(&job_out_dir), &key, &projects)?)
        })
    }

    /// Pins `key` for `holder` (`job:<id>`, `project:<id>`); false when there is no such entry.
    #[napi(catch_unwind, ts_return_type = "Promise<boolean>")]
    pub fn pin<'e>(&self, env: &'e Env, key: String, holder: String) -> Result<Object<'e>> {
        self.run(env, move |c| c.pin(&key, &holder))
    }

    #[napi(catch_unwind, ts_return_type = "Promise<boolean>")]
    pub fn unpin<'e>(&self, env: &'e Env, key: String, holder: String) -> Result<Object<'e>> {
        self.run(env, move |c| c.unpin(&key, &holder))
    }

    /// Removes every pin of `holder` (a job's end hook); how many.
    #[napi(catch_unwind, ts_return_type = "Promise<number>")]
    pub fn release<'e>(&self, env: &'e Env, holder: String) -> Result<Object<'e>> {
        self.run(env, move |c| c.release(&holder).map(|n| n as f64))
    }

    /// The project is open here: its entries are pinned until `unpinProject`; how many.
    #[napi(catch_unwind, ts_return_type = "Promise<number>")]
    pub fn pin_project<'e>(&self, env: &'e Env, project_id: String) -> Result<Object<'e>> {
        self.run(env, move |c| c.pin_project(&project_id).map(|n| n as f64))
    }

    #[napi(catch_unwind, ts_return_type = "Promise<number>")]
    pub fn unpin_project<'e>(&self, env: &'e Env, project_id: String) -> Result<Object<'e>> {
        self.run(env, move |c| c.unpin_project(&project_id).map(|n| n as f64))
    }

    /// The `EntryMeta[]` JSON of the entries made from the source with `fingerprint`.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn entries_for<'e>(&self, env: &'e Env, fingerprint: String) -> Result<Object<'e>> {
        self.run(env, move |c| json_of(&c.entries_for(&fingerprint)?))
    }

    /// The projects `key` serves (reach, spec §7.3).
    #[napi(catch_unwind, ts_return_type = "Promise<string[]>")]
    pub fn projects_of<'e>(&self, env: &'e Env, key: String) -> Result<Object<'e>> {
        self.run(env, move |c| c.projects_of(&key))
    }

    /// The `CacheStatus` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn status<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        self.run(env, |c| json_of(&c.status()?))
    }

    /// Evicts unpinned entries, least recently used first, down to `targetBytes` (at most the
    /// cap; the cap when absent); the `CacheGcResult` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn gc<'e>(&self, env: &'e Env, target_bytes: Option<f64>) -> Result<Object<'e>> {
        let target = bytes_arg("targetBytes", target_bytes)?;
        self.run(env, move |c| json_of(&c.gc(target)?))
    }

    /// Sets the cap (`media.cacheMaxGiB`; absent: the default) and evicts down to it; the
    /// `CacheGcResult` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn set_cap<'e>(&self, env: &'e Env, cap_bytes: Option<f64>) -> Result<Object<'e>> {
        let cap = bytes_arg("capBytes", cap_bytes)?;
        self.run(env, move |c| json_of(&c.set_cap(cap)?))
    }

    /// The holders of `key`'s pins.
    #[napi(catch_unwind, ts_return_type = "Promise<string[]>")]
    pub fn pins_of<'e>(&self, env: &'e Env, key: String) -> Result<Object<'e>> {
        self.run(env, move |c| c.pins_of(&key))
    }
}

#[cfg(feature = "test-hooks")]
#[napi]
impl MediaCache {
    /// Test builds only: `{stopAfterRename?, freeBytes?}` (a commit stops between its rename
    /// and its insert; the free-space check before a move reads `freeBytes`).
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn set_faults_for_test<'e>(&self, env: &'e Env, faults_json: String) -> Result<Object<'e>> {
        self.run(env, move |c| {
            let v: serde_json::Value = serde_json::from_str(&faults_json)
                .map_err(|e| CoreError::new("INTERNAL", e.to_string()))?;
            c.faults.stop_after_rename = v["stopAfterRename"].as_bool().unwrap_or(false);
            c.faults.free_bytes = v["freeBytes"].as_u64();
            Ok(())
        })
    }
}
