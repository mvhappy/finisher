//! `fin schema --registry` (plan 7B Task 1, spec §7.2): the exported command registry the
//! server reads to name an incoming RPC back to its command, and the committed copy of it.

mod support;

use serde_json::Value;
use support::{fin, real_tempdir, repo_root};

fn registry() -> Value {
    let (_guard, dir) = real_tempdir();
    let r = fin(&dir, &dir, &["schema", "--registry"]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    r.json
}

fn command<'a>(reg: &'a Value, name: &str) -> &'a Value {
    reg["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no command {name}"))
}

#[test]
fn registry_lists_every_command() {
    let (_guard, dir) = real_tempdir();
    let index = fin(&dir, &dir, &["schema", "commands"]).json;
    let reg = registry();
    assert_eq!(reg["version"], 1);
    let names: Vec<&str> = reg["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    let listed: Vec<&str> = index["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    // Every command in registry order, then the plugin-command pattern.
    assert_eq!(&names[..listed.len()], &listed[..]);
    assert_eq!(names.len(), listed.len() + 1);
    let plugin = command(&reg, "plugin:<id>/<command>");
    assert_eq!(plugin["dynamic"], true);
    // Named from the plugin host's own record of the caller, never from the payload.
    assert_eq!(
        plugin["rpcs"],
        serde_json::json!([{ "method": "tx.commit" }])
    );
    // The plugin manager's own commands keep their plugin.* names.
    command(&reg, "plugin.install");
}

#[test]
fn registry_entries_carry_rpcs_identity_write_and_arg_kinds() {
    let reg = registry();
    let trim = command(&reg, "item.trim");
    assert_eq!(trim["group"], "item");
    assert_eq!(trim["write"], true);
    assert_eq!(trim["rpcs"][0]["method"], "tx.commit");
    assert_eq!(trim["rpcs"][0]["match"]["tx.ops.*.op"], "item.trim");
    let undo = command(&reg, "undo");
    assert_eq!(
        undo["rpcs"][0],
        serde_json::json!({ "method": "history.undo" })
    );
    assert!(undo["identity"].as_array().unwrap().contains(&"to".into()));
    let rename = command(&reg, "branch.rename");
    assert_eq!(
        rename["rpcs"][0]["match"],
        serde_json::json!({ "name": { "present": true } })
    );
    // The `clip.*` alias ops name what their `item.*` ops name.
    let ops = |name: &str| -> Vec<Value> {
        command(&reg, name)["rpcs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["match"]["tx.ops.*.op"].clone())
            .collect()
    };
    assert_eq!(ops("item.trim"), ["item.trim", "clip.trim"]);
    assert_eq!(ops("clip.delete"), ["item.delete", "clip.delete"]);
    // Any plan that is not an update is an install's.
    let install: Vec<&Value> = command(&reg, "plugin.install")["rpcs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["method"] == "plugin.plan")
        .map(|r| &r["match"]["update"])
        .collect();
    assert_eq!(
        install,
        [
            &serde_json::json!({ "present": false }),
            &serde_json::json!(false)
        ]
    );
    // `view` reads a snapshot, as every edit command does for its base: it names no RPC.
    assert_eq!(command(&reg, "view")["rpcs"], serde_json::json!([]));
    // A client-side command sends no RPC.
    assert_eq!(command(&reg, "help")["rpcs"], serde_json::json!([]));
    // Path arguments are the same as `fin schema <cmd>`'s argKinds.
    assert_eq!(command(&reg, "asset.add")["argKinds"]["path"], "path");
}

#[test]
fn registry_rejects_other_arguments() {
    let (_guard, dir) = real_tempdir();
    let r = fin(&dir, &dir, &["schema", "--registry", "clip.insert"]);
    assert_eq!(r.json["error"]["code"], "USAGE");
}

/// The committed `packages/contracts/registry.json` is exactly what this binary prints (the
/// gate `scripts/check-registry.mjs` runs the same comparison).
#[test]
fn registry_is_committed() {
    let committed = std::fs::read_to_string(repo_root().join("packages/contracts/registry.json"))
        .expect("packages/contracts/registry.json is committed");
    let mut want = serde_json::to_string_pretty(&registry()).unwrap();
    want.push('\n');
    assert_eq!(
        committed, want,
        "regenerate with `node scripts/check-registry.mjs --write`"
    );
}
