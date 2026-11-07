//! `fin` alone: a one-screen map of every command name by group, the global flags and the
//! conventions an agent needs (kept under 1.5k tokens; asserted in tests). Usage lines are in
//! `fin help <group>`.

use serde_json::{Value, json};

use crate::registry::{commands, groups};

/// `"<group>: <name> <name>…"` per group, names without the group prefix.
fn names_by_group() -> Vec<String> {
    groups()
        .into_iter()
        .map(|g| {
            let prefix = format!("{g}.");
            let names: Vec<&str> = commands()
                .filter(|c| c.group == g)
                .map(|c| c.name.strip_prefix(&prefix).unwrap_or(c.name))
                .collect();
            format!("{g}: {}", names.join(" "))
        })
        .collect()
}

pub fn map() -> Value {
    json!({
        "fin": "agent-only Finisher CLI; stdout is one JSON document, stderr is logs",
        "groups": groups(),
        "commands": names_by_group(),
        "usage": "fin <group> <name> ...; fin help <group> gives each command's usage, flags and input schema",
        "globals": [
            "--project <dir>  # default: nearest ancestor with finisher.project.json",
            "--branch <b>  # branch to read/write (default: `fin try` switch, else main)",
            "--timeline <tl>  # timeline for track add / marker add (default: the first)",
            "--dry-run  # validate and show the result without committing (any write)",
            "--idem-key <k>  # retry-safe writes; becomes txId",
        ],
        "time": "12.4s | 00:12.4 | 01:00:12.4 | f300 (frames) | 62/5; deltas are signed: +0.5s, -1s",
        "modes": "ripple (default for clips) | overwrite (default for other kinds; lift for deletes) | roll (trims); echoed as resolved.mode",
        "write": "{rev, txId, bindings, diff, resolved}; --dry-run: {dryRun:true, txId, bindings, diff, resolved}",
        "error": "{error:{code, message, details, fix}}",
        "exit": "0 ok, 2 USAGE, 3 validation, 4 CONFLICT, 5 SERVER_UNAVAILABLE, 6 internal",
        "more": "fin help <group>; fin schema <cmd>",
    })
}
