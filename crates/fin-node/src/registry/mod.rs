//! The `Registry` class (8A plan Task 8): plugin plans, commits, removals and the staging
//! sweep over `<server-data>/plugins` ([`fin_registry::Store`]), and `validateManifest`.
//!
//! Every call runs on the registry's own worker thread, one at a time in call order, and
//! settles a promise: a plan copies, hashes and unpacks a package, which may take seconds,
//! and never holds the JS thread or the libuv pool. Each call journals `plugin.lifecycle`
//! ([`log`]). Nothing a package ships runs.
//!
//! Sources: `registry` (an added local-folder registry: the version is resolved in its trusted
//! metadata, fetched with its signed length as the cap and checked against its signed SHA-256,
//! 8A Task 9b); `bundled` (a tree of the app's resources, checked against the hash compiled
//! into this binary, never one the caller names, 8A Task 10; see [`bundled`]) and, in
//! `test-hooks` builds only, `package` (a `.finpkg` file described as a
//! publisher would sign it, standing in for a verified target).

mod bundled;
mod content;
mod host;
mod lifecycle;
mod log;
mod registries;

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
        /// The exact version a person named (`PlanRequest::version`).
        #[serde(default)]
        version: Option<semver::Version>,
    },
    /// The compiled-in plugin `id` from `root/<name>` (the resources folder).
    Bundled { root: String },
    #[cfg(feature = "test-hooks")]
    #[serde(rename_all = "camelCase")]
    Package {
        archive: String,
        registry_id: String,
        #[serde(default)]
        version: Option<semver::Version>,
        #[serde(default)]
        stale_days: Option<u16>,
        #[serde(default)]
        yanked: bool,
    },
}

impl SourceReq {
    fn version(&self) -> Option<semver::Version> {
        match self {
            SourceReq::Registry { version, .. } => version.clone(),
            SourceReq::Bundled { .. } => None,
            #[cfg(feature = "test-hooks")]
            SourceReq::Package { version, .. } => version.clone(),
        }
    }
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
    #[serde(default)]
    registry_changed: bool,
    /// Mark the plan agent-facing (`PlanRequest::test_agent_facing`).
    #[cfg(feature = "test-hooks")]
    #[serde(default)]
    test_agent_facing: bool,
}

/// `Registry.commit`'s re-checks against `control.db` (8A Task 9b, review M2).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommitChecks {
    /// The package directories of the installed plugins: their font catalogs' ids are taken.
    #[serde(default)]
    installed_dirs: Option<Vec<String>>,
}

struct Inner {
    store: Store,
    /// `<server-data>/plugins`: the registries' configs and trusted metadata.
    base: PathBuf,
    /// The server data, canonical ([`crate::journal::server_key`]).
    key: PathBuf,
    node: Option<semver::Version>,
    /// Test builds only: stands in for the compiled-in list ([`bundled::Entries`]).
    #[cfg(feature = "test-hooks")]
    bundled_override: std::sync::Mutex<Option<Vec<crate::bundled::Entry>>>,
}

impl Inner {
    /// The bundled plugins this binary carries.
    fn bundled_entries(&self) -> Vec<crate::bundled::Entry> {
        #[cfg(feature = "test-hooks")]
        if let Some(list) = self
            .bundled_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return list;
        }
        crate::bundled::compiled()
    }
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
                base: data.join("plugins"),
                key: crate::journal::server_key(data),
                node,
                #[cfg(feature = "test-hooks")]
                bundled_override: std::sync::Mutex::new(None),
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
            let version = match &req.source {
                // A bundled plugin installs the version compiled in, nothing else.
                SourceReq::Bundled { .. } => inner
                    .bundled_entries()
                    .iter()
                    .find(|e| e.id == req.id)
                    .and_then(|e| semver::Version::parse(&e.version).ok()),
                other => other.version(),
            };
            let source_kind = match &req.source {
                SourceReq::Bundled { .. } => SourceKind::Bundled,
                _ => SourceKind::Registry,
            };
            let out = plan(
                &inner,
                req.source,
                &PlanRequest {
                    id: req.id.clone(),
                    version,
                    update: req.update,
                    allow_downgrade: req.allow_downgrade,
                    installed: req.installed,
                    known_ids: req.known_ids,
                    registry_changed: req.registry_changed,
                    #[cfg(feature = "test-hooks")]
                    test_agent_facing: req.test_agent_facing,
                    #[cfg(not(feature = "test-hooks"))]
                    test_agent_facing: false,
                },
            );
            match &out {
                Ok(p) => lifecycle(&inner.key, "plan", &About::of_plan(p), Ok(()), started),
                Err(e) => {
                    let about = About {
                        id: Some(&req.id),
                        source: source_kind,
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
    /// third-party-code acceptances; `CommitChecks` JSON for the re-checks against
    /// `control.db`); resolves to the `InstalledPlugin` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn commit<'e>(
        &self,
        env: &'e Env,
        plan_id: String,
        digest: String,
        accept_json: Option<String>,
        checks_json: Option<String>,
    ) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let accept: Accept = match accept_json {
                Some(s) => serde_json::from_str(&s).map_err(|e| bad_request("accept", e))?,
                None => Accept::default(),
            };
            let checks: CommitChecks = match checks_json {
                Some(s) => serde_json::from_str(&s).map_err(|e| bad_request("checks", e))?,
                None => CommitChecks::default(),
            };
            let started = Instant::now();
            let staged = inner.store.plan(&plan_id).ok();
            let now = fin_registry::Clock::now_ms(&SystemClock);
            let out = recheck(&inner, &plan_id, &checks)
                .and_then(|()| inner.store.commit(&plan_id, &digest, &accept, now));
            let r = out.as_ref().map(|_| ());
            match &staged {
                Some(p) => {
                    let about = About::of_plan(p);
                    lifecycle(&inner.key, action_name(p.action), &about, r, started);
                }
                // Swept, never made or not a plan id: which plugin is unknown.
                None => {
                    let about = About {
                        id: None,
                        source: SourceKind::Registry,
                        kind: None,
                        risk: None,
                        permissions_added: 0,
                        bytes: 0,
                    };
                    lifecycle(&inner.key, "install", &about, r, started);
                }
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
                id: Some(&id),
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

    /// Deletes expired and broken plans and the store's leftover temporary directories;
    /// resolves to how many.
    #[napi(catch_unwind, ts_return_type = "Promise<number>")]
    pub fn sweep<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let now = fin_registry::Clock::now_ms(&SystemClock);
            Ok(inner.store.sweep_staging(now) + inner.store.sweep_store())
        })
    }
}

/// `PLAN_MISMATCH {reason: installed, field: catalogIds}` when the staged pack's font ids are
/// taken by now (another pack installed since the plan was made).
fn recheck(
    inner: &Inner,
    plan_id: &str,
    checks: &CommitChecks,
) -> std::result::Result<(), RegistryError> {
    let Some(dirs) = &checks.installed_dirs else {
        return Ok(());
    };
    let clash = host::staged_catalog_clash(&inner.store, plan_id, dirs);
    if clash.is_empty() {
        return Ok(());
    }
    Err(RegistryError::new(
        "PLAN_MISMATCH",
        format!(
            "font catalog id {:?} was taken since the plan was made",
            clash[0]
        ),
        json!({ "reason": "installed", "field": "catalogIds", "entry": clash[0] }),
    )
    .with_fix("make a new plan"))
}

fn plan(
    inner: &Inner,
    source: SourceReq,
    req: &PlanRequest,
) -> std::result::Result<fin_registry::plan::InstallPlan, RegistryError> {
    let class = match source {
        SourceReq::Bundled { .. } => SourceClass::Bundled,
        _ => SourceClass::Registry,
    };
    let (src, fetched) = resolve(inner, source, req)?;
    let ctx = host::ctx(class, inner.node.clone());
    let plan_host = host::Host {
        store: &inner.store,
    };
    let out = make_plan(src, &inner.store, req, &ctx, &plan_host, &SystemClock);
    if let Some(f) = fetched {
        let _ = std::fs::remove_file(f);
    }
    out
}

/// The package a source names, and the file fetched for it (removed after the plan).
#[cfg_attr(not(feature = "test-hooks"), allow(unused_variables))]
fn resolve(
    inner: &Inner,
    source: SourceReq,
    req: &PlanRequest,
) -> std::result::Result<(PlanInput, Option<PathBuf>), RegistryError> {
    match source {
        SourceReq::Registry { registry, version } => {
            let reg = fin_registry::registry::Registry::open(&inner.base, &registry)?;
            let id = PluginId::parse(&req.id).map_err(|e| {
                RegistryError::new(
                    "PLUGIN_NOT_FOUND",
                    format!("{:?} is not a plugin id: {}", req.id, e.reason()),
                    json!({ "reason": e.reason() }),
                )
            })?;
            let out = inner.store.fetch_path();
            match reg.verified_target(&id, version.as_ref(), &out, &SystemClock) {
                Ok(t) => Ok((PlanInput::Registry(Box::new(t)), Some(out))),
                Err(e) => {
                    let _ = std::fs::remove_file(&out);
                    Err(e)
                }
            }
        }
        SourceReq::Bundled { root } => {
            let entry = inner
                .bundled_entries()
                .into_iter()
                .find(|e| e.id == req.id)
                .ok_or_else(|| {
                    RegistryError::new(
                        "PLUGIN_NOT_FOUND",
                        format!("{} is not a bundled plugin", req.id),
                        json!({ "reason": "notBundled" }),
                    )
                })?;
            let dir = Path::new(&root).join(entry.folder());
            Ok((
                PlanInput::Bundled {
                    dir,
                    tree_hash: entry.tree_hash,
                },
                None,
            ))
        }
        #[cfg(feature = "test-hooks")]
        SourceReq::Package {
            archive,
            registry_id,
            stale_days,
            yanked,
            ..
        } => {
            let scratch = inner
                .store
                .staging_dir()
                .join(format!(".describe-{}", std::process::id()));
            let d = fin_registry::target::describe_package(Path::new(&archive), &scratch)?;
            let mut custom = d.custom;
            custom.yanked = yanked;
            Ok((
                PlanInput::Registry(Box::new(fin_registry::target::VerifiedTarget {
                    registry_id,
                    archive: archive.into(),
                    length: d.length,
                    sha256: d.sha256,
                    custom,
                    stale_days,
                    warnings: Vec::new(),
                })),
                None,
            ))
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
#[allow(clippy::disallowed_methods)] // a plugin source directory, never a library path
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
