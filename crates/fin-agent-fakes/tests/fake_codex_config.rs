//! `fake-codex` configuration as 0.167.0 reads it, each fact seen side by side with the real
//! binary (7E Task 4 review): typed `-c` values, `$CODEX_HOME/config.toml` as the `user` layer,
//! bundled skills, the origin key of a table-shaped feature, skills discovery up to the project
//! root.

mod codex_common;

use codex_common::*;
use serde_json::{Value, json};

fn strict(kv: &str) -> Vec<String> {
    ["app-server", "--strict-config", "-c", kv]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn run(dir: &Dir, args: &[String]) -> (Option<i32>, String) {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, _, err) = run_to_end(dir, &a);
    (code, err)
}

#[test]
fn fake_config_values_are_typed_even_without_strict() {
    let dir = Dir::new("typed");
    for (kv, want) in [
        (
            "model.foo=1",
            "invalid type: map, expected a string\nin `model`",
        ),
        (
            "shell_environment_policy.inherit.x=1",
            "unknown variant `x`, expected one of `core`, `all`, `none`\nin `shell_environment_policy.inherit`",
        ),
        (
            "project_doc_max_bytes=\"abc\"",
            "invalid type: string \"abc\", expected usize\nin `project_doc_max_bytes`",
        ),
        (
            "project_doc_max_bytes=-1",
            "invalid value: integer `-1`, expected usize",
        ),
        (
            "web_search=\"bogus\"",
            "unknown variant `bogus`, expected one of `disabled`, `cached`, `indexed`, `live`\nin `web_search`",
        ),
        (
            "skills.bundled.enabled=\"yes\"",
            "invalid type: string \"yes\", expected a boolean\nin `skills.bundled.enabled`",
        ),
        (
            "history.persistence=\"bogus\"",
            "unknown variant `bogus`, expected `save-all` or `none`",
        ),
        (
            "analytics=\"x\"",
            "invalid type: string \"x\", expected struct AnalyticsConfigToml\nin `analytics`",
        ),
        (
            "mcp_servers={x=1}",
            "invalid type: integer `1`, expected struct RawMcpServerConfig\nin `mcp_servers.x`",
        ),
        (
            "mcp_servers.x.bogus=1",
            "invalid transport\nin `mcp_servers.x`",
        ),
        (
            "project_doc_fallback_filenames=\"a\"",
            "invalid type: string \"a\", expected a sequence",
        ),
        (
            "shell_environment_policy.set={A=1}",
            "invalid type: integer `1`, expected a string\nin `shell_environment_policy.set.A`",
        ),
        (
            "features.hooks=\"x\"",
            "invalid type: string \"x\", expected a boolean\nin `features`",
        ),
        (
            "approval_policy=\"bogus\"",
            "unknown variant `bogus`, expected one of `untrusted`, `on-failure`, `on-request`, `granular`, `never`",
        ),
        (
            "model=1.5",
            "invalid type: floating point `1.5`, expected a string",
        ),
        (
            "check_for_update_on_startup=\"no\"",
            "invalid type: string \"no\", expected a boolean",
        ),
        (
            "cli_auth_credentials_store=\"bogus\"",
            "unknown variant `bogus`, expected one of `file`, `keyring`, `auto`, `ephemeral`",
        ),
    ] {
        let args: Vec<String> = ["app-server", "-c", kv]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (code, err) = run(&dir, &args);
        assert_eq!(code, Some(1), "{kv}: {err}");
        assert!(err.contains(&format!("Error: {want}")), "{kv}: {err}");
        // --strict-config changes nothing for a type error.
        assert_eq!(run(&dir, &strict(kv)).0, Some(1), "{kv}");
    }
}

#[test]
fn fake_unknown_keys_refused_under_strict_only() {
    let dir = Dir::new("unknown");
    for (kv, field) in [
        ("tools.apply_patch=false", "tools.apply_patch"),
        ("tools.view_image=false", "tools.view_image"),
        ("nosuch=1", "nosuch"),
        ("x.y=5", "x"),
        ("features.nosuch=true", "features.nosuch"),
        ("skills.bundled.foo=1", "skills.bundled.foo"),
        (
            "shell_environment_policy.bogus=1",
            "shell_environment_policy.bogus",
        ),
        ("analytics.enabld=false", "analytics.enabld"),
    ] {
        let args: Vec<String> = ["app-server", "-c", kv]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(run(&dir, &args).0, Some(0), "{kv} without --strict-config");
        let (code, err) = run(&dir, &strict(kv));
        assert_eq!(code, Some(1), "{kv}");
        assert!(
            err.contains(&format!(
                "unknown configuration field `{field}` in -c/--config override"
            )),
            "{kv}: {err}"
        );
    }
    for kv in [
        "tools.web_search=false",
        "mcp_servers.x.command=\"ls\"",
        "mcp_servers.x.url=\"http://127.0.0.1:9\"",
        "features.sleep_tool.enabled=false",
        "features.sleep_tool=false",
        "agents.enabled=false",
        "skills.include_instructions=false",
        "skills.config=[]",
        "shell_environment_policy={inherit=\"none\"}",
        "approval_policy=\"never\"",
        "web_search=disabled",
    ] {
        let (code, err) = run(&dir, &strict(kv));
        assert_eq!(code, Some(0), "{kv}: {err}");
    }
}

fn plant_user_config(dir: &Dir) {
    dir.write(
        "codex-home/config.toml",
        "# the user's own file\napproval_policy = \"never\"\n[mcp_servers.x]\ncommand = \"ls\"\n[features]\nhooks = true\n",
    );
    dir.write(
        "codex-home/hooks.json",
        &json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "touch /dev/null"}]}]}})
            .to_string(),
    );
}

#[test]
fn fake_reads_user_config_toml() {
    let dir = Dir::new("usertoml");
    plant_user_config(&dir);
    let file = dir.home().join("config.toml");
    let mut c = Client::start_with(
        &dir,
        None,
        &[
            "app-server",
            "--listen",
            "stdio://",
            "--disable",
            "sleep_tool",
        ],
        &[],
    );
    c.initialize();
    let r = c.call("config/read", json!({"includeLayers": true}));
    let layers = r["layers"].as_array().unwrap();
    assert_eq!(
        layers[0]["config"],
        json!({"features": {"sleep_tool": false}})
    );
    assert_eq!(
        (
            layers[1]["name"]["type"].as_str(),
            layers[1]["name"]["file"].as_str()
        ),
        (Some("user"), file.to_str())
    );
    assert_eq!(
        layers[1]["config"],
        json!({"approval_policy": "never", "mcp_servers": {"x": {"command": "ls"}}, "features": {"hooks": true}})
    );
    let origin = |k: &str| r["origins"][k]["name"]["type"].as_str().map(str::to_string);
    assert_eq!(origin("approval_policy").as_deref(), Some("user"));
    assert_eq!(origin("mcp_servers.x.command").as_deref(), Some("user"));
    assert_eq!(origin("features.hooks").as_deref(), Some("user"));
    assert_eq!(
        origin("features.sleep_tool.enabled").as_deref(),
        Some("sessionFlags")
    );
    assert_eq!(
        r["origins"]["approval_policy"]["name"]["file"],
        file.to_str().unwrap()
    );
    let cfg = &r["config"];
    assert_eq!(cfg["approval_policy"], "never");
    assert_eq!(
        cfg["mcp_servers"]["x"],
        json!({"command": "ls", "args": [], "environment_id": "local", "enabled": true, "tool_timeout_sec": null})
    );
    assert_eq!(
        (
            cfg["features"]["hooks"].as_bool(),
            cfg["features"]["sleep_tool"].as_bool()
        ),
        (Some(true), Some(false))
    );
    // The user layer's server is listed; the fake runs none, so it failed to start.
    let s = c.call("mcpServerStatus/list", json!({}));
    assert_eq!(s["data"][0]["name"], "x");
    assert!(s["data"][0]["toolsError"].is_string(), "{s}");
    // hooks on from the user layer: the planted hook is listed.
    let h = c.call("hooks/list", json!({"cwds": [dir.work()]}));
    let hook = &h["data"][0]["hooks"][0];
    assert_eq!(hook["eventName"], "sessionStart");
    assert_eq!(
        hook["key"],
        format!(
            "{}:session_start:0:0",
            dir.home().join("hooks.json").display()
        )
    );
    // thread/start takes the user's approval_policy when the params name none.
    let t = c.call("thread/start", json!({"cwd": dir.work()}));
    assert_eq!(t["approvalPolicy"], "never");
    let mut statuses = vec![];
    while statuses.len() < 2 {
        let v = c.next();
        if v["method"] == "mcpServer/startupStatus/updated" {
            assert_eq!(v["params"]["name"], "x");
            statuses.push(v["params"]["status"].as_str().unwrap().to_string());
        }
    }
    assert_eq!(statuses, ["starting", "failed"]);
    let t = c.call(
        "thread/start",
        json!({"cwd": dir.work(), "approvalPolicy": "on-request"}),
    );
    assert_eq!(t["approvalPolicy"], "on-request");
    drop(c);
    // Layers merge: `-c mcp_servers={}` does not remove the user's server (as 0.167.0).
    let mut c = Client::start_with(&dir, None, &["app-server", "-c", "mcp_servers={}"], &[]);
    c.initialize();
    let s = c.call("mcpServerStatus/list", json!({}));
    assert_eq!(s["data"][0]["name"], "x");
}

#[test]
fn fake_config_toml_strict_and_broken() {
    let dir = Dir::new("brokentoml");
    dir.write("codex-home/config.toml", "bogus_key = 1\n");
    let (code, err) = run(&dir, &["app-server".into(), "--strict-config".into()]);
    assert_eq!(code, Some(1));
    let file = dir.home().join("config.toml");
    assert!(
        err.contains(&format!(
            "Error: {}:1:1: unknown configuration field `bogus_key`",
            file.display()
        )),
        "{err}"
    );
    assert_eq!(run(&dir, &["app-server".into()]).0, Some(0));
    dir.write("codex-home/config.toml", "[analytics]\n  enabld = true\n");
    let (code, err) = run(&dir, &["app-server".into(), "--strict-config".into()]);
    assert_eq!(code, Some(1));
    assert!(
        err.contains(":2:3: unknown configuration field `analytics.enabld`"),
        "{err}"
    );
    // A wrongly typed value or a file that is not TOML does not stop the start.
    for text in ["project_doc_max_bytes = \"abc\"\n", "this is not toml\n"] {
        dir.write("codex-home/config.toml", text);
        let (code, err) = run(&dir, &["app-server".into(), "--strict-config".into()]);
        assert_eq!(code, Some(0), "{text}: {err}");
    }
}

#[test]
fn fake_bundled_skills_unless_disabled() {
    let dir = Dir::new("bundled");
    let mut c = Client::start_with(&dir, None, &["app-server"], &[]);
    c.initialize();
    let r = c.call("skills/list", json!({"cwds": [dir.work()]}));
    let system: Vec<&Value> = r["data"][0]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["scope"] == "system")
        .collect();
    let names: Vec<&str> = system.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "imagegen",
            "openai-docs",
            "review-agent",
            "skill-creator",
            "skill-installer"
        ]
    );
    let p = dir.home().join("skills/.system/imagegen/SKILL.md");
    assert_eq!(system[0]["path"], p.to_str().unwrap());
    assert!(
        p.is_file(),
        "the bundled skills are written under CODEX_HOME, as 0.167.0 does"
    );
    drop(c);
    // Decision 4 switches them off: nothing listed, nothing written.
    let dir = Dir::new("bundledoff");
    let mut c = Client::start(&dir, None);
    c.initialize();
    let r = c.call("skills/list", json!({"cwds": [dir.work()]}));
    assert_eq!(r["data"][0]["skills"], json!([]));
    assert!(!dir.home().join("skills").exists());
}

#[test]
fn fake_skills_discovery_stops_at_the_project_root() {
    let dir = Dir::new("projroot");
    let skill = |n: &str| format!("---\nname: {n}\ndescription: planted {n}\n---\n");
    dir.write(".agents/skills/above/SKILL.md", &skill("above"));
    dir.write("work/.agents/skills/x/SKILL.md", &skill("x"));
    std::fs::create_dir_all(dir.work().join("sub")).unwrap();
    let names = |c: &mut Client| -> Vec<String> {
        let r = c.call("skills/list", json!({"cwds": [dir.work().join("sub")]}));
        r["data"][0]["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    };
    let mut c = Client::start(&dir, None);
    c.initialize();
    assert_eq!(
        names(&mut c),
        ["x", "above"],
        "no project root: every parent"
    );
    std::fs::create_dir_all(dir.work().join(".git")).unwrap();
    assert_eq!(names(&mut c), ["x"], "the `.git` directory ends the walk");
}

#[test]
fn fake_disable_of_a_table_feature_names_its_enabled_key() {
    let dir = Dir::new("sleeptool");
    let mut c = Client::start_with(
        &dir,
        None,
        &[
            "app-server",
            "--disable",
            "sleep_tool",
            "--disable",
            "hooks",
        ],
        &[],
    );
    c.initialize();
    let r = c.call("config/read", json!({"includeLayers": true}));
    let keys: Vec<&String> = r["origins"].as_object().unwrap().keys().collect();
    assert!(
        keys.iter().any(|k| *k == "features.sleep_tool.enabled"),
        "{keys:?}"
    );
    assert!(
        !keys.iter().any(|k| *k == "features.sleep_tool"),
        "{keys:?}"
    );
    assert!(keys.iter().any(|k| *k == "features.hooks"), "{keys:?}");
    assert_eq!(r["config"]["features"]["sleep_tool"], false);
}
