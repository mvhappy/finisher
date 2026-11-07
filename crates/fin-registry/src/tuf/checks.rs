//! Checks `tough` does not make (8A Task 1 notes, spec §6.1–6.2): consistent snapshots,
//! one terminating delegation per publisher confined to `<publisher>/*`, no nested
//! delegations, at most 500 delegations, and no delegated role silently left unloaded.

use fin_plugin_api::PluginId;
use tough::schema::{PathSet, Root, Signed, Snapshot, Targets};

use super::TufError;
use super::adapter::MAX_DELEGATIONS;
use super::custom::TOP_LEVEL_ROLES;

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

pub(crate) fn delegations(
    targets: &Signed<Targets>,
    snapshot: &Signed<Snapshot>,
) -> Result<(), TufError> {
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
            if !target.raw().starts_with(&prefix) || target.raw() != target.resolved() {
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
