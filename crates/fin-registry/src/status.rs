//! What a registry reports about itself (`registry.list`, `registry.status`): its config, the
//! versions of the metadata it trusts, how many plugins it lists and how stale it is. Read
//! from the trusted set on disk only; nothing is fetched.

use schemars::JsonSchema;
use serde::Serialize;

use crate::registry::Registry;
use crate::search;
use crate::tuf::Clock;

/// The versions of the trusted metadata roles; a role not yet fetched has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrustedVersions {
    pub root: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub snapshot: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub targets: Option<u64>,
}

/// One added registry (spec §5.1: `registry.json` and the trusted set).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryStatus {
    pub id: String,
    /// `local`: a folder (a git registry is a folder the user pulls).
    pub kind: String,
    pub path: String,
    /// The root this registry is trusted through (also the name of its trust directory).
    pub root_sha256: String,
    /// `relaxed` for local folders.
    pub freshness: String,
    pub trusted: TrustedVersions,
    /// Plugins listed (with at least one version), yanked-only ones included.
    pub plugins: u32,
    /// Advisories in scope of the trusted set.
    pub advisories: u32,
    /// Whole days past the trusted metadata's earliest expiry, `None` while fresh.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u16")]
    pub stale_days: Option<u16>,
    /// `maxStaleDays` from the signed root (default 7, at most 90).
    pub max_stale_days: u16,
    /// Seconds since the epoch of the last successful refresh, `None` before the first.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "i64")]
    pub last_refresh_at: Option<i64>,
}

/// A registry that could not be opened (its config or trusted set is damaged); the others
/// are still listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryProblem {
    pub id: String,
    pub code: String,
    pub message: String,
}

/// `registry.list`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryList {
    pub registries: Vec<RegistryStatus>,
    pub problems: Vec<RegistryProblem>,
}

impl Registry {
    /// The registry's status now (`clock` decides staleness).
    pub fn status(&self, clock: &dyn Clock) -> RegistryStatus {
        let cfg = self.config();
        let repo = self.trusted();
        let (root, timestamp, snapshot, targets) = repo.versions();
        RegistryStatus {
            id: cfg.id.clone(),
            kind: "local".into(),
            path: cfg.path.to_string_lossy().into_owned(),
            root_sha256: cfg.root_sha256.clone(),
            freshness: "relaxed".into(),
            trusted: TrustedVersions {
                root,
                timestamp,
                snapshot,
                targets,
            },
            plugins: u32::try_from(search::packages(repo).len()).unwrap_or(u32::MAX),
            advisories: u32::try_from(self.advisories().len()).unwrap_or(u32::MAX),
            stale_days: repo.stale_days(clock),
            max_stale_days: repo.max_stale_days(),
            last_refresh_at: repo.last_refresh_at().ok().flatten(),
        }
    }
}
