//! Signed custom metadata (spec §6.1): what a package target and the root carry for
//! Finisher, and the registry's target names. A package target's custom metadata is
//! [`crate::target::TargetCustom`].

use fin_plugin_api::Permission;
use serde_json::Value;
use tough::schema::Root;

/// `custom.finisher.maxStaleDays` when the root does not set it (spec §6.3, owner decision).
pub const DEFAULT_MAX_STALE_DAYS: u16 = 7;
/// The largest `maxStaleDays` a root may set; larger values are clamped.
pub const MAX_STALE_DAYS_CAP: u16 = 90;

/// The digest publishers sign into `permissionsDigest`: one definition for the editor that
/// signs it and the install plan that checks it ([`crate::target::permissions_digest`]).
pub fn permissions_digest(perms: &[Permission]) -> String {
    crate::target::permissions_digest(perms)
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
