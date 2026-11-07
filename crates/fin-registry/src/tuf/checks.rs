//! Checks `tough` does not make (8A Task 1 notes, spec §6.1–6.2): consistent snapshots,
//! one terminating delegation per publisher confined to `<publisher>/*`, no nested
//! delegations, at most 500 delegations, no delegated role silently left unloaded, and target
//! names that are plain package paths (top-level `targets`' too).

use fin_plugin_api::PluginId;
use tough::schema::{PathSet, Root, Signed, Snapshot, Targets};

use super::adapter::MAX_DELEGATIONS;
use super::custom::TOP_LEVEL_ROLES;
use super::{Layout, TufError};
use crate::package::{Limits, check_path};

pub(crate) fn root(root: &Signed<Root>) -> Result<(), TufError> {
    if !root.signed.consistent_snapshot {
        return Err(TufError::invalid(
            "layout",
            "the root does not use consistent snapshots",
        ));
    }
    if !root.signed.spec_version.starts_with("1.") {
        return Err(TufError::invalid(
            "layout",
            format!("TUF spec version {} is not 1.x", root.signed.spec_version),
        ));
    }
    Ok(())
}

/// Whether `name` can name a publisher's role.
pub(crate) fn publisher_name_ok(name: &str) -> bool {
    !TOP_LEVEL_ROLES.contains(&name) && PluginId::parse(&format!("@{name}/xx")).is_ok()
}

/// Whether a target name is a plain relative path that names one file on every OS.
fn target_name_ok(t: &tough::TargetName) -> bool {
    t.raw() == t.resolved() && check_path(t.raw(), &Limits::default()).is_ok()
}

pub(crate) fn delegations(
    targets: &Signed<Targets>,
    snapshot: &Signed<Snapshot>,
    layout: Layout,
) -> Result<(), TufError> {
    if layout == Layout::Finisher
        && let Some(bad) = targets.signed.targets.keys().find(|t| !target_name_ok(t))
    {
        let mut e = TufError::delegation(
            "targets",
            format!("targets lists {:?}, not a plain path", bad.raw()),
        );
        e.details
            .insert("target".into(), bad.raw().to_string().into());
        return Err(e);
    }
    let Some(d) = &targets.signed.delegations else {
        return Ok(());
    };
    if d.roles.len() > MAX_DELEGATIONS {
        return Err(TufError::new(
            "REGISTRY_INVALID",
            format!("{} delegations; at most {MAX_DELEGATIONS}", d.roles.len()),
            serde_json::json!({ "reason": "tooManyDelegations", "max": MAX_DELEGATIONS }),
        ));
    }
    for role in &d.roles {
        let name = role.name.as_str();
        if layout == Layout::Generic {
            let listed = snapshot.signed.meta.contains_key(&format!("{name}.json"));
            if listed && role.targets.is_none() {
                return Err(TufError::delegation(
                    name,
                    format!("{name} is in the snapshot but was not loaded"),
                ));
            }
            continue;
        }
        if !publisher_name_ok(name) {
            return Err(TufError::delegation(
                name,
                format!("{name:?} is not a publisher name"),
            ));
        }
        let want = format!("{name}/*");
        let confined = matches!(&role.paths, PathSet::Paths(p)
            if p.len() == 1 && p[0].value() == want);
        if !confined || !role.terminating {
            return Err(TufError::delegation(
                name,
                format!("{name} must be one terminating delegation of {want}"),
            ));
        }
        let listed = snapshot.signed.meta.contains_key(&format!("{name}.json"));
        let Some(t) = &role.targets else {
            if listed {
                return Err(TufError::delegation(
                    name,
                    format!("{name} is in the snapshot but was not loaded"),
                ));
            }
            continue;
        };
        if t.signed
            .delegations
            .as_ref()
            .is_some_and(|d| !d.roles.is_empty())
        {
            return Err(TufError::delegation(
                name,
                format!("{name} delegates further"),
            ));
        }
        let prefix = format!("{name}/");
        for target in t.signed.targets.keys() {
            if !target.raw().starts_with(&prefix) || !target_name_ok(target) {
                let mut e = TufError::delegation(
                    name,
                    format!("{name} lists {:?}, outside {want}", target.raw()),
                );
                e.details
                    .insert("target".into(), target.raw().to_string().into());
                return Err(e);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "checks_tests.rs"]
mod tests;
