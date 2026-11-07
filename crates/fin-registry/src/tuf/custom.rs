//! Signed custom metadata (spec §6.1): what a package target and the root carry for
//! Finisher, and the registry's target names.

use fin_plugin_api::{Manifest, Permission, Platform, PluginKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tough::schema::Root;

use crate::package::hex;

/// `custom.finisher.maxStaleDays` when the root does not set it (spec §6.3, owner decision).
pub const DEFAULT_MAX_STALE_DAYS: u16 = 7;
/// The largest `maxStaleDays` a root may set; larger values are clamped.
pub const MAX_STALE_DAYS_CAP: u16 = 90;

/// The custom metadata of a package target, signed by its publisher's role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageCustom {
    pub id: String,
    pub version: String,
    pub kind: PluginKind,
    pub host_api: String,
    #[serde(default)]
    pub platforms: Vec<Platform>,
    pub license: String,
    /// [`permissions_digest`] of the manifest's permissions.
    pub permissions_digest: String,
    /// Lowercase hex SHA-256 of `finisher-plugin.json`.
    pub manifest_sha256: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub yanked: bool,
    pub bytes_unpacked: u64,
}

impl PackageCustom {
    /// The custom metadata of a package whose manifest is `manifest` (bytes `manifest_bytes`)
    /// and whose files unpack to `bytes_unpacked` bytes.
    pub fn from_manifest(m: &Manifest, manifest_bytes: &[u8], bytes_unpacked: u64) -> Self {
        PackageCustom {
            id: m.id.to_string(),
            version: m.version.to_string(),
            kind: m.kind,
            host_api: m.host_api.to_string(),
            platforms: m.platforms.clone(),
            license: m.license.clone(),
            permissions_digest: permissions_digest(&m.permissions),
            manifest_sha256: hex(&Sha256::digest(manifest_bytes)),
            yanked: false,
            bytes_unpacked,
        }
    }

    pub fn to_map(&self) -> Map<String, Value> {
        match serde_json::to_value(self) {
            Ok(Value::Object(m)) => m,
            _ => unreachable!("a struct serialises to an object"),
        }
    }

    pub fn from_map(m: &Map<String, Value>) -> Option<PackageCustom> {
        serde_json::from_value(Value::Object(m.clone())).ok()
    }
}

/// Lowercase hex SHA-256 of the permissions as a JSON array, each permission's JSON sorted,
/// so reordering the manifest does not change it.
pub fn permissions_digest(perms: &[Permission]) -> String {
    let mut each: Vec<String> = perms
        .iter()
        .map(|p| serde_json::to_string(p).unwrap_or_default())
        .collect();
    each.sort();
    each.dedup();
    let joined = format!("[{}]", each.join(","));
    hex(&Sha256::digest(joined.as_bytes()))
}

/// `custom.finisher.maxStaleDays` of a root: default 7, at most 90.
pub fn max_stale_days(root: &Root) -> u16 {
    root._extra
        .get("custom")
        .and_then(|c| c.get("finisher"))
        .and_then(|f| f.get("maxStaleDays"))
        .and_then(Value::as_u64)
        .map_or(DEFAULT_MAX_STALE_DAYS, |d| {
            d.min(u64::from(MAX_STALE_DAYS_CAP)) as u16
        })
}

/// The `custom` value a root carries for `maxStaleDays`.
pub fn root_custom(max_stale_days: u16) -> Value {
    serde_json::json!({ "finisher": { "maxStaleDays": max_stale_days } })
}

/// The target name of a package: `<publisher>/<name>/<version>.finpkg`.
pub fn package_target(publisher: &str, name: &str, version: &str) -> String {
    format!("{publisher}/{name}/{version}.finpkg")
}

/// The target name of a publisher's advisories (`None`: the registry-wide file).
pub fn advisories_target(publisher: Option<&str>) -> String {
    match publisher {
        Some(p) => format!("{p}/{ADVISORIES_FILE}"),
        None => ADVISORIES_FILE.into(),
    }
}

pub const ADVISORIES_FILE: &str = "advisories.json";

/// The consistent file name of a target under `targets/` (TUF 1.0 §5.7.3): the basename
/// prefixed with its SHA-256, so `acme/pack/1.0.0.finpkg` is `acme/pack/<sha>.1.0.0.finpkg`.
pub fn consistent_target_path(name: &str, sha256_hex: &str) -> String {
    match name.rsplit_once('/') {
        Some((dir, base)) => format!("{dir}/{sha256_hex}.{base}"),
        None => format!("{sha256_hex}.{name}"),
    }
}

/// Top-level role names, which a publisher's delegated role may not take.
pub const TOP_LEVEL_ROLES: [&str; 4] = ["root", "targets", "snapshot", "timestamp"];
