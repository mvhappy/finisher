//! What a registry vouches for (spec §6.1): a target's signed custom metadata, and a target
//! the TUF client fetched and verified ([`VerifiedTarget`], made by `Registry` in Task 7 and
//! by tests). [`permissions_digest`] is the digest publishers sign into the custom metadata.

use std::path::PathBuf;

use fin_plugin_api::{Manifest, Permission, Platform};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::plan::PlanWarning;

/// A target's signed custom metadata (`custom` in the delegated targets role).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetCustom {
    pub id: String,
    pub version: String,
    pub kind: fin_plugin_api::PluginKind,
    pub host_api: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

/// A target fetched by the TUF client, whose bytes matched its signed length and hashes.
/// [`crate::plan::make_plan`] checks the file against them again while copying it into
/// staging.
#[derive(Debug, Clone)]
pub struct VerifiedTarget {
    pub registry_id: String,
    /// The fetched `.finpkg`.
    pub archive: PathBuf,
    /// The signed length and SHA-256 (lowercase hex).
    pub length: u64,
    pub sha256: String,
    pub custom: TargetCustom,
    /// Days the registry is past its expiry (relaxed freshness), when it is.
    pub stale_days: Option<u16>,
    /// Advisories and other registry warnings that apply to this version (Task 7).
    pub warnings: Vec<PlanWarning>,
}

/// BLAKE3 (hex) over the permissions' canonical JSON, sorted, so order never matters.
pub fn permissions_digest(perms: &[Permission]) -> String {
    let mut items: Vec<String> = perms
        .iter()
        .map(|p| crate::plan::canonical_json(&serde_json::to_value(p).unwrap_or_default()))
        .collect();
    items.sort();
    items.dedup();
    blake3::hash(format!("[{}]", items.join(",")).as_bytes())
        .to_hex()
        .to_string()
}

/// The custom metadata a publisher signs for `manifest` (`fin-plugin registry publish`, tests).
pub fn custom_for(manifest: &Manifest, manifest_bytes: &[u8], bytes_unpacked: u64) -> TargetCustom {
    use sha2::Digest;
    TargetCustom {
        id: manifest.id.to_string(),
        version: manifest.version.to_string(),
        kind: manifest.kind,
        host_api: manifest.host_api.to_string(),
        platforms: manifest.platforms.clone(),
        license: manifest.license.clone(),
        permissions_digest: permissions_digest(&manifest.permissions),
        manifest_sha256: crate::package::hex(&sha2::Sha256::digest(manifest_bytes)),
        yanked: false,
        bytes_unpacked,
    }
}

/// A package described as a publisher signs it: its length, SHA-256 and custom metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    pub length: u64,
    pub sha256: String,
    pub custom: TargetCustom,
}

/// Reads `archive` through the strict reader into `scratch` (which must not exist, and is
/// removed again) and describes it ([`custom_for`]); `fin-plugin registry publish` and test
/// sources use it.
pub fn describe_package(
    archive: &std::path::Path,
    scratch: &std::path::Path,
) -> Result<Described, crate::RegistryError> {
    use sha2::Digest;
    let bytes = std::fs::read(archive).map_err(|e| crate::RegistryError::io("read", archive, e))?;
    let unpacked = crate::package::unpack(
        archive,
        scratch,
        |m: &Manifest| m.executables(),
        &crate::package::Limits::default(),
    );
    let _ = std::fs::remove_dir_all(scratch);
    let unpacked = unpacked?;
    let manifest: Manifest = serde_json::from_slice(&unpacked.manifest).map_err(|e| {
        crate::RegistryError::new(
            "PLUGIN_MANIFEST_INVALID",
            format!("the manifest does not parse: {e}"),
            serde_json::json!({ "path": "", "reason": "schema" }),
        )
    })?;
    let total = unpacked.files.values().map(|f| f.size).sum();
    Ok(Described {
        length: bytes.len() as u64,
        sha256: crate::package::hex(&sha2::Sha256::digest(&bytes)),
        custom: custom_for(&manifest, &unpacked.manifest, total),
    })
}
