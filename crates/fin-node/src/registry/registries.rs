//! `Registry`'s registry calls (8A plan Task 9a): the local-folder registries of a server
//! (`registry.add|remove|forget|refresh|status|list`) and what they offer (`search`, `info`),
//! over [`fin_registry::registry::Registry`]. Same worker thread and promise discipline as
//! the plan calls. A refresh journals `registry.refresh`; trust is added only with a root
//! (a root file or its SHA-256), never on first use.

// Reads a registry's root file the owner named in registry.add; plugin registries are never library paths.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use fin_core::CoreError;
use fin_plugin_api::{PluginId, PluginKind};
use fin_registry::registry::{Registry as Reg, RegistryConfig, RootSource};
use fin_registry::search::{Listing, PluginInfo, SearchQuery};
use fin_registry::status::{RegistryList, RegistryProblem, RegistryStatus};
use fin_registry::tuf::{SystemClock, TufError, sha256_hex};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::{Value, json};

use super::log::{Refreshed, registry_refresh};
use super::{Inner, Registry, bad_request, on_worker};
use crate::{encode, reject};

fn tuf(e: TufError) -> Error {
    reject(CoreError {
        code: e.code.into(),
        message: e.message,
        details: Value::Object(e.details),
        fix: None,
    })
}

/// `registryAdd`'s registry: where it is. Its trust comes from the root.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddReq {
    id: String,
    path: String,
}

/// How the owner names the root (spec §5.2): a root file of the folder, or its SHA-256 (typed,
/// or shown by the console and typed back).
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum RootReq {
    File { path: String },
    Sha256 { sha256: String },
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchReq {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    kind: Option<PluginKind>,
    /// Only this registry (`REGISTRY_NOT_FOUND` when it is not added).
    #[serde(default)]
    registry: Option<String>,
}

fn base(inner: &Inner) -> &Path {
    &inner.base
}

fn status_of(inner: &Inner, id: &str) -> std::result::Result<RegistryStatus, TufError> {
    Ok(Reg::open(base(inner), id)?.status(&SystemClock))
}

/// The ids to read: `only`, or every added registry.
fn ids(inner: &Inner, only: Option<&str>) -> std::result::Result<Vec<String>, TufError> {
    match only {
        Some(id) => {
            // Opening names a missing one `REGISTRY_NOT_FOUND`.
            Reg::open(base(inner), id)?;
            Ok(vec![id.to_string()])
        }
        None => Ok(Reg::list(base(inner))),
    }
}

#[napi]
impl Registry {
    /// Every added registry's status (`RegistryList` JSON); one that cannot be opened is a
    /// `problems` entry and does not hide the others.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn registry_list<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let mut out = RegistryList::default();
            for id in Reg::list(base(&inner)) {
                match status_of(&inner, &id) {
                    Ok(s) => out.registries.push(s),
                    Err(e) => out.problems.push(RegistryProblem {
                        id,
                        code: e.code.into(),
                        message: e.message,
                    }),
                }
            }
            encode(&out)
        })
    }

    /// One registry's `RegistryStatus` JSON, or `REGISTRY_NOT_FOUND`.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn registry_status<'e>(&self, env: &'e Env, id: String) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            encode(&status_of(&inner, &id).map_err(tuf)?)
        })
    }

    /// Adds registry `{id, path}` (`AddReq` JSON) trusted through `root` (`RootReq` JSON);
    /// resolves to its `RegistryStatus` JSON. A hash or file that is no root of the folder is
    /// `REGISTRY_UNTRUSTED` and nothing is written. It fetches nothing: refresh next.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn registry_add<'e>(
        &self,
        env: &'e Env,
        cfg_json: String,
        root_json: String,
    ) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let cfg: AddReq =
                serde_json::from_str(&cfg_json).map_err(|e| bad_request("the registry", e))?;
            let root: RootReq =
                serde_json::from_str(&root_json).map_err(|e| bad_request("the root", e))?;
            let (source, root_sha256) = match root {
                RootReq::File { path } => {
                    let bytes = std::fs::read(&path).map_err(|e| {
                        reject(
                            CoreError::new("IO_ERROR", format!("read {path}: {e}"))
                                .with_details(json!({ "op": "read", "path": path })),
                        )
                    })?;
                    let sha = sha256_hex(&bytes);
                    (RootSource::File(bytes), sha)
                }
                RootReq::Sha256 { sha256 } => {
                    let sha = sha256.to_ascii_lowercase();
                    (RootSource::Sha256(sha.clone()), sha)
                }
            };
            let reg = Reg::add(
                base(&inner),
                RegistryConfig {
                    id: cfg.id,
                    path: PathBuf::from(cfg.path),
                    root_sha256,
                },
                source,
            )
            .map_err(tuf)?;
            encode(&reg.status(&SystemClock))
        })
    }

    /// Deletes the registry's config; its trusted metadata stays (`forget` deletes it).
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn registry_remove<'e>(&self, env: &'e Env, id: String) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            Reg::remove(base(&inner), &id).map_err(tuf)
        })
    }

    /// Deletes the trusted metadata of a root no registry uses.
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn registry_forget<'e>(&self, env: &'e Env, root_sha256: String) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            Reg::forget(base(&inner), &root_sha256).map_err(tuf)
        })
    }

    /// Refreshes the registry from its folder and resolves to `{status, report}` JSON
    /// (`RegistryStatus`, `RefreshReport`). `by_human`: a person asked (journalled; the
    /// caller installs nothing for anyone else).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn registry_refresh<'e>(
        &self,
        env: &'e Env,
        id: String,
        by_human: bool,
    ) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let started = Instant::now();
            let out = refresh(&inner, &id);
            match &out {
                Ok((_, _, seen)) => registry_refresh(&inner.key, by_human, Ok(seen), started),
                Err(e) => registry_refresh(&inner.key, by_human, Err(e.code), started),
            }
            let (status, report, _) = out.map_err(tuf)?;
            encode(&json!({ "status": status, "report": report }))
        })
    }

    /// Plugins the registries list (`SearchReq` JSON), as `Listing` JSON, ordered by id then
    /// registry. A registry that cannot be opened is skipped (it shows in `registryList`).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn search<'e>(&self, env: &'e Env, query_json: String) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let q: SearchReq =
                serde_json::from_str(&query_json).map_err(|e| bad_request("the search", e))?;
            let query = SearchQuery {
                text: q.text,
                kind: q.kind,
            };
            let mut out: Vec<Listing> = Vec::new();
            for id in ids(&inner, q.registry.as_deref()).map_err(tuf)? {
                if let Ok(r) = Reg::open(base(&inner), &id) {
                    out.extend(r.search(&query));
                }
            }
            out.sort_by(|a, b| (&a.id, &a.registry_id).cmp(&(&b.id, &b.registry_id)));
            encode(&out)
        })
    }

    /// What each registry signs about plugin `id` (`PluginInfo` JSON array, one per registry
    /// that lists it; empty when none does).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn info<'e>(&self, env: &'e Env, id: String) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let pid = PluginId::parse(&id).map_err(|e| {
                reject(
                    CoreError::new("PLUGIN_NOT_FOUND", format!("{id:?} is not a plugin id"))
                        .with_details(json!({ "id": id, "reason": e.reason() })),
                )
            })?;
            let mut out: Vec<PluginInfo> = Vec::new();
            for rid in Reg::list(base(&inner)) {
                if let Ok(r) = Reg::open(base(&inner), &rid)
                    && let Some(i) = r.info(&pid)
                {
                    out.push(i);
                }
            }
            encode(&out)
        })
    }
}

type Refresh = (RegistryStatus, fin_registry::tuf::RefreshReport, Refreshed);

fn refresh(inner: &Inner, id: &str) -> std::result::Result<Refresh, TufError> {
    let mut reg = Reg::open(base(inner), id)?;
    let before = reg.trusted().versions();
    let report = reg.refresh(&SystemClock)?;
    let after = reg.trusted().versions();
    let moved = [
        before.0 != after.0,
        before.1 != after.1,
        before.2 != after.2,
        before.3 != after.3,
    ];
    let seen = Refreshed {
        roles_updated: moved.iter().map(|m| u16::from(*m)).sum(),
        targets: reg.trusted().targets().count() as u64,
        stale_days: report.stale_days,
    };
    Ok((reg.status(&SystemClock), report, seen))
}
