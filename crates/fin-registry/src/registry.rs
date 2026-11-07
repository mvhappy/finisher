//! Local-folder registries (spec §5.1–5.2, §6.3; 8A Decisions 14, 16, 17).
//!
//! ```text
//! <base> = <server-data>/plugins
//!   registries/<id>/registry.json   {id, kind: local, path, trust: {rootSha256, freshness}}
//!   trust/<rootSha256>/             the root's TrustStore, kept when the registry is removed
//! ```
//!
//! A registry is added only with its root: the bytes of a root file, or its SHA-256 (typed by
//! the owner or confirmed after the console showed it). Either must match a root file of the
//! folder (`1.root.json`, or a newer root of its chain), so there is no trust on first use.
//! A git registry is a folder the user keeps current with `git pull`; nothing here runs git,
//! and a pull that rolls the repository back is refused by TUF like any other rollback.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fin_plugin_api::PluginId;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::advisory::{AdvisoryFile, SignedAdvisory};
use crate::atomic;
use crate::search::{self, Listing, PluginInfo, SearchQuery, VersionInfo};
use crate::transport::{FsTransport, Transport};
use crate::tuf::custom::ADVISORIES_FILE;
use crate::tuf::{
    Clock, Freshness, Policy, RefreshReport, TrustStore, TrustedRepo, TufError, sha256_hex,
};

pub type RegistryError = TufError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryConfig {
    /// `[a-z0-9][a-z0-9-]{0,63}`.
    pub id: String,
    /// The folder holding `metadata/` and `targets/`.
    pub path: PathBuf,
    pub root_sha256: String,
}

/// How the owner names the root a registry is trusted through (Decision 16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootSource {
    /// The bytes of a root file (`--root <file>`).
    File(Vec<u8>),
    /// `--root-sha256 <hex>`.
    Sha256(String),
    /// The hash the console showed ([`Registry::root_candidate`]) and the owner typed back.
    Confirmed(String),
}

/// What the console shows before the owner types a root's hash back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootCandidate {
    pub sha256: String,
    pub version: u64,
    pub root_key_ids: Vec<String>,
    pub threshold: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile {
    id: String,
    kind: String,
    path: PathBuf,
    trust: TrustFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustFile {
    root_sha256: String,
    freshness: String,
}

/// A local-folder registry and what it trusts.
#[derive(Debug)]
pub struct Registry {
    cfg: RegistryConfig,
    repo: TrustedRepo,
    transport: Arc<FsTransport>,
    advisories: Vec<SignedAdvisory>,
}

fn error(code: &'static str, reason: &str, message: impl Into<String>) -> TufError {
    TufError::new(code, message, json!({ "reason": reason }))
}

fn not_found(id: &str) -> TufError {
    TufError::new(
        "REGISTRY_NOT_FOUND",
        format!("no registry {id:?}"),
        json!({ "id": id }),
    )
}

fn id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

fn config_dir(base: &Path, id: &str) -> PathBuf {
    base.join("registries").join(id)
}

fn trust_base(base: &Path) -> PathBuf {
    base.join("trust")
}

/// The root files of the folder, oldest first: `(version, bytes)`.
fn folder_roots(path: &Path) -> Vec<(u64, Vec<u8>)> {
    let t = FsTransport::new(path);
    let mut out = Vec::new();
    for v in 1u64.. {
        match t.fetch(&format!("metadata/{v}.root.json"), 4 << 20) {
            Ok(b) => out.push((v, b)),
            Err(_) => break,
        }
    }
    out
}

impl Registry {
    /// The folder's `1.root.json` as the console shows it for confirmation.
    pub fn root_candidate(path: &Path) -> Result<RootCandidate, RegistryError> {
        let (version, bytes) = folder_roots(path).into_iter().next().ok_or_else(|| {
            error(
                "REGISTRY_INVALID",
                "noRoot",
                format!("{} has no metadata/1.root.json", path.display()),
            )
        })?;
        let root: tough::schema::Signed<tough::schema::Root> = serde_json::from_slice(&bytes)
            .map_err(|e| error("REGISTRY_INVALID", "parse", format!("1.root.json: {e}")))?;
        let keys = &root.signed.roles[&tough::schema::RoleType::Root];
        let mut root_key_ids: Vec<String> = keys
            .keyids
            .iter()
            .map(|k| hex::encode(k.as_ref()))
            .collect();
        root_key_ids.sort();
        Ok(RootCandidate {
            sha256: sha256_hex(&bytes),
            version,
            root_key_ids,
            threshold: keys.threshold.get(),
        })
    }

    /// Adds the registry `cfg` trusted through `root`. `cfg.root_sha256` must name the same
    /// root. Re-adding a root whose trust was kept resumes from its trusted versions.
    pub fn add(
        base: &Path,
        cfg: RegistryConfig,
        root: RootSource,
    ) -> Result<Registry, RegistryError> {
        if !id_ok(&cfg.id) {
            return Err(error(
                "REGISTRY_INVALID",
                "id",
                format!("{:?} is not a registry id", cfg.id),
            ));
        }
        if config_dir(base, &cfg.id).join("registry.json").exists() {
            return Err(TufError::new(
                "REGISTRY_EXISTS",
                format!("a registry {:?} exists", cfg.id),
                json!({ "id": cfg.id }),
            ));
        }
        let pinned = match &root {
            RootSource::File(bytes) => sha256_hex(bytes),
            RootSource::Sha256(h) | RootSource::Confirmed(h) => h.to_ascii_lowercase(),
        };
        let expect = cfg.root_sha256.to_ascii_lowercase();
        let untrusted = |got: &str| {
            TufError::new(
                "REGISTRY_UNTRUSTED",
                format!(
                    "the pinned root {expect} is not a root of {}",
                    cfg.path.display()
                ),
                json!({ "expected": expect, "got": got }),
            )
        };
        if pinned != expect {
            return Err(untrusted(&pinned));
        }
        let anchor = folder_roots(&cfg.path)
            .into_iter()
            .find(|(_, b)| sha256_hex(b) == expect)
            .ok_or_else(|| untrusted(""))?
            .1;
        let store = TrustStore::new(&trust_base(base), &expect)?;
        let repo = TrustedRepo::init(store, &anchor, &expect)?;
        let file = ConfigFile {
            id: cfg.id.clone(),
            kind: "local".into(),
            path: cfg.path.clone(),
            trust: TrustFile {
                root_sha256: expect.clone(),
                freshness: "relaxed".into(),
            },
        };
        let dir = config_dir(base, &cfg.id);
        std::fs::create_dir_all(&dir).map_err(|e| TufError::io("create", &dir, e))?;
        let p = dir.join("registry.json");
        let bytes = serde_json::to_vec_pretty(&file).expect("a config serialises");
        atomic::write(&p, &bytes).map_err(|e| TufError::io("write", &p, e))?;
        Ok(Registry::with_repo(cfg, repo))
    }

    fn with_repo(cfg: RegistryConfig, repo: TrustedRepo) -> Registry {
        let transport = Arc::new(FsTransport::new(&cfg.path));
        let mut r = Registry {
            cfg,
            repo,
            transport,
            advisories: Vec::new(),
        };
        // Advisories of the trusted set; a refresh fails loudly on a tampered file instead.
        r.advisories = r.load_advisories().unwrap_or_default();
        r
    }

    /// Opens the registry `id` added earlier.
    pub fn open(base: &Path, id: &str) -> Result<Registry, RegistryError> {
        let p = config_dir(base, id).join("registry.json");
        let bytes = std::fs::read(&p).map_err(|_| not_found(id))?;
        let file: ConfigFile = serde_json::from_slice(&bytes).map_err(|e| {
            error(
                "REGISTRY_INVALID",
                "config",
                format!("{}: {e}", p.display()),
            )
        })?;
        let cfg = RegistryConfig {
            id: file.id,
            path: file.path,
            root_sha256: file.trust.root_sha256,
        };
        let store = TrustStore::new(&trust_base(base), &cfg.root_sha256)?;
        Ok(Registry::with_repo(cfg, TrustedRepo::open(store)?))
    }

    /// The ids of the added registries.
    pub fn list(base: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(base.join("registries"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().join("registry.json").exists())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    /// Deletes the registry's config only: `trust/<rootSha256>/` stays, so removing and
    /// re-adding cannot roll it back.
    pub fn remove(base: &Path, id: &str) -> Result<(), RegistryError> {
        let dir = config_dir(base, id);
        if !dir.join("registry.json").exists() {
            return Err(not_found(id));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| TufError::io("remove", &dir, e))
    }

    /// Deletes the trusted metadata of a root no registry uses (`registry forget`).
    pub fn forget(base: &Path, root_sha256: &str) -> Result<(), RegistryError> {
        let sha = root_sha256.to_ascii_lowercase();
        for id in Registry::list(base) {
            let p = config_dir(base, &id).join("registry.json");
            let used = std::fs::read(&p)
                .ok()
                .and_then(|b| serde_json::from_slice::<ConfigFile>(&b).ok())
                .is_some_and(|f| f.trust.root_sha256 == sha);
            if used {
                return Err(TufError::new(
                    "REGISTRY_INVALID",
                    format!("registry {id:?} still trusts this root; remove it first"),
                    json!({ "reason": "inUse", "id": id }),
                ));
            }
        }
        TrustStore::new(&trust_base(base), &sha)?.forget()
    }

    pub fn config(&self) -> &RegistryConfig {
        &self.cfg
    }

    pub fn trusted(&self) -> &TrustedRepo {
        &self.repo
    }

    pub fn transport(&self) -> &FsTransport {
        &self.transport
    }

    fn policy() -> Policy {
        Policy {
            freshness: Freshness::Relaxed,
            ..Policy::default()
        }
    }

    /// Refreshes the trusted metadata from the folder (relaxed freshness), then reads the
    /// signed advisories.
    pub fn refresh(&mut self, clock: &dyn Clock) -> Result<RefreshReport, RegistryError> {
        let report = self
            .repo
            .refresh(self.transport.clone(), &Registry::policy(), clock)?;
        self.advisories = self.load_advisories()?;
        Ok(report)
    }

    fn load_advisories(&self) -> Result<Vec<SignedAdvisory>, RegistryError> {
        let mut out = Vec::new();
        let files: Vec<_> = self
            .repo
            .targets()
            .filter(|t| {
                t.name == ADVISORIES_FILE || t.name == format!("{}/{ADVISORIES_FILE}", t.role)
            })
            .collect();
        for t in files {
            let bytes = self
                .repo
                .fetch_target_bytes(self.transport.as_ref(), &t.name)?;
            let file: AdvisoryFile = serde_json::from_slice(&bytes)
                .map_err(|e| error("REGISTRY_INVALID", "advisories", format!("{}: {e}", t.name)))?;
            out.extend(file.advisories.into_iter().map(|a| SignedAdvisory {
                signed_by: t.role.clone(),
                advisory: a,
            }));
        }
        out.retain(SignedAdvisory::in_scope);
        Ok(out)
    }

    pub fn search(&self, q: &SearchQuery) -> Vec<Listing> {
        search::search(&self.cfg.id, &self.repo, q)
    }

    pub fn info(&self, id: &PluginId) -> Option<PluginInfo> {
        search::info(&self.cfg.id, &self.repo, &self.advisories, id)
    }

    /// The version to install: the newest not yanked, or exactly `version` (yanked ones
    /// too; the caller warns with `PLUGIN_YANKED`).
    pub fn resolve(&self, id: &PluginId, version: Option<&semver::Version>) -> Option<VersionInfo> {
        let versions = search::packages(&self.repo).remove(&id.to_string())?;
        match version {
            Some(want) => versions
                .into_iter()
                .find(|(v, _)| v == want)
                .map(|(_, i)| i),
            None => versions
                .into_iter()
                .rev()
                .find(|(_, i)| !i.yanked)
                .map(|(_, i)| i),
        }
    }

    /// Whole days past the trusted metadata's earliest expiry, or `None` while fresh.
    pub fn stale_days(&self, clock: &dyn Clock) -> Option<u16> {
        self.repo.stale_days(clock)
    }

    /// The in-scope advisories of the trusted set (registry-wide ones about any plugin,
    /// a publisher's about its own plugins only).
    pub fn advisories(&self) -> Vec<SignedAdvisory> {
        self.advisories.clone()
    }
}
