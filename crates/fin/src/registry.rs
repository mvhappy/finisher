//! The one command registry: names, groups, usage lines, summaries and input schemas. The map,
//! `fin help` and `fin schema` are all generated from it.

use schemars::JsonSchema;
use serde_json::{Value, json};

use crate::commands::{asset, clip, project, view};

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
        if self.args.is_empty() { format!("fin {words}") } else { format!("fin {words} {}", self.args) }
    }
}

fn schema_of<T: JsonSchema>(name: &str) -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| json!({}));
    if let Some(map) = schema.as_object_mut() {
        map.insert("title".to_string(), json!(name));
    }
    schema
}

/// Adds "exactly one of `a` and `b`" to an object schema.
fn one_of(mut schema: Value, a: &str, b: &str) -> Value {
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
        args: "<dir>",
        summary: "open an existing project folder",
        write: false,
        schema: || schema_of::<project::OpenArgs>("project.open"),
    },
    Command {
        name: "status",
        group: "read",
        args: "",
        summary: "rev, rate, tracks (clip count, end), asset ids",
        write: false,
        schema: || schema_of::<view::StatusArgs>("status"),
    },
    Command {
        name: "view",
        group: "read",
        args: "[--track <t>] [--range <a..b>]",
        summary: "clips per track: id, start, end, asset, sourceIn",
        write: false,
        schema: || schema_of::<view::ViewArgs>("view"),
    },
    Command {
        name: "asset.add",
        group: "asset",
        args: "<path>",
        summary: "probe a media file and add it; binds $asset",
        write: true,
        schema: || schema_of::<asset::AddArgs>("asset.add"),
    },
    Command {
        name: "clip.insert",
        group: "clip",
        args: "--track <t> --at <time|after:cN|end> --asset <aN> [--in <time>] (--dur <time>|--out <time>)",
        summary: "insert a clip, ripple later clips; binds $clip",
        write: true,
        schema: || one_of(schema_of::<clip::InsertArgs>("clip.insert"), "dur", "out"),
    },
    Command {
        name: "clip.trim",
        group: "clip",
        args: "<cN> (--start <±time>|--end <±time>)",
        summary: "move one edge by a signed delta, ripple",
        write: true,
        schema: || one_of(schema_of::<clip::TrimArgs>("clip.trim"), "start", "end"),
    },
    Command {
        name: "clip.delete",
        group: "clip",
        args: "<cN>",
        summary: "delete a clip and close the gap",
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
        name: "schema",
        group: "meta",
        args: "<cmd>",
        summary: "input JSON Schema of a command, e.g. clip.insert",
        write: false,
        schema: none,
    },
];

/// Group names in registry order, without duplicates.
pub fn groups() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for c in COMMANDS {
        if !out.contains(&c.group) {
            out.push(c.group);
        }
    }
    out
}

/// Finds a command by dotted name.
pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

pub fn names() -> Vec<&'static str> {
    COMMANDS.iter().map(|c| c.name).collect()
}
