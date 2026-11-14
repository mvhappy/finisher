//! `fin plugin …` and `fin registry …` (plan 8A Task 12c; spec §5, arch §8.3). Every command is
//! one or two server RPCs, so offline they are `SERVER_UNAVAILABLE`:
//! - reads, for any principal with `plugin.read`: `plugin search|info|ls`, `registry
//!   ls|status|refresh` (a refresh by a non-human installs nothing);
//! - `plugin suggest <id> --reason <text>` records a suggestion for a person (never an
//!   approval) on the current project;
//! - what only a person may do: `plugin install|update` (a `plugin.plan`, then `plugin.install`
//!   of that plan), `plugin rm|enable|disable` and `registry add|rm|forget`. `fin` sends them as
//!   they are: the server answers `HUMAN_ONLY` (exit 7) to `fin` and to every agent, and `fin`
//!   prints that answer unchanged. It never asks, prompts or retries; the owner installs from
//!   the Finisher app or the server's console.

use clap::{Args, Subcommand, ValueEnum};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::backend::Backend;
use crate::output::usage;
use crate::paths;

#[derive(Debug, Clone, Subcommand)]
pub enum PluginCmd {
    Search(SearchArgs),
    Info(IdArgs),
    Ls,
    Suggest(SuggestArgs),
    Install(InstallArgs),
    Update(InstallArgs),
    Rm(RmArgs),
    Enable(IdArgs),
    Disable(IdArgs),
    Commands(CommandsArgs),
}

/// The plugin commands this principal can run (`fin <id> <command>`), of one plugin or all.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandsArgs {
    #[arg(value_name = "id")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum RegistryCmd {
    Ls,
    Status(RegistryIdArgs),
    Refresh(RefreshArgs),
    Add(AddArgs),
    Rm(RegistryIdArgs),
    Forget(ForgetArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Content,
    Code,
}

/// Plugins the added registries list, matching `text` (id, name or description).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    #[arg(value_name = "text")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[arg(long, value_enum, value_name = "kind")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    /// Only this registry.
    #[arg(long, value_name = "r")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}

/// A plugin id, `@publisher/name`.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    #[arg(value_name = "id")]
    pub id: String,
}

/// Suggests a plugin to the owner for the current project (shown as a suggestion, never an
/// approval).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestArgs {
    #[arg(value_name = "id")]
    pub id: String,
    /// Why, for the person who decides (1–2000 characters).
    #[arg(long, value_name = "text")]
    pub reason: String,
}

/// Human-only: `fin` gets the server's `HUMAN_ONLY` refusal (exit 7).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallArgs {
    /// `@publisher/name` or `@publisher/name@<version>`.
    #[arg(value_name = "id[@v]")]
    pub id: String,
    /// The registry to install from, when several list the plugin.
    #[arg(long, value_name = "r")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}

/// Human-only: `fin` gets the server's `HUMAN_ONLY` refusal (exit 7).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RmArgs {
    #[arg(value_name = "id")]
    pub id: String,
    /// Delete the plugin's data too.
    #[arg(long = "purge-data")]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub purge_data: bool,
}

/// A registry id (`[a-z0-9][a-z0-9-]{0,63}`).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryIdArgs {
    #[arg(value_name = "id")]
    pub id: String,
}

/// Refreshes one registry, or every added one.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RefreshArgs {
    #[arg(value_name = "id")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Human-only: `fin` gets the server's `HUMAN_ONLY` refusal (exit 7).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    #[arg(value_name = "id")]
    pub id: String,
    /// The registry folder.
    #[arg(long, value_name = "path")]
    pub dir: String,
    /// The registry's `1.root.json`, trusted as is.
    #[arg(long, value_name = "file", conflicts_with = "root_sha256")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// The SHA-256 of its `1.root.json`.
    #[arg(long = "root-sha256", value_name = "hex")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_sha256: Option<String>,
}

/// Human-only: `fin` gets the server's `HUMAN_ONLY` refusal (exit 7).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForgetArgs {
    /// The SHA-256 of the trusted root to forget.
    #[arg(long, value_name = "sha256")]
    pub root: String,
}

/// `@pub/name@1.2.0` → (`@pub/name`, `Some("1.2.0")`); the id's own `@` is its first byte.
fn split_version(spec: &str) -> (&str, Option<&str>) {
    match spec.get(1..).and_then(|rest| rest.find('@')) {
        Some(i) => (&spec[..i + 1], Some(&spec[i + 2..])),
        None => (spec, None),
    }
}

fn absolute(path: &str) -> Result<String, CoreError> {
    Ok(paths::resolve_checked_str(path)?.display().to_string())
}

/// `plugin.list`, with a `REGISTRY_STALE` warning for each installed plugin whose registry's
/// metadata is past its expiry.
fn ls(backend: &dyn Backend) -> Result<Value, CoreError> {
    let mut list = backend.server_call("plugin.list", json!({}))?;
    let stale: Vec<Value> = list["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let days = p.get("registryStaleDays")?.as_u64()?;
            Some(json!({
                "code": "REGISTRY_STALE",
                "plugin": p["id"],
                "registry": p["registryId"],
                "days": days,
                "fix": "ask the owner to refresh the registry or fix its publisher's metadata",
            }))
        })
        .collect();
    if !stale.is_empty() {
        list["warnings"] = json!(stale);
    }
    Ok(list)
}

/// `plugin install|update`: the plan, then its approval. The server decides who may approve it
/// (a person only): `fin` sends both and returns what the server answers.
fn install(backend: &dyn Backend, a: &InstallArgs, update: bool) -> Result<Value, CoreError> {
    let (id, version) = split_version(&a.id);
    let mut req = json!({ "id": id });
    if let Some(v) = version {
        req["version"] = json!(v);
    }
    if let Some(r) = &a.registry {
        req["registry"] = json!(r);
    }
    if update {
        req["update"] = json!(true);
    }
    let plan = backend.server_call("plugin.plan", req)?;
    let (Some(plan_id), Some(digest)) = (plan["planId"].as_str(), plan["digest"].as_str()) else {
        return Err(crate::output::internal(
            "plugin.plan answered without a planId and digest",
            json!({ "answer": plan }),
        ));
    };
    backend.server_call(
        "plugin.install",
        json!({ "planId": plan_id, "digest": digest }),
    )
}

pub fn plugin(backend: &dyn Backend, cmd: &PluginCmd) -> Result<Value, CoreError> {
    match cmd {
        PluginCmd::Search(a) => {
            let mut req = json!({});
            if let Some(t) = &a.text {
                req["text"] = json!(t);
            }
            if let Some(k) = a.kind {
                req["kind"] = json!(k);
            }
            if let Some(r) = &a.registry {
                req["registry"] = json!(r);
            }
            let found = backend.server_call("plugin.search", req)?;
            Ok(json!({ "plugins": found }))
        }
        PluginCmd::Info(a) => backend.server_call("plugin.info", json!({ "id": a.id })),
        PluginCmd::Ls => ls(backend),
        PluginCmd::Suggest(a) => {
            let project_id = backend.project_id()?;
            let req = json!({ "id": a.id, "reason": a.reason, "projectId": project_id });
            backend.server_call("plugin.suggest", req)
        }
        PluginCmd::Install(a) => install(backend, a, false),
        PluginCmd::Update(a) => install(backend, a, true),
        PluginCmd::Rm(a) => {
            let mut req = json!({ "id": a.id });
            if a.purge_data {
                req["purgeData"] = json!(true);
            }
            backend.server_call("plugin.remove", req)
        }
        PluginCmd::Enable(a) => backend.server_call("plugin.enable", json!({ "id": a.id })),
        PluginCmd::Disable(a) => backend.server_call("plugin.disable", json!({ "id": a.id })),
        PluginCmd::Commands(a) => crate::commands::plugin_cmd::list(backend, a.id.as_deref()),
    }
}

pub fn registry(backend: &dyn Backend, cmd: &RegistryCmd) -> Result<Value, CoreError> {
    match cmd {
        RegistryCmd::Ls => backend.server_call("registry.list", json!({})),
        RegistryCmd::Status(a) => backend.server_call("registry.status", json!({ "id": a.id })),
        RegistryCmd::Refresh(a) => {
            let req = a.id.as_ref().map_or(json!({}), |id| json!({ "id": id }));
            backend.server_call("registry.refresh", req)
        }
        RegistryCmd::Add(a) => {
            let root = match (&a.root, &a.root_sha256) {
                (Some(file), None) => json!({ "kind": "file", "path": absolute(file)? }),
                (None, Some(hex)) => json!({ "kind": "sha256", "sha256": hex }),
                _ => {
                    return Err(usage(
                        "registry add needs the root it trusts the registry through",
                        json!({ "command": "registry.add" }),
                        "pass --root <1.root.json> or --root-sha256 <hex>",
                    ));
                }
            };
            let cfg = json!({ "id": a.id, "path": absolute(&a.dir)? });
            backend.server_call("registry.add", json!({ "cfg": cfg, "root": root }))
        }
        RegistryCmd::Rm(a) => backend.server_call("registry.remove", json!({ "id": a.id })),
        RegistryCmd::Forget(a) => {
            backend.server_call("registry.forget", json!({ "rootSha256": a.root }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::split_version;

    #[test]
    fn a_version_follows_the_second_at() {
        assert_eq!(split_version("@acme/fonts"), ("@acme/fonts", None));
        assert_eq!(
            split_version("@acme/fonts@1.2.0"),
            ("@acme/fonts", Some("1.2.0"))
        );
        assert_eq!(split_version("acme@1"), ("acme", Some("1")));
        assert_eq!(split_version(""), ("", None));
    }
}
