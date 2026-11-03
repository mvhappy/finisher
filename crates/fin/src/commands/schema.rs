//! `fin help <group>` and `fin schema <cmd>`, generated from the registry.

use fin_core::CoreError;
use serde_json::{Value, json};

use crate::output::usage;
use crate::registry::{COMMANDS, find, groups, names};

fn entry(c: &crate::registry::Command) -> Value {
    json!({ "name": c.name, "usage": c.usage(), "summary": c.summary, "write": c.write, "input": (c.schema)() })
}

/// The commands of a group (or a single command by name), with usage and input schema.
pub fn help(topic: &str) -> Result<Value, CoreError> {
    let commands: Vec<Value> = COMMANDS.iter().filter(|c| c.group == topic).map(entry).collect();
    if !commands.is_empty() {
        return Ok(json!({ "group": topic, "commands": commands }));
    }
    if let Some(c) = find(&topic.replace(' ', ".")) {
        return Ok(json!({ "group": c.group, "commands": [entry(c)] }));
    }
    Err(usage(
        format!("no help group or command named {topic:?}"),
        json!({ "topic": topic, "alternatives": groups() }),
        "run `fin help <group>` with one of the listed groups",
    ))
}

/// The input JSON Schema of a command: `fin schema clip.insert` or `fin schema clip insert`.
pub fn schema(words: &[String]) -> Result<Value, CoreError> {
    let name = words.join(".");
    match find(&name) {
        Some(c) => Ok((c.schema)()),
        None => Err(usage(
            if name.is_empty() { "schema needs a command name".to_string() } else { format!("unknown command {name:?}") },
            json!({ "command": name, "alternatives": names() }),
            "run `fin schema <cmd>` with one of the listed commands, e.g. clip.insert",
        )),
    }
}
