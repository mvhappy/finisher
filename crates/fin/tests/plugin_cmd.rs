//! `fin plugin …`, `fin registry …` and `fin style add --from @pub/pack:<name>` (plan 8A Task
//! 12c) against a real `finisherd`. The bundled `@finisher/captions-basic` pack is installed at
//! every start, so styles need no registry; the other tests build one with the test-hooks
//! fin-node (`testHooks.buildRegistry`), add it and install from it as the console, and run
//! `fin` (a `local` principal, not a person) against it.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use support::console::Console;
use support::fontserver::{test_native_dir, why_no_test_hooks};
use support::{Run, Server, assert_error, fin, ok};

const PACK: &str = "@finisher/captions-basic";

fn style_file(name: &str) -> Value {
    let path = support::repo_root().join(format!(
        "resources/plugins/bundled/captions-basic/styles/{name}.json"
    ));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// A project `<root>/film` on `server`.
fn film(server: &Server) -> PathBuf {
    ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    server.root.join("film")
}

#[test]
fn style_from_pack_copies_into_project() {
    let Some(server) = Server::start("style_from_pack_copies_into_project") else {
        return;
    };
    let dir = film(&server);
    let run = |args: &[&str]| fin(&server.sd, &dir, args);
    let from = format!("{PACK}:bold");
    let added = ok(run(&[
        "style",
        "add",
        "--name",
        "Mine",
        "--from",
        &from,
        "--set",
        "maxLines=3",
    ]));
    assert_eq!(added["bindings"]["$style"], "st1", "{added}");
    let step = &added["resolved"];
    // `name` is the pack's name for the style (its contribution), not the style's own `name`.
    assert_eq!(
        step["pack"],
        json!({ "id": PACK, "version": "1.0.0", "name": "bold" })
    );
    // Every path the file has, `--set` on top; the copy takes `--name`, not the pack's name.
    let file = style_file("bold");
    assert_eq!(step["set"]["text.size"], file["text"]["size"], "{step}");
    assert_eq!(step["set"]["color"], file["color"]);
    // The optional parts the default style lacks come across too.
    assert_eq!(step["set"]["stroke.color"], file["stroke"]["color"]);
    assert_eq!(step["set"]["stroke.width"], file["stroke"]["width"]);
    assert_eq!(step["set"]["highlight"], file["highlight"]);
    assert_eq!(step["set"]["maxDuration"], file["maxDuration"]);
    assert!(step["set"].get("background.color").is_none(), "{step}");
    assert_eq!(step["set"]["maxLines"], 3.0);
    assert!(step["set"].get("name").is_none(), "{step}");

    // The same style as the preset the pack ships, but for the name and what `--set` changed.
    ok(run(&[
        "style",
        "add",
        "--name",
        "Preset",
        "--from",
        "preset:bold",
    ]));
    let ls = ok(run(&["style", "ls"]));
    let styles = ls["styles"].as_array().unwrap();
    let (mine, preset) = (&styles[0], &styles[1]);
    assert_eq!(mine["name"], "Mine");
    for key in ["font", "size", "safeZone", "maxChars"] {
        assert_eq!(mine[key], preset[key], "{key}: {ls}");
    }
    assert_eq!(
        (mine["maxLines"].clone(), preset["maxLines"].clone()),
        (json!(3), file["maxLines"].clone())
    );

    // What the pack does not serve, and a `--from` naming no style.
    let missing = run(&[
        "style",
        "add",
        "--name",
        "X",
        "--from",
        &format!("{PACK}:nope"),
    ]);
    assert_error(&missing, 3, "NOT_FOUND");
    let absent = run(&["style", "add", "--name", "X", "--from", "@acme/none:bold"]);
    assert_error(&absent, 3, "PLUGIN_NOT_INSTALLED");
    assert_error(
        &run(&["style", "add", "--name", "X", "--from", PACK]),
        2,
        "USAGE",
    );
    // Nothing was added by the refusals.
    assert_eq!(
        ok(run(&["style", "ls"]))["styles"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

/// A registry folder built by the test-hooks fin-node, and its root's SHA-256.
struct Built {
    dir: PathBuf,
    root_sha256: String,
}

fn build_registry(root: &Path, name: &str, spec: &Value) -> Built {
    let dir = root.join(format!("registry-{name}"));
    let keys = root.join(format!("keys-{name}"));
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
        .arg(spec.to_string())
        .env("FIN_NODE_BINARY_DIR", test_native_dir())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let built: Value = serde_json::from_slice(&out.stdout).unwrap();
    Built {
        dir,
        root_sha256: built["rootSha256"].as_str().unwrap().to_string(),
    }
}

fn url_of(path: &Path) -> String {
    let p = path.display().to_string().replace('\\', "/");
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

/// A test-hooks server with registry `team` built from `spec`, added and refreshed by the
/// console.
fn with_registry(test: &str, spec: Value) -> Option<(Server, Console, Built)> {
    let env: Vec<(String, OsString)> =
        vec![("FIN_NODE_BINARY_DIR".into(), test_native_dir().into())];
    let server = Server::start_with(test, env, why_no_test_hooks())?;
    let built = build_registry(&server.root, "team", &spec);
    let mut console = Console::open(&server);
    console.confirmed_ok(
        "registry.add",
        json!({
            "cfg": { "id": "team", "path": built.dir },
            "root": { "kind": "sha256", "sha256": built.root_sha256 },
        }),
    );
    let refreshed = console.call("registry.refresh", json!({ "id": "team" }));
    assert_eq!(refreshed["_tag"], "Success", "{refreshed}");
    Some((server, console, built))
}

/// Installs `id` at `version` as the owner does.
fn console_install(console: &mut Console, id: &str, version: &str) {
    let plan = console.call("plugin.plan", json!({ "id": id, "version": version }));
    assert_eq!(plan["_tag"], "Success", "{plan}");
    let plan = &plan["value"];
    console.confirmed_ok(
        "plugin.install",
        json!({ "planId": plan["planId"], "digest": plan["digest"] }),
    );
}

fn installed(server: &Server) -> Value {
    ok(fin(&server.sd, &server.root, &["plugin", "ls"]))["plugins"].clone()
}

fn human_only(run: &Run) {
    assert_error(run, 7, "HUMAN_ONLY");
    // The server's refusal as it is: where a person does it, no command for an agent to run.
    let e = &run.json["error"];
    assert_eq!(e["details"]["where"], "console", "{}", run.stdout);
    assert!(!e["fix"].as_str().unwrap().contains('`'), "{}", run.stdout);
}

#[test]
fn fin_plugin_install_is_human_only() {
    let spec = json!({ "packages": ["@acme/pack@1.0.0", "@acme/pack@1.1.0", "@acme/other@0.1.0"] });
    let Some((server, mut console, built)) =
        with_registry("fin_plugin_install_is_human_only", spec)
    else {
        return;
    };
    console_install(&mut console, "@acme/pack", "1.0.0");
    let before = installed(&server);
    let run = |args: &[&str]| fin(&server.sd, &server.root, args);
    let sha = &built.root_sha256;
    let dir = built.dir.display().to_string();
    for args in [
        vec!["plugin", "install", "@acme/other"],
        vec![
            "plugin",
            "install",
            "@acme/other@0.1.0",
            "--registry",
            "team",
        ],
        vec!["plugin", "update", "@acme/pack"],
        vec!["plugin", "rm", "@acme/pack", "--purge-data"],
        vec!["plugin", "disable", "@acme/pack"],
        vec!["plugin", "enable", "@acme/pack"],
        vec!["plugin", "disable", PACK],
        vec![
            "registry",
            "add",
            "again",
            "--dir",
            &dir,
            "--root-sha256",
            sha,
        ],
        vec!["registry", "rm", "team"],
        vec!["registry", "forget", "--root", sha],
    ] {
        let r = run(&args);
        human_only(&r);
        assert_eq!(r.json["error"]["details"]["principal"], "local", "{args:?}");
    }
    // Nothing changed.
    assert_eq!(installed(&server), before);
    assert_eq!(
        ok(run(&["registry", "ls"]))["registries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // A plan's own refusal comes first: there is nothing to approve.
    assert_error(
        &run(&["plugin", "install", "@acme/missing"]),
        3,
        "PLUGIN_NOT_FOUND",
    );
    assert_error(
        &run(&["plugin", "update", "@acme/other"]),
        3,
        "PLUGIN_NOT_INSTALLED",
    );
}

#[test]
fn fin_plugin_search_json_shape() {
    let spec = json!({
        "packages": ["@acme/pack@1.0.0", "@acme/pack@1.1.0", { "id": "@acme/tool", "version": "2.0.0", "kind": "code" }],
    });
    let Some((server, _console, _built)) = with_registry("fin_plugin_search_json_shape", spec)
    else {
        return;
    };
    let run = |args: &[&str]| ok(fin(&server.sd, &server.root, args));
    let all = run(&["plugin", "search"]);
    let ids: Vec<&str> = all["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"@acme/pack") && ids.contains(&"@acme/tool"),
        "{all}"
    );
    let code = run(&["plugin", "search", "--kind", "code", "--registry", "team"]);
    let only: Vec<&Value> = code["plugins"].as_array().unwrap().iter().collect();
    assert_eq!(only.len(), 1, "{code}");
    assert_eq!(only[0]["id"], "@acme/tool");
    assert_eq!(only[0]["registryId"], "team", "{code}");
    let info = run(&["plugin", "info", "@acme/pack"]);
    assert_eq!(info["id"], "@acme/pack");
    assert_eq!(info["registries"].as_array().unwrap().len(), 1, "{info}");
    assert!(info.get("installed").is_none(), "{info}");
    // A registry's folder is shown to people only, and `fin` is not one.
    let registries = run(&["registry", "ls"]);
    let team = &registries["registries"][0];
    assert_eq!(team["id"], "team");
    assert!(team.get("path").is_none(), "{registries}");
    assert!(run(&["registry", "status", "team"]).get("path").is_none());
    let refreshed = run(&["registry", "refresh"]);
    assert_eq!(refreshed["byHuman"], false, "{refreshed}");
    let missing = fin(&server.sd, &server.root, &["registry", "status", "nope"]);
    assert_eq!(missing.code, 3, "{}", missing.stdout);
    assert_eq!(missing.json["error"]["code"], "REGISTRY_NOT_FOUND");
}

#[test]
fn fin_plugin_ls_shows_staleness() {
    let spec = json!({ "packages": ["@acme/old@1.0.0"], "ageDays": 40, "maxStaleDays": 60 });
    let Some((server, mut console, _built)) = with_registry("fin_plugin_ls_shows_staleness", spec)
    else {
        return;
    };
    console_install(&mut console, "@acme/old", "1.0.0");
    let ls = ok(fin(&server.sd, &server.root, &["plugin", "ls"]));
    let by: Vec<&Value> = ls["plugins"].as_array().unwrap().iter().collect();
    let old = by.iter().find(|p| p["id"] == "@acme/old").unwrap();
    let bundled = by.iter().find(|p| p["id"] == PACK).unwrap();
    assert!(old["registryStaleDays"].as_u64().unwrap() >= 9, "{ls}");
    assert!(bundled.get("registryStaleDays").is_none());
    let warnings = ls["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{ls}");
    assert_eq!(warnings[0]["code"], "REGISTRY_STALE");
    assert_eq!(warnings[0]["plugin"], "@acme/old");
    assert_eq!(warnings[0]["registry"], "team");
}

#[test]
fn fin_plugin_suggest_records_a_suggestion() {
    let Some(server) = Server::start("fin_plugin_suggest_records_a_suggestion") else {
        return;
    };
    let dir = film(&server);
    let r = ok(fin(
        &server.sd,
        &dir,
        &[
            "plugin",
            "suggest",
            "@acme/fonts",
            "--reason",
            "the client asked for their font",
        ],
    ));
    assert!(
        r["suggestionId"].as_str().is_some_and(|s| !s.is_empty()),
        "{r}"
    );
    // Outside a project there is nothing to suggest it for.
    let out = fin(
        &server.sd,
        &server.root,
        &["plugin", "suggest", "@acme/fonts", "--reason", "x"],
    );
    assert_ne!(out.code, 0, "{}", out.stdout);
}

#[test]
fn plugin_commands_need_the_server() {
    let tmp = tempfile::tempdir().unwrap();
    for args in [
        vec!["plugin", "ls"],
        vec!["plugin", "search", "x"],
        vec!["registry", "ls"],
        vec!["plugin", "install", "@acme/pack"],
    ] {
        let r = fin(&tmp.path().join("no-server"), tmp.path(), &args);
        assert_error(&r, 5, "SERVER_UNAVAILABLE");
    }
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
