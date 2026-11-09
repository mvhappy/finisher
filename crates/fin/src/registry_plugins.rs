//! The 8A `plugin` and `registry` commands' registry entries (plan 8A Task 12c), listed after
//! the settings commands in [`crate::registry::commands`]. The human-only ones are listed too:
//! `fin` sends them and the server refuses them with `HUMAN_ONLY` (exit 7). Then the 5C-2 `job`
//! commands and `wait`, then the 7B `approval` commands.

use crate::commands::{approval, job, plugin, project};
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

pub const JOB_COMMANDS: &[Command] = &[
    cmd(
        "job.ls",
        "job",
        "[--state <s>]… [--kind <k>] [--mine] [--limit <n>] [--cursor <c>]",
        "the jobs this principal sees (its own; every owner's with jobs.control), newest first",
        || schema_of::<job::LsArgs>("job.ls"),
    ),
    cmd(
        "job.show",
        "job",
        "<id>",
        "one job's view (another owner's or out of reach: JOB_NOT_FOUND)",
        || schema_of::<job::IdArgs>("job.show"),
    ),
    cmd(
        "job.cancel",
        "job",
        "<id>",
        "cancel a job; its view once the run ended",
        || schema_of::<job::IdArgs>("job.cancel"),
    ),
    cmd(
        "job.pause",
        "job",
        "(<id>|--all)",
        "pause a job, or dispatch and every job (--all: jobs.control)",
        || schema_of::<job::TargetArgs>("job.pause"),
    ),
    cmd(
        "job.resume",
        "job",
        "(<id>|--all)",
        "resume a paused job, or every job and dispatch (--all)",
        || schema_of::<job::TargetArgs>("job.resume"),
    ),
    cmd(
        "wait",
        "job",
        "<id>… [--any] [--timeout <s>]",
        "wait for jobs: exit 0 succeeded, 4 JOB_FAILED, 5 WAIT_TIMEOUT",
        || schema_of::<job::WaitArgs>("wait"),
    ),
];

pub const APPROVAL_COMMANDS: &[Command] = &[
    cmd(
        "approval.wait",
        "approval",
        "<id> [--timeout <s>]",
        "wait <=90 s for a person's answer: exit 0 allowed, 7 APPROVAL_DENIED, 8 still pending",
        || schema_of::<approval::WaitArgs>("approval.wait"),
    ),
    cmd(
        "approval.ls",
        "approval",
        "[--status open|answered|expired] [--session <id>]",
        "this session's approvals, newest first",
        || schema_of::<approval::LsArgs>("approval.ls"),
    ),
];
