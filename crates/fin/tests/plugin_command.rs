//! `fin @<id> <command>`, `fin help @<id>` and `fin plugin commands` (sp8b Task 10, spec §3.8)
//! against a real `finisherd`. A registry built by the test-hooks fin-node publishes `@acme/tools`,
//! a code plugin whose program is the test probe (`apps/finisherd/test/fixtures/plugins/probe`);
//! the console installs it (accepting third-party code), and enables it; `fin` (a `local`
//! principal) runs its commands.

mod support;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use support::console::Console;
use support::fontserver::{test_native_dir, why_no_test_hooks};
use support::{Run, Server, fin_env, ok};

const ID: &str = "@acme/tools";

/// `look`'s and `edit`'s arguments: what the probe does, and one property per mapped type.
fn args_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "op": { "type": "string", "enum": ["command", "echo"] },
            "out": { "type": "string", "maxLength": 100000 },
            "count": { "type": "integer", "minimum": 0, "maximum": 100 },
            "gain": { "type": "number" },
            "loud": { "type": "boolean" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "frames": { "type": "array", "items": { "type": "integer" } },
            "box": {
                "type": "object",
                "properties": { "w": { "type": "integer" } },
                "additionalProperties": false,
            },
            "sleepMs": { "type": "integer", "minimum": 0 },
        },
        "required": ["op"],
        "additionalProperties": false,
    })
}

fn package() -> Value {
    let probe = support::repo_root().join("apps/finisherd/test/fixtures/plugins/probe/main.mjs");
    let commands = json!([
        { "name": "edit", "title": "cmd.edit", "args": args_schema(), "writes": true },
        { "name": "look", "title": "cmd.look", "args": args_schema() },
    ]);
    json!({
        "id": ID, "version": "1.0.0", "kind": "code",
        "permissions": [{ "perm": "project:read" }, { "perm": "project:write" }],
        "manifest": {
            "hostApi": "^1.1",
            "runtime": { "type": "node", "main": "dist/plugin.mjs", "node": ">=24.22" },
            "messages": "messages", "locales": ["en"],
            "contributes": { "commands": commands },
        },
        "files": {
            "dist/plugin.mjs": std::fs::read_to_string(probe).unwrap(),
            "messages/en.json": json!({ "cmd.edit": "Edit", "cmd.look": "Look" }).to_string(),
        },
    })
}

/// A `file:` URL for an absolute path (Windows' verbatim prefix dropped).
fn url_of(path: &Path) -> String {
    let p = path.display().to_string();
    let p = p.strip_prefix(r"\\?\").unwrap_or(&p).replace('\\', "/");
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

/// Builds registry `team` publishing [`package`]; returns its folder and root SHA-256.
fn build_registry(root: &Path) -> (PathBuf, String) {
    let dir = root.join("registry-team");
    let keys = root.join("keys-team");
    let index = support::repo_root().join("packages/fin-node/index.js");
    let script = format!(
        "const {{ testHooks }} = await import({});\
         process.stdout.write(testHooks.buildRegistry(process.argv[1], process.argv[2], process.argv[3]));",
        json!(url_of(&index))
    );
    let out = Command::new("node")
        .args(["--input-type=module", "-e", &script])
        .arg(&dir)
        .arg(&keys)
        .arg(json!({ "packages": [package()] }).to_string())
        .env("FIN_NODE_BINARY_DIR", test_native_dir())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let built: Value = serde_json::from_slice(&out.stdout).unwrap();
    (dir, built["rootSha256"].as_str().unwrap().to_string())
}

struct Rig {
    server: Server,
    project: PathBuf,
    cache: PathBuf,
}

impl Rig {
    /// A test-hooks server with `@acme/tools` installed and enabled, and a project `film`.
    fn start(test: &str) -> Option<Rig> {
        let env: Vec<(String, OsString)> =
            vec![("FIN_NODE_BINARY_DIR".into(), test_native_dir().into())];
        let server = Server::start_with(test, env, why_no_test_hooks())?;
        let (dir, sha) = build_registry(&server.root);
        let mut console = Console::open(&server);
        console.confirmed_ok(
            "registry.add",
            json!({ "cfg": { "id": "team", "path": dir }, "root": { "kind": "sha256", "sha256": sha } }),
        );
        let refreshed = console.call("registry.refresh", json!({ "id": "team" }));
        assert_eq!(refreshed["_tag"], "Success", "{refreshed}");
        let plan = console.call("plugin.plan", json!({ "id": ID, "version": "1.0.0" }));
        assert_eq!(plan["_tag"], "Success", "{plan}");
        let plan = &plan["value"];
        console.confirmed_ok(
            "plugin.install",
            json!({ "planId": plan["planId"], "digest": plan["digest"], "accept": { "thirdPartyCode": true } }),
        );
        console.confirmed_ok("plugin.enable", json!({ "id": ID }));
        let cache = server.root.join("fin-cache");
        let rig = Rig {
            project: server.root.join("film"),
            server,
            cache,
        };
        ok(rig.fin_at(
            &rig.server.root,
            &["project", "create", "film", "--name", "Film"],
        ));
        Some(rig)
    }

    fn fin_at(&self, cwd: &Path, args: &[&str]) -> Run {
        let env: [(&str, &OsStr); 1] = [("FIN_CACHE_DIR", self.cache.as_os_str())];
        fin_env(&self.server.sd, cwd, args, &env)
    }

    fn fin(&self, args: &[&str]) -> Run {
        self.fin_at(&self.project, args)
    }

    /// The one cache file `fin` keeps for this server.
    fn cache_files(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.cache.join("plugin-commands"))
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    }
}

fn error_code(run: &Run) -> &str {
    run.json["error"]["code"].as_str().unwrap_or("")
}

#[test]
fn fin_runs_plugin_command() {
    let Some(rig) = Rig::start("fin_runs_plugin_command") else {
        return;
    };
    // A data command.
    let out = r#"{"kind":"data","value":{"hi":1}}"#;
    let data = ok(rig.fin(&[ID, "look", "--op", "command", "--out", out]));
    assert_eq!(data, json!({ "outcome": "data", "data": { "hi": 1 } }));

    // A writing command: its transaction is committed (and a dry run commits nothing).
    let tx = json!({ "kind": "tx", "ops": [
        { "op": "marker.add", "id": "$m", "timeline": "tl1", "at": "1", "label": "from plugin" }
    ] })
    .to_string();
    let dry = ok(rig.fin(&[ID, "edit", "--op", "command", "--out", &tx, "--dry-run"]));
    assert_eq!(
        (dry["outcome"].clone(), dry["dryRun"].clone()),
        (json!("data"), json!(true)),
        "{dry}"
    );
    let rev = dry["rev"].as_u64().unwrap();
    let done = ok(rig.fin(&[
        "--branch", "main", ID, "edit", "--op", "command", "--out", &tx,
    ]));
    assert_eq!(done["outcome"], "accepted", "{done}");
    assert_eq!(done["rev"], rev + 1, "{done}");
    assert!(
        done["txId"].as_str().is_some_and(|t| !t.is_empty()),
        "{done}"
    );
    let view = ok(rig.fin(&["view"]));
    assert!(view.to_string().contains("from plugin"), "{view}");

    // The server's refusals keep their exits: arguments (3), a refused transaction (3).
    let bad = rig.fin(&[ID, "look", "--op", "nope"]);
    assert_eq!(
        (bad.code, error_code(&bad)),
        (3, "PLUGIN_COMMAND_ARGS"),
        "{}",
        bad.stdout
    );
    let refused = rig.fin(&[ID, "look", "--op", "command", "--out", &tx]);
    assert_eq!(
        (refused.code, error_code(&refused)),
        (3, "PLUGIN_TX_INVALID"),
        "{}",
        refused.stdout
    );
    // The command line itself is fin's to refuse (2), before any RPC.
    let usage = rig.fin(&[ID, "look", "--op", "echo", "--count", "x"]);
    assert_eq!(
        (usage.code, error_code(&usage)),
        (2, "USAGE"),
        "{}",
        usage.stdout
    );
    assert_eq!(
        rig.fin(&[ID, "look", "--op", "echo", "--idem-key", "k"])
            .code,
        2
    );
    // Outside a project there is nothing to run it on.
    let nowhere = rig.fin_at(&rig.server.root, &[ID, "look", "--op", "echo"]);
    assert_ne!(nowhere.code, 0, "{}", nowhere.stdout);
}

#[test]
fn help_lists_commands_from_cache() {
    let Some(mut rig) = Rig::start("help_lists_commands_from_cache") else {
        return;
    };
    assert!(rig.cache_files().is_empty());
    let help = ok(rig.fin(&["help", ID]));
    assert_eq!(help["source"], "server", "{help}");
    assert_eq!(help["plugin"], ID);
    let names: Vec<&str> = help["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["plugin:@acme/tools/edit", "plugin:@acme/tools/look"]
    );
    let edit = &help["commands"][0];
    assert_eq!(edit["writes"], true);
    assert_eq!(edit["title"], "cmd.edit");
    assert_eq!(edit["plugin"], "@acme/tools@1.0.0");
    let line = edit["usage"].as_str().unwrap();
    assert!(
        line.starts_with("fin @acme/tools edit --op <command|echo> "),
        "{line}"
    );
    assert!(
        line.contains("[--sleep-ms <integer>]") && line.contains("[--loud]"),
        "{line}"
    );
    assert!(line.ends_with("[--args-json '{\"box\": …}']"), "{line}");
    // One cache file, holding the list under its digest; `fin plugin commands` agrees.
    let files = rig.cache_files();
    assert_eq!(files.len(), 1, "{files:?}");
    let cached: Value = serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
    assert_eq!(cached["digest"], help["digest"]);
    assert_eq!(cached["digest"].as_str().unwrap().len(), 64);
    let all = ok(rig.fin(&["plugin", "commands"]));
    assert_eq!(all["commands"], help["commands"]);
    assert_eq!(ok(rig.fin(&[ID]))["commands"], help["commands"]);

    // With the server gone, help comes from the cache.
    rig.server.stop();
    let offline = ok(rig.fin(&["help", ID]));
    assert_eq!(offline["source"], "cache", "{offline}");
    assert_eq!(offline["commands"], help["commands"]);
    assert_eq!(offline["digest"], help["digest"]);
    // Without a cache it is the server's absence.
    std::fs::remove_dir_all(&rig.cache).unwrap();
    let none = rig.fin(&["help", ID]);
    assert_eq!(
        (none.code, error_code(&none)),
        (5, "SERVER_UNAVAILABLE"),
        "{}",
        none.stdout
    );
}

#[test]
fn unknown_command_lists_available() {
    let Some(rig) = Rig::start("unknown_command_lists_available") else {
        return;
    };
    let run = rig.fin(&[ID, "nope", "--op", "echo"]);
    assert_eq!(
        (run.code, error_code(&run)),
        (3, "PLUGIN_NOT_FOUND"),
        "{}",
        run.stdout
    );
    let d = &run.json["error"]["details"];
    assert_eq!(d["name"], "plugin:@acme/tools/nope");
    assert_eq!(d["available"], json!(["edit", "look"]));
    // An unknown plugin lists the plugins that serve commands.
    let other = rig.fin(&["@acme/other", "go"]);
    assert_eq!(
        (other.code, error_code(&other)),
        (3, "PLUGIN_NOT_FOUND"),
        "{}",
        other.stdout
    );
    assert_eq!(other.json["error"]["details"]["available"], json!([ID]));
    let help = rig.fin(&["help", "@acme/other"]);
    assert_eq!(
        (help.code, error_code(&help)),
        (3, "PLUGIN_NOT_FOUND"),
        "{}",
        help.stdout
    );
    // A stale cache naming a command the server no longer serves: the server's refusal stands.
    let file = &rig.cache_files()[0];
    let mut cached: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let mut ghost = cached["commands"][0].clone();
    ghost["name"] = json!("plugin:@acme/tools/ghost");
    ghost["command"] = json!("ghost");
    cached["commands"].as_array_mut().unwrap().push(ghost);
    std::fs::write(file, cached.to_string()).unwrap();
    let stale = rig.fin(&[ID, "ghost", "--op", "echo"]);
    assert_eq!(
        (stale.code, error_code(&stale)),
        (3, "PLUGIN_NOT_FOUND"),
        "{}",
        stale.stdout
    );
}

#[test]
fn schema_registry_contains_plugin_names() {
    let tmp = tempfile::tempdir().unwrap();
    let run = support::fin(
        &tmp.path().join("sd"),
        tmp.path(),
        &["schema", "--registry"],
    );
    assert_eq!(run.code, 0, "{}", run.stdout);
    let list = run.json["commands"].as_array().unwrap();
    let plugin = list
        .iter()
        .find(|c| c["name"] == "plugin:<id>/<command>")
        .expect("the plugin command pattern");
    assert_eq!(plugin["dynamic"], true);
    let methods: Vec<&str> = plugin["rpcs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["method"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["plugin.command.run", "tx.commit"]);
    // `fin plugin commands` (and `fin help @<id>`) is the command sending `plugin.commands`.
    let listing = list
        .iter()
        .find(|c| c["name"] == "plugin.commands")
        .unwrap();
    assert_eq!(listing["rpcs"], json!([{ "method": "plugin.commands" }]));
    // The committed copy the server reads is this output.
    let committed: Value = serde_json::from_slice(
        &std::fs::read(support::repo_root().join("packages/contracts/registry.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(committed, run.json);
}

#[test]
fn args_mapping_table() {
    let Some(rig) = Rig::start("args_mapping_table") else {
        return;
    };
    // The probe's `echo` answers the arguments it was given.
    let echo = |more: &[&str]| -> Run {
        let mut args = vec![ID, "look", "--op", "echo"];
        args.extend_from_slice(more);
        rig.fin(&args)
    };
    for (line, want) in [
        (vec![], json!({})),
        (vec!["--count", "3"], json!({ "count": 3 })),
        (vec!["--count=0"], json!({ "count": 0 })),
        (vec!["--gain", "-1.5"], json!({ "gain": -1.5 })),
        (vec!["--loud"], json!({ "loud": true })),
        (vec!["--loud=false"], json!({ "loud": false })),
        (
            vec!["--tags", "a", "--tags", "b c"],
            json!({ "tags": ["a", "b c"] }),
        ),
        (
            vec!["--frames", "1", "--frames", "2"],
            json!({ "frames": [1, 2] }),
        ),
        (vec!["--sleep-ms", "1"], json!({ "sleepMs": 1 })),
        (vec!["--sleepMs", "2"], json!({ "sleepMs": 2 })),
        (
            vec!["--args-json", r#"{"box":{"w":4}}"#],
            json!({ "box": { "w": 4 } }),
        ),
        (vec!["--out", "--x=y"], json!(null)),
    ] {
        let run = echo(&line);
        if want.is_null() {
            assert_eq!(run.code, 2, "{line:?}: {}", run.stdout);
            continue;
        }
        assert_eq!(run.code, 0, "{line:?}: {}", run.stdout);
        let mut expect = json!({ "op": "echo" });
        expect
            .as_object_mut()
            .unwrap()
            .extend(want.as_object().unwrap().clone());
        assert_eq!(run.json["data"], expect, "{line:?}");
    }
    // fin's own flags anywhere on the line; refusals by class.
    let anywhere = ok(rig.fin(&[ID, "look", "--count", "1", "--dry-run", "--op", "echo"]));
    assert_eq!(anywhere["data"], json!({ "op": "echo", "count": 1 }));
    for (line, exit, code) in [
        (vec!["--count", "x"], 2, "USAGE"),
        (vec!["--nope", "1"], 2, "USAGE"),
        (vec!["--box", "{}"], 2, "USAGE"),
        (vec!["--count", "1", "--count", "2"], 2, "USAGE"),
        (vec!["positional"], 2, "USAGE"),
        (vec!["--count", "101"], 3, "PLUGIN_COMMAND_ARGS"),
        (
            vec!["--args-json", r#"{"box":{"h":1}}"#],
            3,
            "PLUGIN_COMMAND_ARGS",
        ),
    ] {
        let run = echo(&line);
        assert_eq!(
            (run.code, error_code(&run)),
            (exit, code),
            "{line:?}: {}",
            run.stdout
        );
    }
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
