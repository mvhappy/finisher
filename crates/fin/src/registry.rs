//! The one command registry: names, groups, usage lines, summaries and input schemas. The map,
//! `fin help` and `fin schema` are all generated from it.

use schemars::JsonSchema;
use serde_json::{Value, json};

use crate::commands::{asset, branch, clip, drop, history, look, project, review, try_cmd, view};

pub struct Command {
    /// Dotted name, e.g. "clip.insert" (also the `batch` cmd and op-style name).
    pub name: &'static str,
    pub group: &'static str,
    /// Arguments after `fin <words>`.
    pub args: &'static str,
    pub summary: &'static str,
    /// Changes the project (so supports `--dry-run`, `--idem-key`).
    pub write: bool,
    /// JSON Schema of the command's input (the `args` object in `batch`).
    pub schema: fn() -> Value,
}

impl Command {
    /// `fin clip insert --track <t> ...`
    pub fn usage(&self) -> String {
        let words = self.name.replace('.', " ");
        if self.args.is_empty() {
            format!("fin {words}")
        } else {
            format!("fin {words} {}", self.args)
        }
    }
}

pub(crate) fn schema_of<T: JsonSchema>(name: &str) -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| json!({}));
    if let Some(map) = schema.as_object_mut() {
        map.insert("title".to_string(), json!(name));
    }
    schema
}

/// Adds "exactly one of `a` and `b`" to an object schema.
pub(crate) fn one_of(mut schema: Value, a: &str, b: &str) -> Value {
    schema["oneOf"] = json!([{ "required": [a] }, { "required": [b] }]);
    schema
}

fn none() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {},
        "additionalProperties": false,
    })
}

/// Commands a `batch` line may name.
pub const BATCHABLE: &[&str] = &["asset.add", "clip.insert", "clip.trim", "clip.delete"];

/// One `batch` line.
fn batch_line() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "batch",
        "description": "One JSONL line on stdin; args follow `fin schema <cmd>`.",
        "type": "object",
        "required": ["cmd"],
        "properties": { "cmd": { "enum": BATCHABLE }, "args": { "type": "object" } },
        "additionalProperties": false,
    })
}

pub const COMMANDS: &[Command] = &[
    Command {
        name: "project.create",
        group: "project",
        args: "<dir> --name <s> [--rate <fps>=30]",
        summary: "create a project (video t1, audio t2)",
        write: true,
        schema: || schema_of::<project::CreateArgs>("project.create"),
    },
    Command {
        name: "project.open",
        group: "project",
        args: "<dir> [--relocate]",
        summary: "register a project folder with the server (not a write; no --dry-run)",
        write: false,
        schema: || schema_of::<project::OpenArgs>("project.open"),
    },
    Command {
        name: "project.status",
        group: "project",
        args: "",
        summary: "storage: lease, read-only reason, branches, export, warnings",
        write: false,
        schema: || schema_of::<project::NoArgs>("project.status"),
    },
    Command {
        name: "project.takeover",
        group: "project",
        args: "[--confirm]",
        summary: "take the lease from a stopped server (--confirm on sync folders)",
        write: false,
        schema: || schema_of::<project::TakeoverArgs>("project.takeover"),
    },
    Command {
        name: "project.verify",
        group: "project",
        args: "",
        summary: "check the database and folder, change nothing (VERIFY_FAILED, exit 3)",
        write: false,
        schema: || schema_of::<project::NoArgs>("project.verify"),
    },
    Command {
        name: "project.rebuild",
        group: "project",
        args: "",
        summary: "rebuild the database from the folder (works with no server)",
        write: false,
        schema: || schema_of::<project::NoArgs>("project.rebuild"),
    },
    Command {
        name: "project.compact",
        group: "project",
        args: "",
        summary: "pack old segment files now (history is kept)",
        write: false,
        schema: || schema_of::<project::NoArgs>("project.compact"),
    },
    Command {
        name: "project.recover",
        group: "project",
        args: "[--create <group|all>] [--import <dir>] [--dismiss <group>]",
        summary: "list set-aside record groups; --create makes recovered-* branches; --import reads moved-out .fseg files; --dismiss drops an unrecoverable group",
        write: true,
        schema: || schema_of::<project::RecoverArgs>("project.recover"),
    },
    Command {
        name: "project.acknowledge",
        group: "project",
        args: "(--rewind <n>|--salvage <copy>)",
        summary: "clear a REWOUND or SALVAGE_INCOMPLETE warning",
        write: false,
        schema: || {
            one_of(
                schema_of::<project::AcknowledgeArgs>("project.acknowledge"),
                "rewind",
                "salvage",
            )
        },
    },
    Command {
        name: "status",
        group: "read",
        args: "",
        summary: "summary: branch, head, lease, read-only, exports, branches, timelines, fonts, warnings (lists cut at 20, *Next names the ls)",
        write: false,
        schema: || schema_of::<view::StatusArgs>("status"),
    },
    Command {
        name: "look",
        group: "read",
        args: "--at <t>[,<t>…] [--out <dir>] [--size WxH] [--no-wait]",
        summary: "PNG frames of a timeline at times (server only; layers left out are in skipped)",
        write: false,
        schema: || schema_of::<look::LookArgs>("look"),
    },
    Command {
        name: "view",
        group: "read",
        args: "[--track <t>] [--range <a..b>] [--budget <tokens> | --full] [--offset <n>] [--detail]",
        summary: "screenplay lines of every timeline (or --timeline's); default a 4000-token page (elided tail next), --budget fits",
        write: false,
        schema: || schema_of::<view::ViewArgs>("view"),
    },
    Command {
        name: "branch.new",
        group: "branch",
        args: "<name> [--from <b>[@<seq|checkpoint>]]",
        summary: "fork a branch (default: current branch at its head)",
        write: true,
        schema: || schema_of::<branch::NewArgs>("branch.new"),
    },
    Command {
        name: "branch.ls",
        group: "branch",
        args: "[--all]",
        summary: "list branches (--all: archived too)",
        write: false,
        schema: || schema_of::<branch::LsArgs>("branch.ls"),
    },
    Command {
        name: "branch.rename",
        group: "branch",
        args: "<old> <new>",
        summary: "rename a branch",
        write: true,
        schema: || schema_of::<branch::RenameArgs>("branch.rename"),
    },
    Command {
        name: "branch.archive",
        group: "branch",
        args: "<b> [--restore [--as <new>]]",
        summary: "archive a branch, or restore it",
        write: true,
        schema: || schema_of::<branch::ArchiveArgs>("branch.archive"),
    },
    Command {
        name: "branch.default",
        group: "branch",
        args: "[<b>|--clear]",
        summary: "show/set/clear the branch used without --branch",
        write: false,
        schema: || schema_of::<branch::DefaultArgs>("branch.default"),
    },
    Command {
        name: "checkpoint",
        group: "history",
        args: "<name>",
        summary: "name the branch head",
        write: true,
        schema: || schema_of::<history::CheckpointArgs>("checkpoint"),
    },
    Command {
        name: "undo",
        group: "history",
        args: "[--to <checkpoint>]",
        summary: "undo your last edit as $FIN_AUTHOR (default fin), or all since a checkpoint",
        write: true,
        schema: || schema_of::<history::UndoArgs>("undo"),
    },
    Command {
        name: "redo",
        group: "history",
        args: "",
        summary: "redo your last undo",
        write: true,
        schema: || schema_of::<project::NoArgs>("redo"),
    },
    Command {
        name: "merge",
        group: "history",
        args: "<branch> [--into <b>] [--skip-conflicting] [--include <txId>…] [--up-to <seq>] [--conflicts-from <n>]",
        summary: "merge a branch (CONFLICT lists 20 conflicts; conflictsNext pages)",
        write: true,
        schema: || schema_of::<history::MergeArgs>("merge"),
    },
    Command {
        name: "cherry-pick",
        group: "history",
        args: "<txId> [--into <b>]",
        summary: "copy one record onto a branch",
        write: true,
        schema: || schema_of::<history::CherryPickArgs>("cherry-pick"),
    },
    Command {
        name: "drop",
        group: "history",
        args: "<txId>… --from <b> [--into <b>] [--undo]",
        summary: "keep a rejected change out of every merge into a branch (--undo brings it back)",
        write: true,
        schema: || schema_of::<drop::DropArgs>("drop"),
    },
    Command {
        name: "review.request",
        group: "review",
        args: "[--branch <b>] [--into <b>] --title <t> [--summary <s>]",
        summary: "ask for a review of a branch (the open one comes back when there is one)",
        write: true,
        schema: || schema_of::<review::RequestArgs>("review.request"),
    },
    Command {
        name: "review.ls",
        group: "review",
        args: "[--state <s>] [--kind <k>]",
        summary: "the project's review items",
        write: false,
        schema: || schema_of::<review::LsArgs>("review.ls"),
    },
    Command {
        name: "review.show",
        group: "review",
        args: "<id>",
        summary: "an item with its change list, decisions and comments (stale says a head moved)",
        write: false,
        schema: || schema_of::<review::IdArgs>("review.show"),
    },
    Command {
        name: "review.refresh",
        group: "review",
        args: "<id>",
        summary: "re-plan the change list at the current heads; decisions stay",
        write: true,
        schema: || schema_of::<review::IdArgs>("review.refresh"),
    },
    Command {
        name: "review.comment",
        group: "review",
        args: "<id> <text> [--tx <txId>] [--at <t>]",
        summary: "comment on an item, or on one of its transactions",
        write: true,
        schema: || schema_of::<review::CommentArgs>("review.comment"),
    },
    Command {
        name: "review.decide",
        group: "review",
        args: "<id> (--accept <txId>,… | --reject <txId>,… | --pending <txId>,…) [--reason <r>] [--request-changes] [--expect <json>]",
        summary: "mark transactions; a rejection drops it from every merge, accepting a dropped one lifts the drop (undropped lists drops of others cancelled; CONFLICT droppedByOther when another reviewer dropped it; REVIEW_STALE on an --expect mismatch or dropUnknown)",
        write: true,
        schema: || schema_of::<review::DecideArgs>("review.decide"),
    },
    Command {
        name: "review.merge",
        group: "review",
        args: "<id> [--accept-pending] [--expect <json>] [--idem-key <k>]",
        summary: "merge only what was reviewed; a moved target merges and says targetMoved. REVIEW_UNDECIDED: pending (a count) under requireReview, nothingAccepted, or txIds acceptedDropped/rejectedUndropped; REVIEW_STALE: --expect mismatch, replan (with --accept-pending) or truncated. --idem-key applies to merge only",
        write: true,
        schema: || schema_of::<review::MergeArgs>("review.merge"),
    },
    Command {
        name: "review.close",
        group: "review",
        args: "<id> --rejected|--dismissed",
        summary: "close an item without merging it",
        write: true,
        schema: || schema_of::<review::CloseArgs>("review.close"),
    },
    Command {
        name: "revert",
        group: "history",
        args: "<mergeId>",
        summary: "undo a merge on the branch",
        write: true,
        schema: || schema_of::<history::RevertArgs>("revert"),
    },
    Command {
        name: "try",
        group: "try",
        args: "[name]",
        summary: "fork a try branch and switch to it",
        write: true,
        schema: || schema_of::<try_cmd::NameArgs>("try"),
    },
    Command {
        name: "try.keep",
        group: "try",
        args: "",
        summary: "merge the try branch back and switch back",
        write: true,
        schema: || schema_of::<try_cmd::NoArgs>("try.keep"),
    },
    Command {
        name: "try.drop",
        group: "try",
        args: "",
        summary: "archive the try branch and switch back",
        write: true,
        schema: || schema_of::<try_cmd::NoArgs>("try.drop"),
    },
    Command {
        name: "asset.add",
        group: "asset",
        args: "<path>",
        summary: "probe a file (media, image, SVG, dotLottie, font, LUT) and add it; binds $asset",
        write: true,
        schema: || schema_of::<asset::AddArgs>("asset.add"),
    },
    Command {
        name: "clip.insert",
        group: "clip",
        args: "--track <t> --at <time|after:cN|end> --asset <aN> [--in <time>] (--dur <time>|--out <time>) [--mode ripple|overwrite] [--no-audio] [--audio-track <t>]",
        summary: "insert a clip (ripple by default) with its linked audio; binds $clip ($audio, $link)",
        write: true,
        schema: || one_of(schema_of::<clip::InsertArgs>("clip.insert"), "dur", "out"),
    },
    Command {
        name: "clip.trim",
        group: "clip",
        args: "<cN> (--start <±time>|--end <±time>) [--mode ripple|roll|overwrite]",
        summary: "move one edge by a signed delta (ripple by default); echoes edgeAt",
        write: true,
        schema: || one_of(schema_of::<clip::TrimArgs>("clip.trim"), "start", "end"),
    },
    Command {
        name: "clip.delete",
        group: "clip",
        args: "<cN> [--mode ripple|lift]",
        summary: "delete a clip (ripple closes the gap, lift leaves it)",
        write: true,
        schema: || schema_of::<clip::DeleteArgs>("clip.delete"),
    },
    Command {
        name: "batch",
        group: "batch",
        args: "< JSONL {\"cmd\":\"clip.insert\",\"args\":{...}}",
        summary: "write commands as one tx; line n binds $clipN/$assetN",
        write: true,
        schema: batch_line,
    },
    Command {
        name: "help",
        group: "meta",
        args: "<group>",
        summary: "commands of a group with usage and input schemas",
        write: false,
        schema: none,
    },
    Command {
        name: "whoami",
        group: "meta",
        args: "",
        summary: "the principal this fin proves as (anonymous without a credential)",
        write: false,
        schema: none,
    },
    Command {
        name: "schema",
        group: "meta",
        args: "<cmd>",
        summary: "input JSON Schema of a command, e.g. clip.insert",
        write: false,
        schema: none,
    },
    Command {
        name: "schema.param",
        group: "meta",
        args: "[--scope <item|effect|track|timeline|marker|style|kind>] [--path <prefix>]",
        summary: "the settings registry: types, ranges, units, defaults",
        write: false,
        schema: none,
    },
];

/// Groups listed after the edit commands.
const LAST: &[&str] = &["batch", "meta"];

/// Every command: the 4B ones, then the 4C edits and settings, the 8A plugin and registry
/// commands, the 5C-2 job commands, the 5D library commands, the 7B approval commands, then
/// `batch`, `help` and `schema`.
pub fn commands() -> impl Iterator<Item = &'static Command> {
    let head = COMMANDS.iter().filter(|c| !LAST.contains(&c.group));
    let tail = COMMANDS.iter().filter(|c| LAST.contains(&c.group));
    head.chain(crate::registry_edit::EDIT_COMMANDS)
        .chain(crate::registry_settings::SETTINGS_COMMANDS)
        .chain(crate::registry_inspect::INSPECT_COMMANDS)
        .chain(crate::registry_plugins::PLUGIN_COMMANDS)
        .chain(crate::registry_plugins::JOB_COMMANDS)
        .chain(crate::registry_library::LIBRARY_COMMANDS)
        .chain(crate::registry_plugins::APPROVAL_COMMANDS)
        .chain(crate::registry_plugins::AGENT_COMMANDS)
        .chain(tail)
}

/// Group names in registry order, without duplicates.
pub fn groups() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for c in commands() {
        if !out.contains(&c.group) {
            out.push(c.group);
        }
    }
    out
}

/// Finds a command by dotted name.
pub fn find(name: &str) -> Option<&'static Command> {
    commands().find(|c| c.name == name)
}

pub fn names() -> Vec<&'static str> {
    commands().map(|c| c.name).collect()
}
