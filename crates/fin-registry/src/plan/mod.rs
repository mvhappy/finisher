//! Install plans (spec §5.3 step 1, §5.4; 8A Decision 18): a source is staged, verified,
//! unpacked strictly and validated into `staging/<planId>/`, and an [`InstallPlan`] says what
//! approving it grants. Nothing in a package runs, here or at commit.
//!
//! A plan is built in `staging/.tmp-<planId>/` and renamed to `staging/<planId>/` once its
//! `plan.json` is written, so the sweeper never sees half a plan. Plans live 10 minutes
//! ([`PLAN_TTL_MS`]).

mod checks;
mod stage;
mod types;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fin_plugin_api::{
    Manifest, PackageFiles, PluginId, PluginKind, Risk, ValidateCtx, classify, diff, overall,
    risks, validate,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clock::Clock;
use crate::error::RegistryError;
use crate::store::Store;
use crate::target::VerifiedTarget;

pub use checks::lookalike;
pub use types::{
    Accept, InstallPlan, InstalledPlugin, PlanAction, PlanLicence, PlanModel, PlanPlugin,
    PlanSource, PlanWarning, PlannedPermission, SourceKind, ValidationReport,
};

/// How long a plan can be approved.
pub const PLAN_TTL_MS: u64 = 10 * 60 * 1000;

/// What to plan from.
#[derive(Debug, Clone)]
pub enum PlanInput {
    /// A registry target the TUF client verified.
    Registry(Box<VerifiedTarget>),
    /// A bundled plugin's unpacked tree and its compiled-in tree hash (Task 10).
    Bundled { dir: PathBuf, tree_hash: String },
}

/// The caller's side of a plan (`finisherd`, through `fin-node`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanRequest {
    /// The plugin asked for; the manifest must carry it.
    pub id: String,
    /// The exact version asked for, when a person named one; the manifest must carry it, and
    /// a downgrade needs it.
    #[serde(default)]
    pub version: Option<semver::Version>,
    /// An update of the installed version (`installed` must be set).
    #[serde(default)]
    pub update: bool,
    /// A person named an older version (spec §5.3 step 4); needs `version`.
    #[serde(default)]
    pub allow_downgrade: bool,
    /// The installed version and its grants, from `control.db`.
    #[serde(default)]
    pub installed: Option<InstalledPlugin>,
    /// Other plugin ids people may confuse this one with (installed, bundled, listed).
    #[serde(default)]
    pub known_ids: Vec<String>,
}

/// What a plan needs from crates above this one; `fin-node` implements it over `fin-fonts`.
pub trait PlanHost {
    /// The entry ids of a contributed font catalog (already validated).
    fn font_catalog_ids(&self, bytes: &[u8]) -> Vec<String>;
    /// Entry ids the built-in catalog and other installed packs (not `except`) hold.
    fn taken_font_ids(&self, except: &PluginId) -> BTreeSet<String>;
}

/// Makes a plan for `req` from `src` in `store`'s staging directory.
pub fn make_plan(
    src: PlanInput,
    store: &Store,
    req: &PlanRequest,
    ctx: &ValidateCtx,
    host: &dyn PlanHost,
    clock: &dyn Clock,
) -> Result<InstallPlan, RegistryError> {
    let now = clock.now_ms();
    let id = PluginId::parse(&req.id).map_err(|e| {
        RegistryError::new(
            "PLUGIN_NOT_FOUND",
            format!("{:?} is not a plugin id: {}", req.id, e.reason()),
            json!({ "reason": e.reason() }),
        )
    })?;
    let plan_id = new_plan_id(now, &req.id);
    let tmp = store.staging_dir().join(format!(".tmp-{plan_id}"));
    let built = build(&src, &tmp, &id, req, ctx, host, now, &plan_id);
    let plan = match built {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
    };
    let dest = store.staging_dir().join(&plan_id);
    std::fs::rename(&tmp, &dest).map_err(|e| {
        let _ = std::fs::remove_dir_all(&tmp);
        RegistryError::io("rename", &dest, e)
    })?;
    Ok(plan)
}

#[allow(clippy::too_many_arguments)]
fn build(
    src: &PlanInput,
    tmp: &Path,
    id: &PluginId,
    req: &PlanRequest,
    ctx: &ValidateCtx,
    host: &dyn PlanHost,
    now: u64,
    plan_id: &str,
) -> Result<InstallPlan, RegistryError> {
    std::fs::create_dir(tmp).map_err(|e| RegistryError::io("mkdir", tmp, e))?;
    let pkg = tmp.join("pkg");
    let unpacked = match src {
        PlanInput::Registry(t) => stage::registry(t, tmp, &pkg)?,
        PlanInput::Bundled { dir, tree_hash } => stage::bundled(dir, tree_hash, tmp, &pkg)?,
    };
    let files = DirFiles(&pkg);
    let manifest = validate(&unpacked.manifest, &files, ctx).map_err(checks::invalid)?;
    if manifest.id != *id {
        return Err(checks::mismatch(
            "/id",
            "idMismatch",
            format!("the package is {}, not {}", manifest.id, id),
        ));
    }
    let bytes: u64 = unpacked.files.values().map(|f| f.size).sum();
    let (source, stale, mut registry_warnings, target_sha256) = match src {
        PlanInput::Registry(t) => {
            checks::custom_matches(&t.custom, &manifest, &unpacked.manifest, bytes)?;
            let source = PlanSource {
                kind: SourceKind::Registry,
                registry_id: Some(t.registry_id.clone()),
            };
            let mut w = t.warnings.clone();
            if t.custom.yanked {
                w.insert(0, PlanWarning::new("yanked", json!({})));
            }
            (source, t.stale_days, w, Some(t.sha256.clone()))
        }
        PlanInput::Bundled { .. } => {
            let source = PlanSource {
                kind: SourceKind::Bundled,
                registry_id: None,
            };
            (source, None, Vec::new(), None)
        }
    };
    checks::catalog_ids(&manifest, &files, id, host)?;
    let (action, from, old_perms) = checks::action(&manifest, req)?;

    let third_party_code = manifest.kind == PluginKind::Code && source.kind == SourceKind::Registry;
    let mut risk = overall(&manifest.permissions, manifest.agent_facing());
    if third_party_code {
        risk = Risk::High;
    }
    let licence = PlanLicence {
        spdx: manifest.license.clone(),
        class: classify(&manifest.license),
    };
    let mut warnings = Vec::new();
    if third_party_code {
        // No `plugin` sandbox profile exists before 7C (spec §3.1).
        warnings.push(PlanWarning::new(
            "thirdPartyCode",
            json!({ "confined": false }),
        ));
    }
    warnings.append(&mut registry_warnings);
    warnings.extend(checks::licence_warning(&licence));
    warnings.extend(
        lookalike(&req.id, &req.known_ids)
            .map(|similar| PlanWarning::new("lookalikeId", json!({ "similar": similar }))),
    );
    if let Some(days) = stale.filter(|d| *d > 0) {
        warnings.push(PlanWarning::new("registryStale", json!({ "days": days })));
    }
    let mut plan = InstallPlan {
        plan_id: plan_id.to_string(),
        digest: String::new(),
        plugin: PlanPlugin {
            id: manifest.id.to_string(),
            version: manifest.version.to_string(),
            kind: manifest.kind,
            display_name: manifest.id.name().to_string(),
            publisher: manifest.publisher.name.clone(),
        },
        source,
        action,
        from,
        permissions: risks(&manifest.permissions)
            .into_iter()
            .map(|(permission, risk)| PlannedPermission { permission, risk })
            .collect(),
        diff: diff(&old_perms, &manifest.permissions),
        risk,
        licence,
        bytes,
        models: models(&manifest),
        warnings,
        registry_stale_days: stale,
        tree_hash: unpacked.tree_hash.clone(),
        target_sha256,
        expires_at: now.saturating_add(PLAN_TTL_MS),
    };
    plan.digest = plan_digest(&plan);
    let text = serde_json::to_vec_pretty(&plan).map_err(|e| {
        RegistryError::new(
            "INTERNAL",
            format!("cannot encode the plan: {e}"),
            json!({}),
        )
    })?;
    let path = tmp.join("plan.json");
    std::fs::write(&path, text).map_err(|e| RegistryError::io("write", &path, e))?;
    Ok(plan)
}

fn models(m: &Manifest) -> Vec<PlanModel> {
    m.models
        .iter()
        .map(|x| PlanModel {
            id: x.id.clone(),
            bytes: x.bytes,
            licence: x.license.clone(),
            attribution: x.attribution.clone(),
        })
        .collect()
}

/// `p` and 24 hex characters: unique per process and call.
fn new_plan_id(now: u64, id: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h = blake3::Hasher::new();
    h.update(&now.to_le_bytes());
    h.update(&n.to_le_bytes());
    h.update(&std::process::id().to_le_bytes());
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.update(&wall.to_le_bytes());
    h.update(id.as_bytes());
    format!("p{}", &h.finalize().to_hex()[..24])
}

/// Whether `s` has the shape of a plan id (it names a staging directory).
pub fn is_plan_id(s: &str) -> bool {
    s.len() == 25
        && s.starts_with('p')
        && s[1..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// BLAKE3 (hex) of the plan's canonical JSON with `digest` empty.
pub fn plan_digest(plan: &InstallPlan) -> String {
    let mut p = plan.clone();
    p.digest = String::new();
    let v = serde_json::to_value(&p).unwrap_or(Value::Null);
    blake3::hash(canonical_json(&v).as_bytes())
        .to_hex()
        .to_string()
}

/// JSON with object keys sorted at every level and no whitespace.
pub fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical_json(&m[k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

/// An unpacked package on disk as [`PackageFiles`]; only package paths are read.
pub struct DirFiles<'a>(pub &'a Path);

impl PackageFiles for DirFiles<'_> {
    fn exists(&self, path: &str) -> bool {
        fin_plugin_api::validate::is_package_path(path) && self.0.join(path).is_file()
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if !self.exists(path) {
            return None;
        }
        std::fs::read(self.0.join(path)).ok()
    }
}
