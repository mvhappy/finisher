//! `fin schema --registry`: every command's name, group, the RPC(s) it sends and the payload
//! fields that identify it, from the one registry (plan 7B Decision 1, spec §7.2). The output
//! is committed as `packages/contracts/registry.json` (drift-gated by `check-registry.mjs`) and
//! read by the server, which names each incoming RPC back to its command.

use serde_json::{Value, json};

use crate::registry::{Command, commands};
use crate::registry_rpcs::{Match, Row, RpcUse};
use crate::registry_rpcs_edit::{EDIT_ROWS, PLUGIN_COMMAND, PLUGIN_COMMAND_RPC};

pub const VERSION: u32 = 1;

/// The row of a command, from either table.
pub fn row_of(name: &str) -> Option<&'static Row> {
    crate::registry_rpcs::ROWS
        .iter()
        .chain(EDIT_ROWS)
        .find(|r| r.name == name)
}

fn rpc(r: &RpcUse) -> Value {
    let mut v = json!({ "method": r.method });
    if !r.matches.is_empty() {
        let m: serde_json::Map<String, Value> = r
            .matches
            .iter()
            .map(|(k, m): &(&str, Match)| ((*k).to_string(), m.to_json()))
            .collect();
        v["match"] = Value::Object(m);
    }
    v
}

fn entry(c: &Command) -> Value {
    let row = row_of(c.name);
    let rpcs: Vec<Value> = row
        .map(|r| r.rpcs.iter().map(rpc).collect())
        .unwrap_or_default();
    let identity: Vec<&str> = row.map(|r| r.identity.to_vec()).unwrap_or_default();
    let mut kinds = serde_json::Map::new();
    for a in crate::agent_paths::path_args(c.name) {
        kinds.insert(a.to_string(), json!("path"));
    }
    json!({
        "name": c.name,
        "group": c.group,
        "rpcs": rpcs,
        "identity": identity,
        "write": c.write,
        "argKinds": kinds,
    })
}

/// The plugin commands' entry: a name pattern, composed by the server from the plugin host's
/// record of the caller (never the payload's `tx.via`).
fn plugin_entry() -> Value {
    json!({
        "name": PLUGIN_COMMAND,
        "group": "plugin-command",
        "rpcs": [rpc(&PLUGIN_COMMAND_RPC)],
        "identity": ["projectId", "tx.branch", "tx.ops"],
        "write": true,
        "argKinds": {},
        "dynamic": true,
    })
}

/// `{version, commands}`: the commands in registry order, then the plugin-command pattern.
pub fn registry() -> Value {
    let mut list: Vec<Value> = commands().map(entry).collect();
    list.push(plugin_entry());
    json!({ "version": VERSION, "commands": list })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_one_row_and_every_row_a_command() {
        let names: Vec<&str> = commands().map(|c| c.name).collect();
        for n in &names {
            let rows = crate::registry_rpcs::ROWS
                .iter()
                .chain(EDIT_ROWS)
                .filter(|r| r.name == *n)
                .count();
            assert_eq!(rows, 1, "{n} needs exactly one row in registry_rpcs*.rs");
        }
        for r in crate::registry_rpcs::ROWS.iter().chain(EDIT_ROWS) {
            assert!(names.contains(&r.name), "row {} names no command", r.name);
        }
    }

    #[test]
    fn op_conditions_name_real_ops() {
        for r in EDIT_ROWS {
            for u in r.rpcs {
                for (field, m) in u.matches {
                    if let (&"tx.ops.*.op", Match::Eq(op)) = (field, m) {
                        assert!(fin_core::Op::NAMES.contains(op), "{}: no op {op}", r.name);
                    }
                }
            }
        }
    }

    /// The ops of the edit rows' `tx.ops.*.op` conditions.
    fn named_ops() -> Vec<&'static str> {
        let mut ops = Vec::new();
        for r in EDIT_ROWS {
            for u in r.rpcs {
                for (field, m) in u.matches {
                    if let (&"tx.ops.*.op", Match::Eq(op)) = (field, m) {
                        ops.push(*op);
                    }
                }
            }
        }
        ops
    }

    /// Every client op fin-core takes on the wire is some command's, the `clip.*` aliases
    /// included (fin-core rewrites them only in the engine, so the server sees them as sent).
    #[test]
    fn every_client_op_is_named_by_a_row() {
        // Internal ops: produced as inverses, refused from clients (`Op::is_internal`).
        const INTERNAL: &[&str] = &[
            "caption.setWords",
            "effect.restore",
            "item.remove",
            "item.restore",
            "timeline.restore",
            "track.restore",
        ];
        let named = named_ops();
        for op in fin_core::Op::NAMES {
            if !INTERNAL.contains(op) {
                assert!(named.contains(op), "no row names the op {op}");
            }
        }
    }

    #[test]
    fn alias_ops_name_what_their_item_ops_name() {
        let mut aliases = 0;
        for op in fin_core::Op::NAMES {
            let parsed: Result<fin_core::Op, _> = serde_json::from_value(alias_sample(op));
            let Ok(parsed) = parsed else { continue };
            if !parsed.is_alias() {
                continue;
            }
            aliases += 1;
            let named = |o: &str| -> Vec<&str> {
                EDIT_ROWS
                    .iter()
                    .filter(|r| {
                        r.rpcs.iter().any(|u| {
                            u.matches.iter().any(|(f, m)| {
                                matches!((f, m), (&"tx.ops.*.op", Match::Eq(x)) if *x == o)
                            })
                        })
                    })
                    .map(|r| r.name)
                    .collect()
            };
            let by_alias = named(op);
            assert!(!by_alias.is_empty(), "no row names the alias {op}");
            if *op != "clip.insert" {
                let item = op.replace("clip.", "item.");
                assert_eq!(
                    by_alias,
                    named(&item),
                    "{op} and {item} name different commands"
                );
            }
        }
        assert_eq!(
            aliases, 3,
            "the clip.* aliases are clip.insert, clip.trim, clip.delete"
        );
    }

    /// A wire op of the given name with the aliases' fields.
    fn alias_sample(op: &str) -> Value {
        json!({
            "op": op, "id": "$c", "track": "t1", "at": "0", "asset": "a1",
            "sourceIn": "0", "duration": "1", "clip": "c1", "edge": "start", "delta": "1",
        })
    }

    #[test]
    fn write_commands_commit_a_tx_or_send_a_write_rpc() {
        for c in commands().filter(|c| c.write) {
            let row = row_of(c.name).unwrap();
            assert!(!row.rpcs.is_empty(), "{} writes but sends no RPC", c.name);
        }
    }
}
