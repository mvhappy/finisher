//! `fake-codex` against its own binary: argv, handshake, the read methods and what it records.

mod codex_common;

use codex_common::*;
use serde_json::{Value, json};

#[test]
fn fake_handshake_required() {
    let dir = Dir::new("handshake");
    let mut c = Client::start(&dir, None);
    let e = c.call_err("config/read", json!({"includeLayers": false}));
    assert_eq!(
        (e["code"].as_i64(), e["message"].as_str()),
        (Some(-32600), Some("Not initialized"))
    );
    let init = c.initialize();
    assert_eq!(
        init["userAgent"],
        "finisher/0.167.0 (Linux; x86_64) unknown (finisher; 0.0.0)"
            .replace("Linux; x86_64", &platform())
    );
    assert_eq!(init["codexHome"], dir.home().to_str().unwrap());
    assert!(init["platformFamily"].is_string() && init["platformOs"].is_string());
    let n = c.until("remoteControl/status/changed");
    assert_eq!(n["params"]["status"], "disabled");
    let e = c.call_err(
        "initialize",
        json!({"clientInfo": {"name": "x", "version": "1"}}),
    );
    assert_eq!(
        (e["code"].as_i64(), e["message"].as_str()),
        (Some(-32600), Some("Already initialized"))
    );
    // A method outside the positive set is the real binary's "unknown variant" error.
    let e = c.call_err("thread/rollback", json!({}));
    assert_eq!(e["code"], -32600);
    let m = e["message"].as_str().unwrap();
    assert!(
        m.starts_with("Invalid request: unknown variant `thread/rollback`, expected one of"),
        "{m}"
    );
    // `config/read` still answers, so the error did not wedge the handshake.
    c.call("config/read", json!({"includeLayers": false}));
    c.close_stdin();
    assert_eq!(c.finish().code(), Some(0), "stdin EOF is a clean stop");
    let o = observed(&dir);
    let names: Vec<&str> = o["received"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["method"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "config/read",
            "initialize",
            "initialized",
            "initialize",
            "thread/rollback",
            "config/read"
        ]
    );
}

fn platform() -> String {
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "Mac OS",
        "windows" => "Windows",
        o => o,
    };
    format!("{os}; {}", std::env::consts::ARCH)
}

#[test]
fn fake_refuses_unknown_config_key() {
    let dir = Dir::new("strict");
    let (code, out, err) = run_to_end(
        &dir,
        &[
            "app-server",
            "--listen",
            "stdio://",
            "--strict-config",
            "-c",
            "project_doc_max_byts=0",
        ],
    );
    assert_eq!(code, Some(1));
    assert!(out.is_empty(), "nothing on stdout: {out}");
    assert!(
        err.contains("unknown configuration field `project_doc_max_byts` in -c/--config override"),
        "{err}"
    );
    // A misspelt key one level down is caught too (`analytics` has a known shape).
    let (code, _, err) = run_to_end(
        &dir,
        &[
            "app-server",
            "--strict-config",
            "-c",
            "analytics.enabld=false",
        ],
    );
    assert_eq!(code, Some(1));
    assert!(err.contains("analytics.enabld"), "{err}");
    // The same keys without --strict-config are ignored.
    let (code, _, _) = run_to_end(&dir, &["app-server", "-c", "project_doc_max_byts=0"]);
    assert_eq!(code, Some(0));
    // A feature the pin does not have.
    let (code, _, err) = run_to_end(&dir, &["app-server", "--disable", "no_such_feature"]);
    assert_eq!(code, Some(1));
    assert!(err.contains("Unknown feature flag"), "{err}");
    // An unknown flag and a missing subcommand are usage errors (clap exits 2).
    let (code, _, err) = run_to_end(&dir, &["app-server", "--bogus"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("unexpected argument '--bogus'"), "{err}");
    assert_eq!(run_to_end(&dir, &[]).0, Some(2));
    // Decision 4's whole command line is accepted.
    assert_eq!(run_to_end(&dir, D4_ARGS).0, Some(0));
    let (code, out, _) = run_to_end(&dir, &["--version"]);
    assert_eq!((code, out.trim()), (Some(0), "codex-cli 0.167.0"));
}

#[test]
fn fake_config_read_reflects_overrides() {
    let dir = Dir::new("config");
    let mut c = Client::start(&dir, None);
    c.initialize();
    let r = c.call("config/read", json!({"includeLayers": true}));
    let cfg = &r["config"];
    assert_eq!(cfg["project_doc_max_bytes"], 0);
    assert_eq!(cfg["project_doc_fallback_filenames"], json!([]));
    assert_eq!(cfg["cli_auth_credentials_store"], "file");
    assert_eq!(cfg["web_search"], "disabled");
    assert_eq!(cfg["analytics"]["enabled"], false);
    assert_eq!(
        cfg["shell_environment_policy"]["set"],
        json!({"PATH": "/usr/bin:/bin"})
    );
    assert_eq!(cfg["shell_environment_policy"]["inherit"], "none");
    assert_eq!(cfg["skills"]["bundled"]["enabled"], false);
    // --disable F is features.F = false; a feature nobody named keeps its default.
    for f in [
        "hooks",
        "multi_agent",
        "shell_tool",
        "unified_exec",
        "plugins",
    ] {
        assert_eq!(cfg["features"][f], false, "{f}");
    }
    assert_eq!(
        cfg["features"]["view_image"], true,
        "default-on feature stays on"
    );
    // Origins and layers: the flags are a `sessionFlags` layer above `user` and `system`.
    assert_eq!(
        r["origins"]["project_doc_max_bytes"]["name"]["type"],
        "sessionFlags"
    );
    assert_eq!(
        r["origins"]["features.hooks"]["name"]["type"],
        "sessionFlags"
    );
    let layers = r["layers"].as_array().unwrap();
    let kinds: Vec<&str> = layers
        .iter()
        .map(|l| l["name"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["sessionFlags", "user", "system"]);
    assert_eq!(layers[0]["config"]["project_doc_max_bytes"], 0);
    assert_eq!(layers[0]["config"]["features"]["hooks"], false);
    assert_eq!(
        layers[1]["name"]["file"],
        dir.home().join("config.toml").to_str().unwrap()
    );
    // Without includeLayers there are no layers.
    let r = c.call("config/read", json!({"includeLayers": false}));
    assert!(r["layers"].is_null(), "{r}");
    // Other launches see their own values; no flags, no sessionFlags layer.
    let dir2 = Dir::new("config2");
    let mut c2 = Client::start_with(
        &dir2,
        None,
        &[
            "app-server",
            "-c",
            "web_search=\"live\"",
            "--enable",
            "goals",
        ],
        &[],
    );
    c2.initialize();
    let r = c2.call("config/read", json!({"includeLayers": true}));
    assert_eq!(r["config"]["web_search"], "live");
    assert_eq!(r["config"]["features"]["goals"], true);
    assert_eq!(r["config"]["project_doc_max_bytes"], 32768);
    let dir3 = Dir::new("config3");
    let mut c3 = Client::start_with(&dir3, None, &["app-server"], &[]);
    c3.initialize();
    let r = c3.call("config/read", json!({"includeLayers": true}));
    let kinds: Vec<&str> = r["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["user", "system"]);
}

fn skill(name: &str) -> String {
    format!("---\nname: {name}\ndescription: planted skill {name}\n---\nbody\n")
}

#[test]
fn fake_lists_planted_skills_and_hooks() {
    let dir = Dir::new("lists");
    // Fresh: everything empty and no instruction source.
    let mut c = Client::start(&dir, None);
    c.initialize();
    let work = dir.work();
    assert_eq!(
        c.call("skills/list", json!({"cwds": [work]}))["data"],
        json!([{"cwd": work, "skills": [], "errors": []}])
    );
    assert_eq!(
        c.call("hooks/list", json!({"cwds": [work]}))["data"][0]["hooks"],
        json!([])
    );
    assert_eq!(
        c.call("plugin/installed", json!({}))["marketplaces"],
        json!([])
    );
    assert_eq!(c.call("mcpServerStatus/list", json!({}))["data"], json!([]));
    assert_eq!(c.call("app/installed", json!({}))["apps"], json!([]));
    assert_eq!(
        c.call("configRequirements/read", json!({}))["requirements"],
        Value::Null
    );
    assert_eq!(
        c.start_thread(&dir, json!({}))["instructionSources"],
        json!([])
    );
    drop(c);
    // Planted, as the isolation tests do: workspace, HOME and CODEX_HOME.
    dir.write("work/.agents/skills/x/SKILL.md", &skill("x"));
    dir.write("home/.agents/skills/y/SKILL.md", &skill("y"));
    dir.write("codex-home/skills/z/SKILL.md", &skill("z"));
    dir.write("codex-home/AGENTS.md", "global instructions\n");
    dir.write("work/AGENTS.md", "workspace instructions\n");
    dir.write(
        "codex-home/hooks.json",
        &json!({"hooks": {"PreToolUse": [{"matcher": "Bash",
        "hooks": [{"type": "command", "command": "echo hi", "timeout": 5}]}]}})
        .to_string(),
    );
    let mut c = Client::start(&dir, None);
    c.initialize();
    let skills = c.call("skills/list", json!({"cwds": [work], "forceReload": true}));
    let mut got: Vec<(String, String, String)> = skills["data"][0]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap().into(),
                s["scope"].as_str().unwrap().into(),
                s["path"].as_str().unwrap().into(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            (
                "x".to_string(),
                "repo".to_string(),
                work.join(".agents/skills/x/SKILL.md")
                    .to_str()
                    .unwrap()
                    .to_string()
            ),
            (
                "y".to_string(),
                "user".to_string(),
                dir.path("home/.agents/skills/y/SKILL.md")
                    .to_str()
                    .unwrap()
                    .to_string()
            ),
            (
                "z".to_string(),
                "user".to_string(),
                dir.home()
                    .join("skills/z/SKILL.md")
                    .to_str()
                    .unwrap()
                    .to_string()
            ),
        ]
    );
    assert_eq!(
        skills["data"][0]["skills"][0]["description"]
            .as_str()
            .map(|d| d.starts_with("planted skill")),
        Some(true)
    );
    // No cwds: the process's own working directory.
    assert_eq!(
        c.call("skills/list", json!({}))["data"][0]["skills"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    // Decision 4 disables the `hooks` feature: the planted file is not listed, and
    // instructionSources is the global AGENTS.md only (project_doc_max_bytes is 0).
    assert_eq!(
        c.call("hooks/list", json!({"cwds": [work]}))["data"][0]["hooks"],
        json!([])
    );
    let t = c.start_thread(&dir, json!({}));
    assert_eq!(
        t["instructionSources"],
        json!([dir.home().join("AGENTS.md")])
    );
    drop(c);
    // Without those flags: the hook is listed and the workspace file loads too.
    let mut c = Client::start_with(&dir, None, &["app-server"], &[]);
    c.initialize();
    let hooks = c.call("hooks/list", json!({"cwds": [work]}));
    let h = &hooks["data"][0]["hooks"][0];
    assert_eq!(
        (
            h["eventName"].as_str(),
            h["handlerType"].as_str(),
            h["command"].as_str()
        ),
        (Some("preToolUse"), Some("command"), Some("echo hi"))
    );
    assert_eq!(
        (
            h["matcher"].as_str(),
            h["timeoutSec"].as_u64(),
            h["source"].as_str()
        ),
        (Some("Bash"), Some(5), Some("user"))
    );
    assert_eq!(
        h["sourcePath"],
        dir.home().join("hooks.json").to_str().unwrap()
    );
    let t = c.start_thread(&dir, json!({}));
    assert_eq!(
        t["instructionSources"],
        json!([dir.home().join("AGENTS.md"), work.join("AGENTS.md")])
    );
    // An override file replaces the global AGENTS.md.
    dir.write("codex-home/AGENTS.override.md", "override");
    let t = c.start_thread(&dir, json!({}));
    assert_eq!(
        t["instructionSources"][0],
        dir.home().join("AGENTS.override.md").to_str().unwrap()
    );
}

/// What the probe's isolation tests plant: one installed plugin, one app, one MCP server.
fn planted_lists() -> Value {
    json!({
        "plugins": [{"name": "m", "plugins": [{"id": "p@m", "name": "p", "enabled": true,
            "installed": true, "authPolicy": "ON_USE", "installPolicy": "AVAILABLE",
            "source": {"type": "remote"}}]}],
        "apps": [{"id": "a", "enabled": true, "callable": true}],
        "mcpServers": [{"name": "s", "authStatus": "unsupported", "tools": {},
            "resources": [], "resourceTemplates": []}],
    })
}

#[test]
fn fake_plants_plugins_apps_mcp_and_refuses_dynamic_tools() {
    let dir = Dir::new("planted");
    let mut c = Client::start(&dir, Some(planted_lists()));
    c.initialize();
    assert_eq!(
        c.call("plugin/installed", json!({}))["marketplaces"][0]["plugins"][0]["id"],
        "p@m"
    );
    assert_eq!(c.call("app/installed", json!({}))["apps"][0]["id"], "a");
    assert_eq!(
        c.call("mcpServerStatus/list", json!({}))["data"][0]["name"],
        "s"
    );
    drop(c);
    let mut c = Client::start(&dir, Some(json!({"refuseDynamicTools": true})));
    c.initialize();
    let p = json!({"cwd": dir.work(), "dynamicTools": [Client::shell_tool()]});
    let e = c.call_err("thread/start", p);
    assert_eq!(
        e["message"],
        "Invalid request: unknown field `dynamicTools`"
    );
    // Without the field the thread starts.
    assert!(c.start_thread(&dir, json!({}))["thread"]["id"].is_string());
    drop(c);
    // Signed in by other means: an account with no auth.json.
    let mut c = Client::start(&dir, Some(json!({"signedInElsewhere": true})));
    c.initialize();
    assert_eq!(
        c.call("account/read", json!({}))["account"]["type"],
        "chatgpt"
    );
    assert!(!dir.home().join("auth.json").exists());
}

#[test]
fn fake_account_and_models_read() {
    let dir = Dir::new("account");
    let mut c = Client::start(&dir, None);
    c.initialize();
    // Signed out: no auth.json, whatever the environment holds.
    let r = c.call("account/read", json!({"refreshToken": false}));
    assert_eq!(
        r,
        json!({"account": null, "requiresOpenaiAuth": true, "workspaceRouting": null})
    );
    dir.write("codex-home/auth.json", "{}");
    let r = c.call("account/read", json!({"refreshToken": false}));
    assert_eq!(
        r["account"],
        json!({"type": "chatgpt", "email": "person@example.com", "planType": "plus"})
    );
    dir.write(
        "codex-home/fake-account.json",
        &json!({"type": "chatgpt", "email": "a@b.test", "planType": "pro"}).to_string(),
    );
    assert_eq!(
        c.call("account/read", json!({}))["account"]["email"],
        "a@b.test"
    );
    dir.write(
        "codex-home/fake-account.json",
        &json!({"type": "apiKey"}).to_string(),
    );
    assert_eq!(
        c.call("account/read", json!({}))["account"],
        json!({"type": "apiKey"})
    );
    // model/list answers signed out and pages by `limit` and `cursor`.
    let first = c.call("model/list", json!({"limit": 2}));
    assert_eq!(first["data"].as_array().unwrap().len(), 2);
    assert_eq!(first["nextCursor"], "2");
    assert_eq!(first["data"][0]["isDefault"], true);
    let mut seen = 2;
    let mut cursor = first["nextCursor"].clone();
    while !cursor.is_null() {
        let p = c.call("model/list", json!({"limit": 4, "cursor": cursor}));
        seen += p["data"].as_array().unwrap().len();
        cursor = p["nextCursor"].clone();
    }
    let all = c.call("model/list", json!({}));
    assert_eq!(all["data"].as_array().unwrap().len(), seen);
    assert!(all["nextCursor"].is_null());
    let f = c.call("experimentalFeature/list", json!({"limit": 5}));
    assert_eq!(f["data"].as_array().unwrap().len(), 5);
    assert!(f["nextCursor"].is_string());
}

#[test]
fn fake_records_env_names_only() {
    let dir = Dir::new("env");
    let secret = "sk-this-value-must-never-be-written";
    let mut c = Client::start_with(
        &dir,
        None,
        D4_ARGS,
        &[
            ("OPENAI_API_KEY", secret),
            ("CODEX_ACCESS_TOKEN", "tok-secret-9"),
            ("NO_COLOR", "1"),
        ],
    );
    c.initialize();
    c.call("config/read", json!({}));
    c.call("model/list", json!({}));
    c.close_stdin();
    assert_eq!(c.finish().code(), Some(0));
    let raw = std::fs::read_to_string(dir.path("observed.json")).unwrap();
    assert!(
        !raw.contains(secret) && !raw.contains("tok-secret-9"),
        "a value reached the observed file"
    );
    let o = observed(&dir);
    let names: Vec<&str> = o["envNames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    for n in [
        "OPENAI_API_KEY",
        "CODEX_ACCESS_TOKEN",
        "NO_COLOR",
        "CODEX_HOME",
        "HOME",
        "PATH",
    ] {
        assert!(names.contains(&n), "{n} missing from {names:?}");
    }
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    // Flags: `-c` values as raw strings, disabled features, listen, strict-config.
    assert_eq!(o["flags"]["strictConfig"], true);
    assert_eq!(o["flags"]["listen"], "stdio://");
    assert_eq!(o["flags"]["config"]["project_doc_max_bytes"], "0");
    assert_eq!(
        o["flags"]["config"]["cli_auth_credentials_store"],
        "\"file\""
    );
    assert_eq!(
        o["flags"]["disable"],
        json!([
            "hooks",
            "multi_agent",
            "shell_tool",
            "unified_exec",
            "plugins"
        ])
    );
    assert_eq!(o["flags"]["enable"], json!([]));
    assert_eq!(o["argv"][0], "app-server");
    // The initialize params and every method, in order (a notification has no id).
    assert_eq!(o["clientInfo"]["name"], "finisher");
    let methods: Vec<&str> = o["received"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["method"].as_str().unwrap())
        .collect();
    assert_eq!(
        methods,
        ["initialize", "initialized", "config/read", "model/list"]
    );
    assert!(o["received"][1].get("id").is_none() && o["received"][0]["id"].is_number());
    assert_eq!(o["exit"], json!({"code": 0, "reason": "eof"}));
}

#[test]
fn fake_messages_validate_against_schema() {
    // The validator itself: a correct line passes, broken ones do not.
    let s = schema::Schemas::load();
    let good = json!({"method": "thread/compacted", "params": {"threadId": "t", "turnId": "u"}});
    assert_eq!(s.check_line(&good, None), Ok(()));
    let missing = json!({"method": "thread/compacted", "params": {"threadId": "t"}});
    assert!(s.check_line(&missing, None).is_err());
    let wrong_enum = json!({"method": "turn/completed", "params": {"threadId": "t", "turn": {"id": "x", "items": [],
        "status": "weird", "error": null}}});
    assert!(s.check_line(&wrong_enum, None).is_err());
    let wrong_type = json!({"id": 1, "result": {"account": 5, "requiresOpenaiAuth": true}});
    assert!(s.check_line(&wrong_type, Some("account/read")).is_err());
    let unknown = json!({"method": "fake/unknown", "params": {}});
    assert!(s.check_line(&unknown, None).is_err());

    // A busy session: every line below is validated when the client drops.
    let dir = Dir::new("schema");
    dir.write("work/.agents/skills/x/SKILL.md", &skill("x"));
    dir.write("codex-home/AGENTS.md", "x");
    dir.write(
        "codex-home/hooks.json",
        &json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "true"}]}]}})
            .to_string(),
    );
    dir.write("codex-home/auth.json", "{}");
    dir.write("work/b.txt", "b\n");
    let script = json!({"turns": [
        {"steps": [{"say": "hello", "usage": {"last": {"inputTokens": 10, "outputTokens": 2}, "total": {"inputTokens": 10, "outputTokens": 2}}},
            {"dynamicTool": {"tool": "shell", "arguments": {"command": "ls"}}},
            {"patch": {"changes": [{"path": "a.txt", "kind": {"type": "add"}, "diff": "x\n"},
                {"path": "b.txt", "kind": {"type": "update", "move_path": "c.txt"}, "diff": ""}]}}]},
        {"steps": [{"fail": {"codexErrorInfo": "usageLimitExceeded", "message": "limit"}}]},
        {"steps": [{"notLoggedIn": true}]},
        {"steps": [{"accountChange": {"type": "chatgpt", "email": "n@x.test", "planType": "pro"}}]}]});
    let mut c = Client::start(&dir, Some(script));
    c.initialize();
    let mut methods = vec![];
    c.call("config/read", json!({"includeLayers": true}));
    c.call("skills/list", json!({}));
    c.call("hooks/list", json!({}));
    c.call("account/read", json!({}));
    c.call("model/list", json!({"limit": 3}));
    c.call("experimentalFeature/list", json!({}));
    let t = c.start_thread(&dir, json!({"dynamicTools": [Client::shell_tool()]}));
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    c.start_turn(&tid, "go");
    loop {
        let v = c.next();
        methods.push(v["method"].as_str().unwrap_or("").to_string());
        if v.get("id").is_some() && v["method"] == "item/tool/call" {
            c.answer(
                &v["id"],
                json!({"contentItems": [{"type": "inputText", "text": "exit 0"}], "success": true}),
            );
        } else if v.get("id").is_some() && v["method"] == "item/fileChange/requestApproval" {
            c.answer(&v["id"], json!({"decision": "accept"}));
        } else if v["method"] == "turn/completed" {
            break;
        }
    }
    for turn in 0..3 {
        c.start_turn(&tid, "again");
        let lines = c.turn_lines();
        methods.extend(
            lines
                .iter()
                .map(|l| l["method"].as_str().unwrap_or("").to_string()),
        );
        let last = &lines.last().unwrap()["params"]["turn"];
        assert_eq!(
            last["status"],
            if turn == 2 { "completed" } else { "failed" }
        );
    }
    c.call("account/read", json!({}));
    c.notify("initialized", json!({}));
    for m in [
        "thread/tokenUsage/updated",
        "item/tool/call",
        "item/fileChange/requestApproval",
        "item/agentMessage/delta",
        "thread/started",
        "turn/started",
        "turn/completed",
        "error",
        "account/updated",
    ] {
        assert!(
            methods.iter().any(|x| x == m) || m == "thread/started",
            "no {m} in {methods:?}"
        );
    }
    let n = c.validate();
    assert!(
        n > 40,
        "the scenario should have produced many lines, got {n}"
    );
}
