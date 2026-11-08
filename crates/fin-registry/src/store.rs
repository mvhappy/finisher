//! The plugin store under `<server-data>/plugins/` (spec §5.1):
//!
//! ```text
//! store/@<publisher>/<name>/<version>-<treeHash12>/   read-only after install
//! staging/<planId>/{plan.json, pkg/}                  verified, unpacked, not yet approved
//! data/@<publisher>/<name>/<projectId|_>/              the plugin's writable data
//! ```
//!
//! The store keeps no index: which version is installed, enabled and granted is `control.db`'s
//! (`finisherd`). A commit re-checks the plan's digest, expiry and acceptances, moves the
//! staged tree into the store, marks its files read-only and checks its tree hash there; on a
//! mismatch the tree is deleted (`PLAN_MISMATCH`). Nothing is executed.
//!
//! Every change is renames inside `store/@<publisher>/<name>/`, so an installed directory is
//! always whole: a commit moves its tree in as `.tmp-<planId>` and renames it into place (a
//! directory already there is re-hashed and, if it differs, moved aside as
//! `.tmp-old-<rand>`); a removal renames the directory to `.tmp-rm-<rand>` before deleting it.
//! `.tmp-*` directories are never installed ([`Store::installed_manifests`]) and
//! [`Store::sweep_store`] deletes those a crash or a failed deletion left behind. Commit,
//! remove and the sweeps of one `Store` (and its clones) run one at a time; one server owns
//! its store (the server lock).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use fin_plugin_api::{LicenceClass, MANIFEST_FILE, Manifest, PluginId, PluginKind};
use serde_json::json;

use crate::error::RegistryError;
use crate::package::tree_hash;
use crate::plan::{
    Accept, InstallPlan, InstalledPlugin, PLAN_TTL_MS, SourceKind, is_plan_id, plan_digest,
};

/// A server's plugin directories.
#[derive(Debug, Clone)]
pub struct Store {
    base: PathBuf,
    /// Held by commit, remove and the sweeps.
    busy: Arc<Mutex<()>>,
}

/// A fresh suffix for a temporary name.
fn rand_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = blake3::Hasher::new();
    h.update(&COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    h.update(&std::process::id().to_le_bytes());
    let wall = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.update(&wall.to_le_bytes());
    h.finalize().to_hex()[..16].to_string()
}

fn mkdirs(p: &Path) -> Result<(), RegistryError> {
    fs::create_dir_all(p).map_err(|e| RegistryError::io("mkdir", p, e))
}

fn plan_missing(plan_id: &str) -> RegistryError {
    RegistryError::new(
        "PLAN_EXPIRED",
        format!("no plan {plan_id:?}: it expired or was never made"),
        json!({ "reason": "missing" }),
    )
    .with_fix("make a new plan")
}

impl Store {
    /// The store at `base` (`<server-data>/plugins`), creating its directories.
    pub fn open(base: &Path) -> Result<Store, RegistryError> {
        let store = Store {
            base: base.to_path_buf(),
            busy: Arc::new(Mutex::new(())),
        };
        for d in [store.store_dir(), store.staging_dir(), store.data_root()] {
            mkdirs(&d)?;
        }
        Ok(store)
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.base.join("staging")
    }

    fn store_dir(&self) -> PathBuf {
        self.base.join("store")
    }

    fn data_root(&self) -> PathBuf {
        self.base.join("data")
    }

    /// `store/@<publisher>/<name>/`.
    fn plugin_root(&self, id: &PluginId) -> PathBuf {
        self.store_dir()
            .join(format!("@{}", id.publisher()))
            .join(id.name())
    }

    /// `data/@<publisher>/<name>/`.
    pub fn data_dir(&self, id: &PluginId) -> PathBuf {
        self.data_root()
            .join(format!("@{}", id.publisher()))
            .join(id.name())
    }

    /// The plan `plan_id` as staged.
    pub fn plan(&self, plan_id: &str) -> Result<InstallPlan, RegistryError> {
        if !is_plan_id(plan_id) {
            return Err(plan_missing(plan_id));
        }
        let path = self.staging_dir().join(plan_id).join("plan.json");
        let bytes = fs::read(&path).map_err(|_| plan_missing(plan_id))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| RegistryError::plan_mismatch("plan", "the staged plan does not decode"))
    }

    /// Installs the plan `plan_id` approved with `digest` and `accept` at `now` (Unix ms).
    pub fn commit(
        &self,
        plan_id: &str,
        digest: &str,
        accept: &Accept,
        now: u64,
    ) -> Result<InstalledPlugin, RegistryError> {
        let _busy = self.lock();
        let plan = self.plan(plan_id)?;
        let staged = self.staging_dir().join(plan_id);
        if plan_digest(&plan) != plan.digest || plan.plan_id != plan_id {
            let _ = fs::remove_dir_all(&staged);
            return Err(RegistryError::plan_mismatch(
                "plan",
                "the staged plan was changed",
            ));
        }
        if digest != plan.digest {
            return Err(RegistryError::plan_mismatch(
                "digest",
                "the digest is not this plan's",
            ));
        }
        if now >= plan.expires_at {
            let _ = fs::remove_dir_all(&staged);
            return Err(RegistryError::new(
                "PLAN_EXPIRED",
                format!("plan {plan_id} expired"),
                json!({ "reason": "expired", "expiresAt": plan.expires_at }),
            )
            .with_fix("make a new plan"));
        }
        check_accept(&plan, accept)?;
        let id = PluginId::parse(&plan.plugin.id).map_err(|_| {
            RegistryError::plan_mismatch("plan", "the staged plan names no plugin id")
        })?;
        // Both name the store directory: parsed, never used raw.
        let version = semver::Version::parse(&plan.plugin.version).ok();
        let hex =
            plan.tree_hash.len() == 64 && plan.tree_hash.bytes().all(|b| b.is_ascii_hexdigit());
        let Some(version) = version.filter(|_| hex) else {
            let _ = fs::remove_dir_all(&staged);
            return Err(RegistryError::plan_mismatch(
                "plan",
                "the staged plan is malformed",
            ));
        };
        let root = self.plugin_root(&id);
        mkdirs(&root)?;
        let name = format!("{version}-{}", &plan.tree_hash[..12]);
        let dest = root.join(&name);
        let moving = root.join(format!(".tmp-{plan_id}"));
        let pkg = staged.join("pkg");
        fs::rename(&pkg, &moving).map_err(|e| RegistryError::io("rename", &pkg, e))?;
        let _ = fs::remove_dir_all(&staged);
        set_read_only(&moving, true)?;
        let have = tree_hash(&moving);
        if have.as_ref().ok() != Some(&plan.tree_hash) {
            remove_tree(&moving);
            return Err(RegistryError::plan_mismatch(
                "tree",
                "the staged files changed after the plan was made",
            ));
        }
        self.place(&moving, &dest, &plan.tree_hash)?;
        Ok(InstalledPlugin {
            id: plan.plugin.id.clone(),
            version: plan.plugin.version.clone(),
            kind: plan.plugin.kind,
            source: plan.source.kind,
            registry_id: plan.source.registry_id.clone(),
            permissions: plan
                .permissions
                .iter()
                .map(|p| p.permission.clone())
                .collect(),
            tree_hash: plan.tree_hash.clone(),
            target_sha256: plan.target_sha256.clone(),
            dir: format!("store/@{}/{}/{name}", id.publisher(), id.name()),
        })
    }

    /// Deletes every installed copy of `id` `version`; its data too with `purge_data`.
    /// `PLUGIN_NOT_INSTALLED` when the store has none.
    pub fn remove(
        &self,
        id: &PluginId,
        version: &semver::Version,
        purge_data: bool,
    ) -> Result<(), RegistryError> {
        let _busy = self.lock();
        let root = self.plugin_root(id);
        let prefix = format!("{version}-");
        let found: Vec<PathBuf> = fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(&prefix) && n.len() == prefix.len() + 12)
            })
            .map(|e| e.path())
            .collect();
        if found.is_empty() {
            return Err(RegistryError::new(
                "PLUGIN_NOT_INSTALLED",
                format!("{id} {version} is not in the store"),
                json!({}),
            ));
        }
        for dir in &found {
            // Hidden first, so a deletion that fails half way leaves nothing that looks
            // installed; the sweep finishes it.
            let hidden = root.join(format!(".tmp-rm-{}", rand_suffix()));
            fs::rename(dir, &hidden).map_err(|e| RegistryError::io("rename", dir, e))?;
            remove_tree(&hidden);
        }
        if purge_data {
            let data = self.data_dir(id);
            if data.exists() {
                fs::remove_dir_all(&data).map_err(|e| RegistryError::io("remove", &data, e))?;
            }
        }
        // Drop the now-empty name and publisher directories.
        if fs::remove_dir(&root).is_ok()
            && let Some(publisher) = root.parent()
        {
            let _ = fs::remove_dir(publisher);
        }
        Ok(())
    }

    /// Deletes expired or unreadable plans (and half-built ones older than a plan's life);
    /// returns how many.
    pub fn sweep_staging(&self, now: u64) -> u32 {
        let _busy = self.lock();
        let mut n = 0;
        let Ok(entries) = fs::read_dir(self.staging_dir()) else {
            return 0;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let gone = if name.starts_with(".tmp-") {
                let age = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok());
                age.is_some_and(|a| a > Duration::from_millis(PLAN_TTL_MS))
            } else {
                self.plan(&name).map_or(true, |p| now >= p.expires_at)
            };
            if gone && fs::remove_dir_all(&path).is_ok() {
                n += 1;
            }
        }
        n
    }

    /// Deletes every `.tmp-*` directory under `store/@<publisher>/<name>/` (a removal that
    /// failed half way, a tree moved aside, a commit interrupted between its renames) and the
    /// name and publisher directories left empty; returns how many temporary directories.
    pub fn sweep_store(&self) -> u32 {
        let _busy = self.lock();
        let mut n = 0;
        for publisher in subdirs(&self.store_dir()) {
            for name in subdirs(&publisher) {
                for dir in subdirs(&name) {
                    let temp = dir
                        .file_name()
                        .is_some_and(|f| f.to_string_lossy().starts_with(".tmp-"));
                    if temp {
                        remove_tree(&dir);
                        n += u32::from(!dir.exists());
                    }
                }
                let _ = fs::remove_dir(&name);
            }
            let _ = fs::remove_dir(&publisher);
        }
        n
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.busy.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Puts the verified tree `moving` at `dest`. A directory already at `dest` stays when its
    /// tree hash is `tree_hash`; otherwise (a file deleted or changed after install) it is
    /// moved aside and replaced.
    fn place(&self, moving: &Path, dest: &Path, tree_hash: &str) -> Result<(), RegistryError> {
        if dest.exists() {
            if tree_hash_of(dest).as_deref() == Some(tree_hash) {
                remove_tree(moving);
                return Ok(());
            }
            let root = dest.parent().unwrap_or(dest);
            let aside = root.join(format!(".tmp-old-{}", rand_suffix()));
            fs::rename(dest, &aside).map_err(|e| RegistryError::io("rename", dest, e))?;
            if let Err(e) = fs::rename(moving, dest) {
                let _ = fs::rename(&aside, dest);
                return Err(RegistryError::io("rename", moving, e));
            }
            remove_tree(&aside);
            return Ok(());
        }
        fs::rename(moving, dest).map_err(|e| RegistryError::io("rename", moving, e))
    }

    /// Every installed package directory with its manifest (for the catalog-id check).
    pub fn installed_manifests(&self) -> Vec<(PathBuf, Manifest)> {
        let mut out = Vec::new();
        let level = |p: &Path| -> Vec<PathBuf> {
            subdirs(p)
                .into_iter()
                .filter(|d| {
                    d.file_name()
                        .is_some_and(|f| !f.to_string_lossy().starts_with(".tmp-"))
                })
                .collect()
        };
        for publisher in level(&self.store_dir()) {
            for name in level(&publisher) {
                for dir in level(&name) {
                    let m = fs::read(dir.join(MANIFEST_FILE))
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok());
                    if let Some(m) = m {
                        out.push((dir, m));
                    }
                }
            }
        }
        out
    }
}

fn check_accept(plan: &InstallPlan, accept: &Accept) -> Result<(), RegistryError> {
    if plan.licence.class == LicenceClass::Copyleft && !accept.copyleft {
        return Err(RegistryError::new(
            "COPYLEFT_NOT_ACCEPTED",
            format!(
                "{} is under {}, a copyleft licence",
                plan.plugin.id, plan.licence.spdx
            ),
            json!({ "spdx": plan.licence.spdx }),
        )
        .with_fix("accept the copyleft licence"));
    }
    let third_party =
        plan.plugin.kind == PluginKind::Code && plan.source.kind == SourceKind::Registry;
    if third_party && !accept.third_party_code {
        return Err(RegistryError::new(
            "THIRD_PARTY_CODE_NOT_ACCEPTED",
            format!("{} is third-party code and runs unconfined", plan.plugin.id),
            json!({ "confined": false }),
        )
        .with_fix("accept third-party code after reading the warning"));
    }
    Ok(())
}

/// Files read-only (or writable again) under `root`; directories keep their modes so the tree
/// can be renamed and removed.
fn set_read_only(root: &Path, on: bool) -> Result<(), RegistryError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| RegistryError::io("read", &dir, e))?;
        for e in entries {
            let e = e.map_err(|e| RegistryError::io("read", &dir, e))?;
            let path = e.path();
            let ft = e
                .file_type()
                .map_err(|err| RegistryError::io("stat", &path, err))?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                read_only_file(&path, on)?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_only_file(path: &Path, on: bool) -> Result<(), RegistryError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path).map_err(|e| RegistryError::io("stat", path, e))?;
    let exec = meta.permissions().mode() & 0o100 != 0;
    let mode = match (on, exec) {
        (true, true) => 0o555,
        (true, false) => 0o444,
        (false, true) => 0o755,
        (false, false) => 0o644,
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| RegistryError::io("chmod", path, e))
}

#[cfg(not(unix))]
fn read_only_file(path: &Path, on: bool) -> Result<(), RegistryError> {
    let mut perms = fs::metadata(path)
        .map_err(|e| RegistryError::io("stat", path, e))?
        .permissions();
    perms.set_readonly(on);
    fs::set_permissions(path, perms).map_err(|e| RegistryError::io("chmod", path, e))
}

/// The directories directly under `p`.
fn subdirs(p: &Path) -> Vec<PathBuf> {
    fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

fn tree_hash_of(p: &Path) -> Option<String> {
    tree_hash(p).ok()
}

fn remove_tree(p: &Path) {
    let _ = set_read_only(p, false);
    let _ = fs::remove_dir_all(p);
}
