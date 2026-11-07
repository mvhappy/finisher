//! The `Registry` class (8A plan Task 8): plugin plans, commits, removals and the staging
//! sweep over `<server-data>/plugins` ([`fin_registry::Store`]), and `validateManifest`.
//!
//! Every call runs on the registry's own worker thread, one at a time in call order, and
//! settles a promise: a plan copies, hashes and unpacks a package, which may take seconds,
//! and never holds the JS thread or the libuv pool. Each call journals `plugin.lifecycle`
//! ([`log`]). Nothing a package ships runs.
//!
//! Sources: `registry` (a configured local registry; there are none until 8A Task 7, so it is
//! `REGISTRY_NOT_FOUND`) and, in `test-hooks` builds only, `package` (a `.finpkg` file
//! described as a publisher would sign it, standing in for a verified target).

mod host;
mod log;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use fin_core::CoreError;
use fin_plugin_api::{MANIFEST_FILE, PluginId, SourceClass, validate};
use fin_registry::plan::{
    Accept, InstalledPlugin, PlanInput, PlanRequest, SourceKind, ValidationReport, make_plan,
};
use fin_registry::{RegistryError, Store, SystemClock};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::json;

use crate::worker::Worker;
use crate::{encode, guarded, reject};
use log::{About, action_name, lifecycle};

fn core(e: RegistryError) -> CoreError {
    CoreError {
        code: e.code,
        message: e.message,
        details: serde_json::Value::Object(e.details),
        fix: e.fix,
    }
}

fn bad_request(what: &str, e: impl std::fmt::Display) -> Error {
    reject(
        CoreError::new("INVALID_OP", format!("{what} does not decode: {e}"))
            .with_details(json!({ "reason": e.to_string() })),
    )
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum SourceReq {
    Registry {
        registry: String,
        #[serde(default)]
        #[allow(dead_code)]
        version: Option<String>,
    },
    #[cfg(feature = "test-hooks")]
    #[serde(rename_all = "camelCase")]
    Package {
        archive: String,
        registry_id: String,
        #[serde(default)]
        stale_days: Option<u16>,
        #[serde(default)]
        yanked: bool,
    },
}

/// `Registry.plan`'s request: [`PlanRequest`] and where the package comes from.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanReq {
    id: String,
    source: SourceReq,
    #[serde(default)]
    update: bool,
    #[serde(default)]
    allow_downgrade: bool,
    #[serde(default)]
    installed: Option<InstalledPlugin>,
    #[serde(default)]
    known_ids: Vec<String>,
}

struct Inner {
    store: Store,
    /// The server data, canonical ([`crate::journal::server_key`]).
    key: PathBuf,
    node: Option<semver::Version>,
}

/// A server data folder's plugin store.
#[napi]
pub struct Registry {
    inner: Arc<Inner>,
    worker: Worker,
}

/// Runs `f` on `worker` and settles the promise with its value.
fn on_worker<'e, T: ToNapiValue + Send + 'static>(
    env: &'e Env,
    worker: &Worker,
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<Object<'e>> {
    let (deferred, promise) = env.create_deferred::<T, _>()?;
    worker.submit(Box::new(move || match guarded(f, || {}) {
        Ok(v) => deferred.resolve(move |_env| Ok(v)),
        Err(e) => deferred.reject(e),
    }))?;
    Ok(promise)
}

#[napi]
impl Registry {
    /// The registry of `server_data` (its `plugins/` directories are created).
    #[napi(factory, catch_unwind)]
    pub fn open(env: Env, server_data: String) -> Result<Registry> {
        let data = Path::new(&server_data);
        let store = Store::open(&data.join("plugins")).map_err(|e| reject(core(e)))?;
        let node = env
            .get_node_version()
            .ok()
            .map(|v| semver::Version::new(v.major.into(), v.minor.into(), v.patch.into()));
        Ok(Registry {
            inner: Arc::new(Inner {
                store,
                key: crate::journal::server_key(data),
                node,
            }),
            worker: Worker::spawn()?,
        })
    }

    /// Makes a plan (`PlanReq` JSON); resolves to the `InstallPlan` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn plan<'e>(&self, env: &'e Env, req_json: String) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let req: PlanReq =
                serde_json::from_str(&req_json).map_err(|e| bad_request("the plan request", e))?;
            let started = Instant::now();
            let out = plan(
                &inner,
                req.source,
                &PlanRequest {
                    id: req.id.clone(),
                    update: req.update,
                    allow_downgrade: req.allow_downgrade,
                    installed: req.installed,
                    known_ids: req.known_ids,
                },
            );
            match &out {
                Ok(p) => lifecycle(&inner.key, "plan", &About::of_plan(p), Ok(()), started),
                Err(e) => {
                    let about = About {
                        id: &req.id,
                        source: SourceKind::Registry,
                        kind: None,
                        risk: None,
                        permissions_added: 0,
                        bytes: 0,
                    };
                    lifecycle(&inner.key, "plan", &about, Err(e), started)
                }
            }
            encode(&out.map_err(|e| reject(core(e)))?)
        })
    }

    /// Installs the plan `plan_id` approved with `digest` (`Accept` JSON for the copyleft and
    /// third-party-code acceptances); resolves to the `InstalledPlugin` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn commit<'e>(
        &self,
        env: &'e Env,
        plan_id: String,
        digest: String,
        accept_json: Option<String>,
    ) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let accept: Accept = match accept_json {
                Some(s) => serde_json::from_str(&s).map_err(|e| bad_request("accept", e))?,
                None => Accept::default(),
            };
            let started = Instant::now();
            let staged = inner.store.plan(&plan_id).ok();
            let now = fin_registry::Clock::now_ms(&SystemClock);
            let out = inner.store.commit(&plan_id, &digest, &accept, now);
            if let Some(p) = &staged {
                let r = out.as_ref().map(|_| ());
                lifecycle(
                    &inner.key,
                    action_name(p.action),
                    &About::of_plan(p),
                    r,
                    started,
                );
            }
            encode(&out.map_err(|e| reject(core(e)))?)
        })
    }

    /// Deletes `id` `version` from the store, and its data with `purge_data`.
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn remove_plugin<'e>(
        &self,
        env: &'e Env,
        id: String,
        version: String,
        purge_data: bool,
    ) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let started = Instant::now();
            let out = remove(&inner, &id, &version, purge_data);
            let about = About {
                id: &id,
                source: SourceKind::Registry,
                kind: out.as_ref().ok().copied().flatten(),
                risk: None,
                permissions_added: 0,
                bytes: 0,
            };
            lifecycle(
                &inner.key,
                "remove",
                &about,
                out.as_ref().map(|_| ()),
                started,
            );
            out.map(|_| ()).map_err(|e| reject(core(e)))
        })
    }

    /// Deletes expired and broken plans; resolves to how many.
    #[napi(catch_unwind, ts_return_type = "Promise<number>")]
    pub fn sweep<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let now = fin_registry::Clock::now_ms(&SystemClock);
            Ok(inner.store.sweep_staging(now))
        })
    }
}

fn plan(
    inner: &Inner,
    source: SourceReq,
    req: &PlanRequest,
) -> std::result::Result<fin_registry::plan::InstallPlan, RegistryError> {
    let src = resolve(inner, source)?;
    let ctx = host::ctx(SourceClass::Registry, inner.node.clone());
    let plan_host = host::Host {
        store: &inner.store,
    };
    make_plan(src, &inner.store, req, &ctx, &plan_host, &SystemClock)
}

/// The package a source names.
#[cfg_attr(not(feature = "test-hooks"), allow(unused_variables))]
fn resolve(inner: &Inner, source: SourceReq) -> std::result::Result<PlanInput, RegistryError> {
    match source {
        SourceReq::Registry { registry, .. } => Err(RegistryError::new(
            "REGISTRY_NOT_FOUND",
            format!("no registry {registry:?} is configured"),
            json!({}),
        )
        .with_fix("add a local registry first")),
        #[cfg(feature = "test-hooks")]
        SourceReq::Package {
            archive,
            registry_id,
            stale_days,
            yanked,
        } => {
            let scratch = inner
                .store
                .staging_dir()
                .join(format!(".describe-{}", std::process::id()));
            let d = fin_registry::target::describe_package(Path::new(&archive), &scratch)?;
            let mut custom = d.custom;
            custom.yanked = yanked;
            Ok(PlanInput::Registry(Box::new(
                fin_registry::target::VerifiedTarget {
                    registry_id,
                    archive: archive.into(),
                    length: d.length,
                    sha256: d.sha256,
                    custom,
                    stale_days,
                    warnings: Vec::new(),
                },
            )))
        }
    }
}

/// Removes `id` `version`; returns the removed package's kind when its manifest was readable.
fn remove(
    inner: &Inner,
    id: &str,
    version: &str,
    purge: bool,
) -> std::result::Result<Option<fin_plugin_api::PluginKind>, RegistryError> {
    let not_installed = |reason: &str| {
        RegistryError::new(
            "PLUGIN_NOT_INSTALLED",
            format!("{id} {version} is not installed"),
            json!({ "reason": reason }),
        )
    };
    let pid = PluginId::parse(id).map_err(|_| not_installed("id"))?;
    let v = semver::Version::parse(version).map_err(|_| not_installed("version"))?;
    let kind = inner
        .store
        .installed_manifests()
        .into_iter()
        .find(|(_, m)| m.id == pid && m.version == v)
        .map(|(_, m)| m.kind);
    inner.store.remove(&pid, &v, purge)?;
    Ok(kind)
}

/// Validates the unpacked plugin in `dir` (`source`: `registry`, the default, `bundled` or
/// `dev`); resolves to the `ValidationReport` JSON. Offline: nothing is fetched or run.
#[napi(catch_unwind, ts_return_type = "Promise<string>")]
pub fn validate_manifest<'e>(
    env: &'e Env,
    dir: String,
    source: Option<String>,
) -> Result<Object<'e>> {
    let worker = Worker::spawn()?;
    let node = env
        .get_node_version()
        .ok()
        .map(|v| semver::Version::new(v.major.into(), v.minor.into(), v.patch.into()));
    on_worker(env, &worker, move || {
        let source = match source.as_deref() {
            None | Some("registry") => SourceClass::Registry,
            Some("bundled") => SourceClass::Bundled,
            Some("dev") => SourceClass::Dev,
            Some(other) => {
                return Err(reject(
                    CoreError::new("INVALID_OP", format!("unknown source {other:?}"))
                        .with_fix("use registry, bundled or dev"),
                ));
            }
        };
        let root = PathBuf::from(&dir);
        let report = match std::fs::read(root.join(MANIFEST_FILE)) {
            Err(_) => ValidationReport {
                ok: false,
                manifest: None,
                errors: vec![fin_plugin_api::Invalid {
                    code: "PLUGIN_MANIFEST_INVALID".into(),
                    message: format!("{dir}: no {MANIFEST_FILE}"),
                    details: json!({ "path": "", "reason": "missing" })
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                }],
            },
            Ok(bytes) => {
                let files = fin_registry::plan::DirFiles(&root);
                match validate(&bytes, &files, &host::ctx(source, node)) {
                    Ok(m) => ValidationReport {
                        ok: true,
                        manifest: Some(m),
                        errors: Vec::new(),
                    },
                    Err(errors) => ValidationReport {
                        ok: false,
                        manifest: None,
                        errors,
                    },
                }
            }
        };
        encode(&report)
    })
}
