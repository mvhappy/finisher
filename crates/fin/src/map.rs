//! `fin` alone: a one-screen map of every command, the global flags and the conventions an
//! agent needs (kept under 1.5k tokens; asserted in tests).

use serde_json::{Value, json};

use crate::registry::{COMMANDS, groups};

pub fn map() -> Value {
    let commands: Vec<String> = COMMANDS.iter().map(|c| format!("{}  # {}", c.usage(), c.summary)).collect();
    json!({
        "fin": "agent-only Finisher CLI; stdout is one JSON document, stderr is logs",
        "groups": groups(),
        "commands": commands,
        "globals": [
            "--project <dir>  # default: nearest ancestor with finisher.project.json",
            "--dry-run  # validate and show the result without committing (any write)",
            "--idem-key <k>  # retry-safe writes; becomes txId",
        ],
        "time": "12.4s | 00:12.4 | 01:00:12.4 | f300 (frames) | 62/5; trim deltas are signed: +0.5s, -1s",
        "write": "{rev, txId, bindings, diff, resolved}; --dry-run: {dryRun:true, txId, bindings, diff, resolved}",
        "error": "{error:{code, message, details, fix}}",
        "exit": "0 ok, 2 USAGE, 3 validation, 4 CONFLICT, 5 SERVER_UNAVAILABLE, 6 internal",
        "more": "fin help <group>; fin schema <cmd>",
    })
}
