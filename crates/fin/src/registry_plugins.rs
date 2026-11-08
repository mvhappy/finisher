//! The 8A `plugin` and `registry` commands' registry entries (plan 8A Task 12c), listed after
//! the settings commands in [`crate::registry::commands`]. The human-only ones are listed too:
//! `fin` sends them and the server refuses them with `HUMAN_ONLY` (exit 7).

use crate::commands::{plugin, project};
use crate::registry::{Command, schema_of};

const fn cmd(
    name: &'static str,
    group: &'static str,
    args: &'static str,
    summary: &'static str,
    schema: fn() -> serde_json::Value,
) -> Command {
    Command {
        name,
        group,
        args,
        summary,
        write: false,
        schema,
    }
}

pub const PLUGIN_COMMANDS: &[Command] = &[
    cmd(
        "plugin.search",
        "plugin",
        "[<text>] [--kind content|code] [--registry <r>]",
        "plugins the added registries list",
        || schema_of::<plugin::SearchArgs>("plugin.search"),
    ),
    cmd(
        "plugin.info",
        "plugin",
        "<id>",
        "what each registry signs about a plugin, and its installed row",
        || schema_of::<plugin::IdArgs>("plugin.info"),
    ),
    cmd(
        "plugin.ls",
        "plugin",
        "",
        "installed plugins, bundled ones, problems; REGISTRY_STALE warnings",
        || schema_of::<project::NoArgs>("plugin.ls"),
    ),
    cmd(
        "plugin.suggest",
        "plugin",
        "<id> --reason <text>",
        "suggest a plugin to the owner for this project (never an approval)",
        || schema_of::<plugin::SuggestArgs>("plugin.suggest"),
    ),
    cmd(
        "plugin.install",
        "plugin",
        "<id>[@<v>] [--registry <r>]",
        "human-only: the server refuses fin (HUMAN_ONLY, exit 7)",
        || schema_of::<plugin::InstallArgs>("plugin.install"),
    ),
    cmd(
        "plugin.update",
        "plugin",
        "<id>[@<v>] [--registry <r>]",
        "human-only: the server refuses fin (HUMAN_ONLY, exit 7)",
        || schema_of::<plugin::InstallArgs>("plugin.update"),
    ),
    cmd(
        "plugin.rm",
        "plugin",
        "<id> [--purge-data]",
        "human-only: the server refuses fin (HUMAN_ONLY, exit 7)",
        || schema_of::<plugin::RmArgs>("plugin.rm"),
    ),
    cmd(
        "plugin.enable",
        "plugin",
        "<id>",
        "human-only: the server refuses fin (HUMAN_ONLY, exit 7)",
        || schema_of::<plugin::IdArgs>("plugin.enable"),
    ),
    cmd(
        "plugin.disable",
        "plugin",
        "<id>",
        "human-only: the server refuses fin (HUMAN_ONLY, exit 7)",
        || schema_of::<plugin::IdArgs>("plugin.disable"),
    ),
    cmd(
        "registry.ls",
        "registry",
        "",
        "the added plugin registries and their state",
        || schema_of::<project::NoArgs>("registry.ls"),
    ),
    cmd(
        "registry.status",
        "registry",
        "<id>",
        "one registry: trusted versions, expiry, staleness, last refresh",
        || schema_of::<plugin::RegistryIdArgs>("registry.status"),
    ),
    cmd(
        "registry.refresh",
        "registry",
        "[<id>]",
        "refresh one registry or all (installs nothing for fin)",
        || schema_of::<plugin::RefreshArgs>("registry.refresh"),
    ),
    cmd(
        "registry.add",
        "registry",
        "<id> --dir <path> (--root <file>|--root-sha256 <hex>)",
        "human-only: the server refuses fin (HUMAN_ONLY, exit 7)",
        || schema_of::<plugin::AddArgs>("registry.add"),
    ),
    cmd(
        "registry.rm",
        "registry",
        "<id>",
        "human-only: the server refuses fin (HUMAN_ONLY, exit 7)",
        || schema_of::<plugin::RegistryIdArgs>("registry.rm"),
    ),
    cmd(
        "registry.forget",
        "registry",
        "--root <sha256>",
        "human-only: the server refuses fin (HUMAN_ONLY, exit 7)",
        || schema_of::<plugin::ForgetArgs>("registry.forget"),
    ),
];
