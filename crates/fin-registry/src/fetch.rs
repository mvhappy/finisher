//! A registry's package as a plan's source (spec §5.3 step 1): the version a plan installs,
//! fetched from the folder with its signed length as the cap and checked against its signed
//! SHA-256, with the advisories that apply to it as plan warnings ([`VerifiedTarget`]).

use std::path::Path;

use fin_plugin_api::PluginId;
use serde_json::json;

use crate::advisory::applicable;
use crate::clock::Clock;
use crate::error::RegistryError;
use crate::plan::PlanWarning;
use crate::registry::Registry;
use crate::target::{TargetCustom, VerifiedTarget};

impl Registry {
    /// Fetches `id` at `version` (or the newest version not yanked) into `out` and describes it
    /// as the TUF client verified it. `PLUGIN_NOT_FOUND {id, registryId, version?}` when the
    /// trusted metadata lists no such package.
    pub fn verified_target(
        &self,
        id: &PluginId,
        version: Option<&semver::Version>,
        out: &Path,
        clock: &dyn Clock,
    ) -> Result<VerifiedTarget, RegistryError> {
        let registry_id = self.config().id.clone();
        let info = self.resolve(id, version).ok_or_else(|| {
            let mut details = json!({ "id": id.to_string(), "registryId": registry_id });
            if let Some(v) = version {
                details["version"] = json!(v.to_string());
            }
            let what = version.map_or_else(
                || "an installable version of".to_string(),
                |v| format!("{v} of"),
            );
            RegistryError::new(
                "PLUGIN_NOT_FOUND",
                format!("registry {registry_id:?} lists no {what} {id}"),
                details,
            )
            .with_fix("refresh the registry, or check the plugin's id and version")
        })?;
        let target = self.trusted().target(&info.target).ok_or_else(|| {
            RegistryError::new(
                "PLUGIN_NOT_FOUND",
                format!("no trusted target {:?}", info.target),
                json!({ "id": id.to_string(), "registryId": registry_id }),
            )
        })?;
        let custom = TargetCustom::from_map(&target.custom).ok_or_else(|| {
            RegistryError::new(
                "REGISTRY_INVALID",
                format!("{} carries no valid package metadata", info.target),
                json!({ "reason": "custom", "target": info.target }),
            )
        })?;
        self.trusted()
            .fetch_target(self.transport(), &info.target, out)?;
        let v = semver::Version::parse(&info.version).map_err(|e| {
            RegistryError::new(
                "REGISTRY_INVALID",
                format!("{} has version {:?}: {e}", info.target, info.version),
                json!({ "reason": "custom", "target": info.target }),
            )
        })?;
        let all = self.advisories();
        let warnings = applicable(&all, &id.to_string(), &v)
            .into_iter()
            .map(|a| {
                PlanWarning::new(
                    "advisory",
                    json!({
                        "id": a.advisory.id,
                        "severity": a.advisory.severity,
                        "action": a.advisory.action,
                        "signedBy": a.signed_by,
                    }),
                )
            })
            .collect();
        Ok(VerifiedTarget {
            registry_id,
            archive: out.to_path_buf(),
            length: target.length,
            sha256: target.sha256,
            custom,
            stale_days: self.stale_days(clock),
            warnings,
        })
    }

    /// The installed versions (`(id, version)`) a `disable` advisory of this registry applies
    /// to, with the advisory's id: what a refresh by anyone revokes (spec §6.5).
    pub fn revocations(&self, installed: &[(String, String)]) -> Vec<(String, String, String)> {
        let all = self.advisories();
        let mut out = Vec::new();
        for (id, version) in installed {
            let Ok(v) = semver::Version::parse(version) else {
                continue;
            };
            if let Some(a) = applicable(&all, id, &v)
                .into_iter()
                .find(|a| a.advisory.action == crate::advisory::AdvisoryAction::Disable)
            {
                out.push((id.clone(), version.clone(), a.advisory.id.clone()));
            }
        }
        out
    }
}
