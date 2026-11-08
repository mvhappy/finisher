//! What a registry offers, read from signed custom metadata only (spec §6.1): no package is
//! downloaded to search or describe it. A package target is listed only when its custom
//! metadata parses and agrees with where it is signed: the id's publisher is the role that
//! signs it, and the target name is `<publisher>/<name>/<version>.finpkg`.

use std::collections::BTreeMap;

use fin_plugin_api::{Platform, PluginId, PluginKind};
use semver::Version;
use serde::Serialize;

use crate::advisory::SignedAdvisory;
use crate::tuf::TrustedRepo;
use crate::tuf::custom::package_target;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Matches the id, case-insensitively, as a substring.
    pub text: Option<String>,
    pub kind: Option<PluginKind>,
}

/// One published version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub version: String,
    pub yanked: bool,
    pub kind: PluginKind,
    pub host_api: String,
    pub platforms: Vec<Platform>,
    pub license: String,
    pub permissions_digest: String,
    pub manifest_sha256: String,
    pub bytes_unpacked: u64,
    /// The target name, its signed length and SHA-256.
    pub target: String,
    pub length: u64,
    pub sha256: String,
}

/// A plugin in search results: its newest version that is not yanked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub registry_id: String,
    pub id: String,
    pub latest: String,
    /// Versions offered to new installs (not yanked), oldest first.
    pub versions: Vec<String>,
    pub kind: PluginKind,
    pub license: String,
}

/// Everything a registry signs about one plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub registry_id: String,
    pub id: String,
    /// Every version, yanked ones included, oldest first.
    pub versions: Vec<VersionInfo>,
    /// In-scope advisories that apply to at least one listed version.
    pub advisories: Vec<SignedAdvisory>,
}

/// Every consistent package target by plugin id, versions oldest first.
pub(crate) fn packages(repo: &TrustedRepo) -> BTreeMap<String, Vec<(Version, VersionInfo)>> {
    let mut out: BTreeMap<String, Vec<(Version, VersionInfo)>> = BTreeMap::new();
    for t in repo.targets() {
        let Some(p) = t.package() else { continue };
        let (Ok(id), Ok(version)) = (PluginId::parse(&p.id), Version::parse(&p.version)) else {
            continue;
        };
        if t.role != id.publisher()
            || t.name != package_target(id.publisher(), id.name(), &p.version)
            || !version.build.is_empty()
        {
            continue;
        }
        let info = VersionInfo {
            version: p.version.clone(),
            yanked: p.yanked,
            kind: p.kind,
            host_api: p.host_api,
            platforms: p.platforms,
            license: p.license,
            permissions_digest: p.permissions_digest,
            manifest_sha256: p.manifest_sha256,
            bytes_unpacked: p.bytes_unpacked,
            target: t.name.clone(),
            length: t.length,
            sha256: t.sha256.clone(),
        };
        out.entry(p.id).or_default().push((version, info));
    }
    for v in out.values_mut() {
        v.sort_by(|a, b| a.0.cmp(&b.0));
    }
    out
}

pub(crate) fn search(registry_id: &str, repo: &TrustedRepo, q: &SearchQuery) -> Vec<Listing> {
    let text = q.text.as_ref().map(|t| t.to_lowercase());
    packages(repo)
        .into_iter()
        .filter(|(id, _)| text.as_ref().is_none_or(|t| id.to_lowercase().contains(t)))
        .filter_map(|(id, versions)| {
            let offered: Vec<&VersionInfo> = versions
                .iter()
                .map(|(_, i)| i)
                .filter(|i| !i.yanked)
                .collect();
            let latest = *offered.last()?;
            if q.kind.is_some_and(|k| k != latest.kind) {
                return None;
            }
            Some(Listing {
                registry_id: registry_id.to_string(),
                id,
                latest: latest.version.clone(),
                versions: offered.iter().map(|i| i.version.clone()).collect(),
                kind: latest.kind,
                license: latest.license.clone(),
            })
        })
        .collect()
}

pub(crate) fn info(
    registry_id: &str,
    repo: &TrustedRepo,
    advisories: &[SignedAdvisory],
    id: &PluginId,
) -> Option<PluginInfo> {
    let id = id.to_string();
    let versions = packages(repo).remove(&id)?;
    let applying = advisories
        .iter()
        .filter(|a| versions.iter().any(|(v, _)| a.applies_to(&id, v)))
        .cloned()
        .collect();
    Some(PluginInfo {
        registry_id: registry_id.to_string(),
        id,
        versions: versions.into_iter().map(|(_, i)| i).collect(),
        advisories: applying,
    })
}
