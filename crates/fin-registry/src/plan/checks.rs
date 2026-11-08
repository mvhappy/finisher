//! The plan's checks beyond manifest validation: the signed custom metadata, catalog ids, the
//! action against the installed version, licence warnings and look-alike ids.

use fin_plugin_api::{Invalid, LicenceClass, Manifest, PackageFiles, Permission, PluginId};
use semver::{Version, VersionReq};
use serde_json::{Value, json};
use sha2::Digest;

use super::types::{PlanAction, PlanLicence, PlanWarning};
use super::{PlanHost, PlanRequest};
use crate::error::RegistryError;
use crate::target::{TargetCustom, permissions_digest};

/// Validation's violations as one error: the first one's code and details, with every one
/// under `details.errors` when there are several.
pub(super) fn invalid(all: Vec<Invalid>) -> RegistryError {
    let first = all.first().cloned().unwrap_or_else(|| Invalid {
        code: "PLUGIN_MANIFEST_INVALID".into(),
        message: "the manifest is invalid".into(),
        details: Default::default(),
    });
    let mut e = RegistryError::new(&first.code, first.message, Value::Object(first.details));
    if all.len() > 1 {
        e.details.insert(
            "errors".into(),
            serde_json::to_value(&all).unwrap_or(Value::Null),
        );
    }
    e
}

/// `PLUGIN_MANIFEST_INVALID {path, reason}`.
pub(super) fn mismatch(path: &str, reason: &str, message: impl Into<String>) -> RegistryError {
    RegistryError::new(
        "PLUGIN_MANIFEST_INVALID",
        message,
        json!({ "path": path, "reason": reason }),
    )
}

/// The manifest must be what the registry signed for (spec §5.3 step 1).
pub(super) fn custom_matches(
    c: &TargetCustom,
    m: &Manifest,
    manifest_bytes: &[u8],
    bytes: u64,
) -> Result<(), RegistryError> {
    let host_api_same = VersionReq::parse(&c.host_api).is_ok_and(|r| r == m.host_api);
    let mut want_platforms = c.platforms.clone();
    let mut have_platforms = m.platforms.clone();
    want_platforms.sort();
    have_platforms.sort();
    let sha = crate::package::hex(&sha2::Sha256::digest(manifest_bytes));
    let fields = [
        ("id", c.id == m.id.to_string()),
        (
            "version",
            Version::parse(&c.version).is_ok_and(|v| v == m.version),
        ),
        ("kind", c.kind == m.kind),
        ("hostApi", host_api_same),
        ("platforms", want_platforms == have_platforms),
        ("license", c.license == m.license),
        (
            "permissionsDigest",
            c.permissions_digest == permissions_digest(&m.permissions),
        ),
        ("manifestSha256", c.manifest_sha256 == sha),
        ("bytesUnpacked", c.bytes_unpacked == bytes),
    ];
    match fields.iter().find(|(_, ok)| !ok) {
        None => Ok(()),
        Some((field, _)) => {
            let mut e = mismatch(
                "",
                "metadataMismatch",
                format!("the package's {field} is not what the registry signed"),
            );
            e.details.insert("field".into(), json!(field));
            Err(e)
        }
    }
}

/// A contributed font catalog must merge into the server's (8A Decision 22): no entry id the
/// built-in catalog or another pack holds, and no file or face listed twice between them.
pub(super) fn catalog_merge(
    m: &Manifest,
    files: &dyn PackageFiles,
    id: &PluginId,
    host: &dyn PlanHost,
) -> Result<(), RegistryError> {
    if m.contributes.font_catalogs.is_empty() {
        return Ok(());
    }
    let catalogs: Vec<Vec<u8>> = m
        .contributes
        .font_catalogs
        .iter()
        .map(|c| files.read(&c.file).unwrap_or_default())
        .collect();
    let Some(clash) = host.catalog_clash(&catalogs, id) else {
        return Ok(());
    };
    let path = format!("/contributes/fontCatalogs/{}/file", clash.index);
    let mut e = match &clash.entry {
        Some(entry) if clash.rule == "catalogIdTaken" => mismatch(
            &path,
            "catalogIdTaken",
            format!("font catalog entry {entry:?} is taken"),
        ),
        _ => mismatch(&path, "catalogMerge", clash.message.clone()),
    };
    e.details.insert("rule".into(), json!(clash.rule));
    if let Some(entry) = clash.entry {
        e.details.insert("entry".into(), json!(entry));
    }
    Err(e)
}

/// The action against the installed version, and the grants the diff is taken against.
pub(super) fn action(
    m: &Manifest,
    req: &PlanRequest,
) -> Result<(PlanAction, Option<String>, Vec<Permission>), RegistryError> {
    if let Some(want) = req.version.as_ref().filter(|v| **v != m.version) {
        let mut e = mismatch(
            "/version",
            "versionMismatch",
            format!("the package is {} {}, not {want}", m.id, m.version),
        );
        e.details.insert("version".into(), json!(want.to_string()));
        return Err(e);
    }
    let Some(installed) = &req.installed else {
        return Ok((PlanAction::Install, None, Vec::new()));
    };
    let have = Version::parse(&installed.version).map_err(|e| {
        RegistryError::new(
            "PLUGIN_MANIFEST_INVALID",
            format!(
                "the installed version {:?} is not semver: {e}",
                installed.version
            ),
            json!({ "path": "/installed/version", "reason": "version" }),
        )
    })?;
    let from = Some(installed.version.clone());
    let old = installed.permissions.clone();
    let details = json!({ "installed": installed.version, "version": m.version.to_string() });
    if m.version == have {
        return Err(RegistryError::new(
            "PLUGIN_ALREADY_INSTALLED",
            format!("{} {} is installed", m.id, m.version),
            details,
        ));
    }
    if !req.update {
        return Err(RegistryError::new(
            "PLUGIN_ALREADY_INSTALLED",
            format!("{} {have} is installed; plan an update", m.id),
            details,
        )
        .with_fix("plan with update: true"));
    }
    if m.version > have {
        return Ok((PlanAction::Update, from, old));
    }
    // Only a person naming this exact version downgrades (spec §5.3 step 4); `version` equals
    // the manifest's here when set.
    if !req.allow_downgrade || req.version.is_none() {
        return Err(RegistryError::new(
            "PLUGIN_DOWNGRADE_REFUSED",
            format!("{} {} is older than the installed {have}", m.id, m.version),
            details,
        )
        .with_fix("name the exact version and allow a downgrade"));
    }
    Ok((PlanAction::Downgrade, from, old))
}

pub(super) fn licence_warning(l: &PlanLicence) -> Option<PlanWarning> {
    let code = match l.class {
        LicenceClass::Copyleft | LicenceClass::WeakCopyleft => "copyleft",
        LicenceClass::Unknown => "licenceUnknown",
        _ => return None,
    };
    let class = serde_json::to_value(l.class).unwrap_or(Value::Null);
    Some(PlanWarning::new(
        code,
        json!({ "spdx": l.spdx, "class": class }),
    ))
}

/// The first of `known` that `id` could be mistaken for: the same after folding look-alike
/// characters (`0`/`o`, `1`/`l`/`i`, `5`/`s`, `rn`/`m`, `vv`/`w`, hyphens dropped), or one edit
/// away. `id` itself is never a look-alike of itself.
pub fn lookalike<'a>(id: &str, known: &'a [String]) -> Option<&'a str> {
    let skel = skeleton(id);
    known
        .iter()
        .filter(|k| k.as_str() != id)
        .find(|k| skeleton(k) == skel || edits_at_most_one(id, k))
        .map(String::as_str)
}

fn skeleton(s: &str) -> String {
    let folded: String = s
        .to_ascii_lowercase()
        .chars()
        .filter(|c| *c != '-')
        .map(|c| match c {
            '0' => 'o',
            '1' | 'i' => 'l',
            '5' => 's',
            _ => c,
        })
        .collect();
    folded.replace("rn", "m").replace("vv", "w")
}

fn edits_at_most_one(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    if long.len() - short.len() > 1 {
        return false;
    }
    let mut i = 0;
    while i < short.len() && short[i] == long[i] {
        i += 1;
    }
    if i == short.len() {
        return true;
    }
    if short.len() == long.len() {
        short[i + 1..] == long[i + 1..]
    } else {
        short[i..] == long[i + 1..]
    }
}
