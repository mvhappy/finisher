//! The plugin protocol's types (8B plan Task 2, Decision 2): every type round-trips through
//! JSON unchanged; host → plugin payloads ignore unknown fields and the rest refuse them; and
//! the manifest's `commands` (host API 1.1) and `hooks` (1.2, 7F Task 9) contributions parse and
//! validate.

mod support;

use std::fmt::Debug;

use fin_plugin_api::protocol::{
    CallLimits, CommandOut, CommandRun, HookEvent, HostMethod, JobRights, PluginContext,
    PluginDirs, PluginError, PluginRefFull, PluginScope, Reach, calls,
};
use fin_plugin_api::{
    CommandContribution, ContributionKind, HOST_API, HookContribution, SourceClass, consumable,
    host_api,
};
use semver::Version;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use support::{MemFiles, check, check_with, ctx, fixture, reasons, set};

/// Parses `v` as `T`, serialises it back and checks the JSON is unchanged.
fn round<T: Serialize + DeserializeOwned + PartialEq + Debug>(v: Value) -> T {
    let t: T = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
    assert_eq!(serde_json::to_value(&t).unwrap(), v);
    let again: T = serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
    assert_eq!(again, t);
    t
}

/// `v` with an extra field decodes, the field ignored: a host → plugin payload from a newer host
/// (forward compatibility; the cross-SDK contract).
fn accepts_extra<T: Serialize + DeserializeOwned + Debug>(mut v: Value) {
    let plain = serde_json::to_value(serde_json::from_value::<T>(v.clone()).unwrap()).unwrap();
    v.as_object_mut().unwrap().insert("future".into(), json!(1));
    let t: T = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
    assert_eq!(serde_json::to_value(&t).unwrap(), plain, "{v}");
}

/// `v` with an extra field is refused.
fn refuses_extra<T: DeserializeOwned + Debug>(mut v: Value) {
    v.as_object_mut()
        .unwrap()
        .insert("surprise".into(), json!(1));
    assert!(serde_json::from_value::<T>(v.clone()).is_err(), "{v}");
}

fn context_json() -> Value {
    json!({
        "hostApi": "1.1.0",
        "plugin": { "id": "@acme/tools", "version": "1.2.0" },
        "grants": [{ "perm": "project:read" }, { "perm": "net", "origins": ["https://api.example.org"] }],
        "dirs": { "plugin": "/store/acme/tools/1.2.0", "data": "/data/acme/tools/p1", "tmp": "/tmp/p" },
        "locale": "en",
        "limits": { "timeoutS": 10, "messageBytes": 1048576, "pageBytes": 774144 }
    })
}

#[test]
fn plugin_context_round_trips() {
    let c: PluginContext = round(context_json());
    assert_eq!(c.host_api, Version::new(1, 1, 0));
    assert_eq!(c.plugin.id.to_string(), "@acme/tools");
    assert_eq!(c.grants.len(), 2);
    assert_eq!(c.dirs.data.as_deref(), Some("/data/acme/tools/p1"));
    accepts_extra::<PluginContext>(context_json());
    accepts_extra::<PluginDirs>(context_json()["dirs"].clone());
    accepts_extra::<CallLimits>(context_json()["limits"].clone());
    accepts_extra::<PluginRefFull>(context_json()["plugin"].clone());
    // `data` is absent without `fs pluginData`.
    let mut v = context_json();
    v["dirs"].as_object_mut().unwrap().remove("data");
    let c: PluginContext = round(v);
    assert_eq!(c.dirs.data, None);
    // A bad plugin id is refused.
    let v = set(context_json(), "/plugin/id", json!("acme/tools"));
    assert!(serde_json::from_value::<PluginContext>(v).is_err());
}

#[test]
fn command_run_round_trips() {
    let v = json!({ "command": "trim-silence", "args": { "threshold": -40 }, "project": "p1", "branch": "main", "rev": 7 });
    let r: CommandRun = round(v.clone());
    assert_eq!(r.rev, 7);
    accepts_extra::<CommandRun>(v);
}

#[test]
fn command_out_round_trips() {
    let tx =
        json!({ "kind": "tx", "ops": [{ "op": "item.delete", "item": "c1", "mode": "ripple" }] });
    match round::<CommandOut>(tx.clone()) {
        CommandOut::Tx { ops } => assert_eq!(ops.len(), 1),
        other => panic!("{other:?}"),
    }
    refuses_extra::<CommandOut>(tx);
    let data = json!({ "kind": "data", "value": { "title": "Hello" } });
    assert!(matches!(round::<CommandOut>(data), CommandOut::Data { .. }));
    // An op fin-core does not know is refused at decode.
    let bad = json!({ "kind": "tx", "ops": [{ "op": "item.teleport", "item": "c1" }] });
    assert!(serde_json::from_value::<CommandOut>(bad).is_err());
}

#[test]
fn calls_round_trip() {
    round::<calls::ProjectSnapshot>(json!({}));
    round::<calls::ProjectSnapshot>(json!({ "page": 2 }));
    refuses_extra::<calls::ProjectSnapshot>(json!({}));
    let page = json!({ "bytesB64": "e30=", "page": 0, "pages": 1, "rev": 9 });
    round::<calls::ProjectSnapshotPage>(page.clone());
    accepts_extra::<calls::ProjectSnapshotPage>(page);
    round::<calls::ProjectView>(json!({}));
    round::<calls::ProjectView>(json!({ "budget": 2000, "range": "2s..5s" }));
    round::<calls::ProjectViewText>(json!({ "text": "timeline main" }));
    accepts_extra::<calls::ProjectViewText>(json!({ "text": "timeline main" }));
    let req = json!({
        "method": "POST", "url": "https://api.example.org/v1/x",
        "headers": [{ "name": "content-type", "value": "application/json" }],
        "bodyB64": "e30="
    });
    round::<calls::HttpRequest>(req.clone());
    refuses_extra::<calls::HttpRequest>(req.clone());
    round::<calls::HttpRequest>(json!({ "method": "GET", "url": "https://api.example.org/" }));
    assert!(
        serde_json::from_value::<calls::HttpRequest>(
            json!({ "method": "CONNECT", "url": "https://x.org/" })
        )
        .is_err()
    );
    round::<calls::NetFetch>(json!({ "request": req }));
    round::<calls::NetFetch>(
        json!({ "request": { "method": "GET", "url": "https://a.org/" }, "grant": "example" }),
    );
    let reply = json!({
        "status": 302, "headers": [{ "name": "location", "value": "https://b.org/" }],
        "handle": "h1", "length": 0, "redirectNotFollowed": true
    });
    round::<calls::NetFetchReply>(reply.clone());
    accepts_extra::<calls::NetFetchReply>(reply);
    round::<calls::NetFetchReply>(
        json!({ "status": 200, "headers": [], "handle": "h2", "length": 12 }),
    );
    round::<calls::NetRead>(json!({ "handle": "h2", "offset": 0, "length": 774144 }));
    round::<calls::NetReadPage>(json!({ "bytesB64": "aGVsbG8=", "eof": true }));
    accepts_extra::<calls::NetReadPage>(json!({ "bytesB64": "aGVsbG8=", "eof": true }));
    // Plugin → host payloads stay strict.
    refuses_extra::<calls::NetRead>(json!({ "handle": "h2", "offset": 0, "length": 1 }));
    refuses_extra::<calls::Log>(json!({ "level": "info", "msg": "hi" }));
    refuses_extra::<calls::ProjectView>(json!({}));
    for level in ["debug", "info", "warn", "error"] {
        round::<calls::Log>(json!({ "level": level, "msg": "hi" }));
    }
    assert!(serde_json::from_value::<calls::Log>(json!({ "level": "fatal", "msg": "x" })).is_err());
}

#[test]
fn host_methods_are_named() {
    for (m, name) in [
        (HostMethod::ProjectSnapshot, "project.snapshot"),
        (HostMethod::ProjectView, "project.view"),
        (HostMethod::NetFetch, "net.fetch"),
        (HostMethod::NetRead, "net.read"),
        (HostMethod::Log, "log"),
    ] {
        assert_eq!(m.name(), name);
        assert_eq!(HostMethod::from_name(name), Some(m));
        assert_eq!(round::<HostMethod>(json!(name)), m);
    }
    assert_eq!(HostMethod::from_name("job.submit"), None);
}

#[test]
fn plugin_error_round_trips() {
    let v =
        json!({ "code": "PLUGIN_TIMEOUT", "message": "too slow", "details": { "timeoutS": 10 } });
    round::<PluginError>(v.clone());
    refuses_extra::<PluginError>(v);
    let v =
        json!({ "code": "SECRET_MISSING", "message": "no secret", "details": {}, "fix": "ask" });
    round::<PluginError>(v);
    let e: PluginError = serde_json::from_value(json!({ "code": "X", "message": "m" })).unwrap();
    assert!(e.details.is_empty());
}

#[test]
fn job_rights_round_trip() {
    let v = json!({
        "principal": { "kind": "agent", "id": "S1" },
        "class": "agent:S0",
        "scopes": ["project.read", "project.write"],
        "projects": ["p1"],
        "branch": { "root": "b1", "subtree": true },
        "libraries": "all",
        "grants": [{ "perm": "project:read" }, { "perm": "project:write" }],
        "rightsHash": "0123456789abcdef0123456789abcdef",
        "generation": 3
    });
    let r: JobRights = round(v.clone());
    assert_eq!((r.rights_hash.len(), r.generation), (32, 3));
    for field in ["rightsHash", "generation"] {
        let mut w = v.clone();
        w.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<JobRights>(w).is_err(), "{field}");
    }
    assert!(r.projects.includes("p1") && !r.projects.includes("p2"));
    assert!(r.libraries.includes("any"));
    refuses_extra::<JobRights>(v.clone());
    let mut no_branch = v.clone();
    no_branch.as_object_mut().unwrap().remove("branch");
    assert_eq!(round::<JobRights>(no_branch).branch, None);
    // No scope outside the four of spec §3.7 parses: a human-only or admin scope is not representable.
    for bad in [
        "project.admin",
        "auth.admin",
        "branch.protected",
        "review.decide",
        "diagnostics.read",
        "plugin.admin",
    ] {
        let mut w = v.clone();
        w["scopes"] = json!([bad]);
        assert!(serde_json::from_value::<JobRights>(w).is_err(), "{bad}");
    }
    let names: Vec<_> = PluginScope::ALL
        .iter()
        .map(|s| (s.name(), s.permission()))
        .collect();
    assert_eq!(
        names,
        [
            ("project.read", "project:read"),
            ("project.write", "project:write"),
            ("library.read", "media:read"),
            ("jobs.submit", "jobs:submit")
        ]
    );
    assert!(serde_json::from_value::<Reach>(json!("some")).is_err());
}

#[test]
fn contributions_round_trip() {
    let c = json!({ "name": "trim-silence", "title": "cmd.trimSilence", "args": { "type": "object", "properties": {}, "additionalProperties": false }, "writes": true, "timeoutS": 30 });
    let t: CommandContribution = round(c.clone());
    assert_eq!(t.timeout_s, Some(30));
    refuses_extra::<CommandContribution>(c);
    let c = json!({ "name": "fetch-title", "title": "cmd.fetchTitle", "args": { "type": "object", "properties": {}, "additionalProperties": false }, "writes": false });
    round::<CommandContribution>(c);
    let h = json!({ "event": "PostCommand", "handler": "on-post" });
    round::<HookContribution>(h.clone());
    round::<HookContribution>(json!({ "event": "PreMerge", "handler": "gate", "timeoutS": 60 }));
    refuses_extra::<HookContribution>(h);
}

#[test]
fn host_api_consumes_commands_from_1_1() {
    assert_eq!(
        consumable(&Version::new(1, 1, 0)),
        [
            ContributionKind::FontCatalogs,
            ContributionKind::CaptionStyles,
            ContributionKind::Commands
        ]
    );
    // A 1.0 host does not consume commands.
    assert!(!consumable(&Version::new(1, 0, 0)).contains(&ContributionKind::Commands));
    // A code plugin with a command validates and keeps it.
    let m = check_with(
        &with_commands(json!([command("trim-silence")])),
        &files(),
        &ctx(SourceClass::Registry),
    )
    .unwrap();
    assert_eq!(m.contributes.commands[0].name, "trim-silence");
    assert_eq!(m.contributes.kinds(), [ContributionKind::Commands]);
    // The manifest serialises it back.
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(
        v["contributes"]["commands"][0]["name"],
        json!("trim-silence")
    );
    // A 1.0 host refuses it.
    let mut c = ctx(SourceClass::Registry);
    c.host_api = Version::new(1, 0, 0);
    let errs = check_with(&with_commands(json!([command("x")])), &files(), &c).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].code, "PLUGIN_CONTRIBUTION_UNSUPPORTED");
    assert_eq!(errs[0].details["kind"], json!("commands"));
}

/// `code-node` (observes `PostCommand` and `PreCommand`) contributing `hooks`.
fn with_hooks(hooks: Value) -> Value {
    set(
        fixture("code-node"),
        "/contributes",
        json!({ "hooks": hooks }),
    )
}

fn check_hooks(hooks: Value) -> Vec<fin_plugin_api::Invalid> {
    check(&with_hooks(hooks)).err().unwrap_or_default()
}

#[test]
fn hooks_consumable_from_new_minor() {
    // 7F Decision 20: `hooks` is consumed from host API 1.2, the minor after 8B's 1.1.
    assert_eq!(HOST_API, (1, 2, 0));
    assert_eq!(host_api(), Version::new(1, 2, 0));
    assert_eq!(
        consumable(&host_api()),
        [
            ContributionKind::FontCatalogs,
            ContributionKind::CaptionStyles,
            ContributionKind::Commands,
            ContributionKind::Hooks
        ]
    );
    assert!(!consumable(&Version::new(1, 1, 0)).contains(&ContributionKind::Hooks));
    let hooks = json!([
        { "event": "PostCommand", "handler": "on-post" },
        { "event": "PreCommand", "handler": "guard", "timeoutS": 30 }
    ]);
    let m = check(&with_hooks(hooks.clone())).unwrap();
    let h = m.contributes.hooks.as_deref().unwrap();
    assert_eq!(h[0].event, HookEvent::PostCommand);
    assert_eq!(h[1].handler, "guard");
    assert_eq!(h[1].timeout_s, Some(30));
    assert_eq!(m.contributes.kinds(), [ContributionKind::Hooks]);
    // The manifest serialises them back unchanged.
    assert_eq!(
        serde_json::to_value(&m).unwrap()["contributes"]["hooks"],
        hooks
    );
    // Still refused below the new minor.
    let mut c = ctx(SourceClass::Registry);
    c.host_api = Version::new(1, 1, 0);
    let errs = check_with(&with_hooks(hooks), &MemFiles::full(), &c).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].code, "PLUGIN_CONTRIBUTION_UNSUPPORTED");
    assert_eq!(errs[0].details["kind"], json!("hooks"));
    assert_eq!(errs[0].details["hostApi"], json!("1.1.0"));
    // A malformed hook entry is a schema error, not silently dropped.
    assert_eq!(
        reasons(check(&with_hooks(json!([{ "event": "PostCommand" }])))),
        ["schema"]
    );
    // A content pack has no hooks.
    let v = set(
        fixture("content-pack"),
        "/contributes/hooks",
        json!([{ "event": "PostCommand", "handler": "on-post" }]),
    );
    let r = reasons(check(&v));
    assert!(r.contains(&"contentHooks".to_string()), "{r:?}");
}

#[test]
fn hook_needs_observe_for_its_event() {
    let errs = check_hooks(json!([{ "event": "SessionStart", "handler": "greet" }]));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].path(), Some("/contributes/hooks/0/event"));
    assert_eq!(errs[0].reason(), Some("hookNotObserved"));
    assert_eq!(errs[0].details["event"], json!("SessionStart"));
    // `hooks:modify` alone is not `hooks:observe` (and is refused without it anyway).
    let v = set(
        with_hooks(json!([{ "event": "PreCommand", "handler": "guard" }])),
        "/permissions/0",
        json!({ "perm": "hooks:observe", "hooks": ["PostCommand"] }),
    );
    let r = reasons(check(&v));
    assert!(r.contains(&"hookNotObserved".to_string()), "{r:?}");
    // No `hooks:observe` at all.
    let v = set(
        with_hooks(json!([{ "event": "PostCommand", "handler": "on-post" }])),
        "/permissions",
        json!([]),
    );
    assert_eq!(reasons(check(&v)), ["hookNotObserved"]);
}

#[test]
fn unknown_event_refused() {
    // In a contribution: `PreOp`/`PostOp` were cut from v1, and names are exact.
    for bad in ["PreOp", "PostOp", "postCommand", "Whatever"] {
        let r = reasons(check(&with_hooks(
            json!([{ "event": bad, "handler": "h" }]),
        )));
        assert_eq!(r, ["schema"], "{bad}");
    }
    // In a permission: `hooks:observe` and `hooks:modify` name events.
    let at = |perms: Value, path: &str| {
        let errs = check(&set(fixture("code-node"), "/permissions", perms)).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.reason() == Some("hooks") && e.path() == Some(path)),
            "{path}: {errs:?}"
        );
    };
    at(
        json!([{ "perm": "hooks:observe", "hooks": ["PreCommand", "PreOp"] }]),
        "/permissions/0/hooks/1",
    );
    at(
        json!([
            { "perm": "hooks:observe", "hooks": ["PreCommand", "PreOp"] },
            { "perm": "hooks:modify", "hooks": ["PreOp"] }
        ]),
        "/permissions/1/hooks/0",
    );
}

#[test]
fn duplicate_handler_refused() {
    let errs = check_hooks(json!([
        { "event": "PostCommand", "handler": "on-post" },
        { "event": "PostCommand", "handler": "on-post" }
    ]));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].path(), Some("/contributes/hooks/1/handler"));
    assert_eq!(errs[0].reason(), Some("duplicateHook"));
    // One handler on two events, and two handlers on one event, are fine.
    assert!(
        check_hooks(json!([
            { "event": "PostCommand", "handler": "audit" },
            { "event": "PreCommand", "handler": "audit" },
            { "event": "PreCommand", "handler": "guard" }
        ]))
        .is_empty()
    );
    // Handler names: `[a-z][a-z0-9-]{0,63}`.
    for bad in ["", "On-post", "-post", "9lives", "on_post", &"a".repeat(65)] {
        let errs = check_hooks(json!([{ "event": "PostCommand", "handler": bad }]));
        assert_eq!(errs.len(), 1, "{bad}: {errs:?}");
        assert_eq!(errs[0].path(), Some("/contributes/hooks/0/handler"));
        assert_eq!(errs[0].reason(), Some("hookHandler"), "{bad}");
    }
    assert!(check_hooks(json!([{ "event": "PostCommand", "handler": "a".repeat(64) }])).is_empty());
}

#[test]
fn hook_timeout_bounds() {
    use fin_plugin_api::protocol::{DEFAULT_HOOK_TIMEOUT_S, MAX_HOOK_TIMEOUT_S};
    assert_eq!((DEFAULT_HOOK_TIMEOUT_S, MAX_HOOK_TIMEOUT_S), (10, 60));
    for (t, ok) in [
        (1, true),
        (10, true),
        (60, true),
        (0, false),
        (61, false),
        (600, false),
    ] {
        let errs =
            check_hooks(json!([{ "event": "PreCommand", "handler": "guard", "timeoutS": t }]));
        assert_eq!(errs.is_empty(), ok, "{t}: {errs:?}");
        if !ok {
            assert_eq!(errs[0].path(), Some("/contributes/hooks/0/timeoutS"));
            assert_eq!(errs[0].reason(), Some("hookTimeout"));
        }
    }
}

#[test]
fn modify_permission_still_parses_and_validates() {
    // 7F Decision 19: `hooks:modify` grants nothing in v1, but a manifest may still name it
    // (high risk at install), within `hooks:observe`, with event names.
    let m = check(&with_hooks(
        json!([{ "event": "PreCommand", "handler": "guard" }]),
    ))
    .unwrap();
    assert!(m.permissions.iter().any(|p| matches!(
        p,
        fin_plugin_api::Permission::HooksModify { hooks } if hooks == &["PreCommand"]
    )));
    let v = set(
        fixture("code-node"),
        "/permissions/1/hooks",
        json!(["PostCommand"]),
    );
    assert!(check(&v).is_ok());
    let v = set(
        fixture("code-node"),
        "/permissions/1/hooks",
        json!(["SessionStart"]),
    );
    assert_eq!(reasons(check(&v)), ["modifyWithoutObserve"]);
    let v = set(
        fixture("code-node"),
        "/permissions/1/hooks",
        json!(["PreOp"]),
    );
    let r = reasons(check(&v));
    assert!(r.contains(&"hooks".to_string()), "{r:?}");
}

fn command(name: &str) -> Value {
    json!({
        "name": name, "title": "cmd.trimSilence", "writes": true,
        "args": {
            "type": "object",
            "properties": { "threshold": { "type": "number", "description": "arg.threshold" } },
            "required": ["threshold"],
            "additionalProperties": false
        }
    })
}

fn files() -> MemFiles {
    let en = json!({ "cmd.trimSilence": "Trim silence", "arg.threshold": "Threshold" });
    MemFiles::full().with("i18n/en.json", en.to_string().as_bytes())
}

fn with_commands(commands: Value) -> Value {
    let v = set(
        fixture("code-node"),
        "/contributes",
        json!({ "commands": commands }),
    );
    let v = set(v, "/messages", json!("i18n"));
    set(v, "/locales", json!(["en"]))
}

fn check_cmds(commands: Value) -> Vec<fin_plugin_api::Invalid> {
    check_with(
        &with_commands(commands),
        &files(),
        &ctx(SourceClass::Registry),
    )
    .err()
    .unwrap_or_default()
}

#[test]
fn command_rules() {
    assert!(check_cmds(json!([command("trim-silence"), command("other")])).is_empty());
    // Names.
    for bad in ["", "Trim", "-trim", "trim_silence", &"a".repeat(65)] {
        let errs = check_cmds(json!([command(bad)]));
        assert_eq!(errs.len(), 1, "{bad}: {errs:?}");
        assert_eq!(errs[0].path(), Some("/contributes/commands/0/name"));
        assert_eq!(errs[0].reason(), Some("commandName"));
    }
    assert!(check_cmds(json!([command(&"a".repeat(64))])).is_empty());
    // Duplicates.
    let errs = check_cmds(json!([command("trim"), command("trim")]));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].path(), Some("/contributes/commands/1/name"));
    assert_eq!(errs[0].reason(), Some("duplicateCommand"));
    // Timeouts: 1 to 60 s.
    for (t, ok) in [(1, true), (60, true), (61, false), (0, false)] {
        let c = set(command("trim"), "/timeoutS", json!(t));
        let errs = check_cmds(json!([c]));
        assert_eq!(errs.is_empty(), ok, "{t}: {errs:?}");
        if !ok {
            assert_eq!(errs[0].path(), Some("/contributes/commands/0/timeoutS"));
            assert_eq!(errs[0].reason(), Some("commandTimeout"));
        }
    }
    // Titles and argument descriptions are message keys in the `en` catalogue.
    let c = set(command("trim"), "/title", json!("cmd.missing"));
    let errs = check_cmds(json!([c]));
    assert_eq!(errs[0].reason(), Some("messageKeyMissing"), "{errs:?}");
    assert_eq!(errs[0].path(), Some("/contributes/commands/0/title"));
    let c = set(
        command("trim"),
        "/args/properties/threshold/description",
        json!("arg.missing"),
    );
    let errs = check_cmds(json!([c]));
    assert_eq!(errs[0].reason(), Some("messageKeyMissing"), "{errs:?}");
    assert_eq!(
        errs[0].path(),
        Some("/contributes/commands/0/args/properties/threshold/description")
    );
    // A content pack has no commands.
    let v = set(
        fixture("content-pack"),
        "/contributes/commands",
        json!([command("trim")]),
    );
    let r = reasons(check_with(&v, &files(), &ctx(SourceClass::Registry)));
    assert!(r.contains(&"contentCommands".to_string()), "{r:?}");
}

#[test]
fn schema_outside_subset_refused() {
    for (pointer, value) in [
        ("/args/properties/threshold/$ref", json!("#/definitions/x")),
        ("/args/properties/threshold/format", json!("email")),
        ("/args/oneOf", json!([{ "type": "object" }])),
        ("/args/additionalProperties", json!(true)),
        ("/args/type", json!("string")),
    ] {
        let c = set(command("trim"), pointer, value);
        let errs = check_cmds(json!([c]));
        assert_eq!(errs.len(), 1, "{pointer}: {errs:?}");
        assert_eq!(errs[0].code, "PLUGIN_MANIFEST_INVALID");
        assert_eq!(errs[0].path(), Some("/contributes/commands/0/args"));
        assert_eq!(errs[0].reason(), Some("argsSchema"), "{pointer}");
    }
    // The problem says where in the schema.
    let c = set(
        command("trim"),
        "/args/properties/threshold/format",
        json!("email"),
    );
    let errs = check_cmds(json!([c]));
    assert_eq!(errs[0].details["at"], json!("/properties/threshold/format"));
    assert_eq!(errs[0].details["problem"], json!("keyword"));
}

#[test]
fn pattern_heavy_schema_refused_quickly_at_install() {
    let props: serde_json::Map<String, Value> = (0..2000)
        .map(|i| {
            (
                format!("p{i}"),
                json!({ "type": "string", "pattern": "[a-z]{1,8}" }),
            )
        })
        .collect();
    let c = set(
        command("trim"),
        "/args",
        json!({ "type": "object", "properties": props, "additionalProperties": false }),
    );
    let t = std::time::Instant::now();
    let errs = check_cmds(json!([c]));
    assert!(
        t.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        t.elapsed()
    );
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].reason(), Some("argsSchema"));
    assert_eq!(errs[0].details["problem"], json!("budget"));
}

#[test]
fn a_base64_page_leaves_room_in_a_line() {
    use fin_plugin_api::protocol::{MAX_MESSAGE_BYTES, PAGE_BYTES};
    assert_eq!(PAGE_BYTES, 756 * 1024);
    let b64 = PAGE_BYTES.div_ceil(3) * 4;
    assert_eq!(b64, 1_032_192);
    assert!(
        MAX_MESSAGE_BYTES - b64 >= 16 * 1024,
        "room for the reply's JSON"
    );
}

#[test]
fn plugin_codes_are_named() {
    use fin_plugin_api::protocol::errors::codes;
    assert_eq!(codes::PLUGIN_COMMAND_FAILED, "PLUGIN_COMMAND_FAILED");
    assert_eq!(codes::PLUGIN_OUTPUT_INVALID, "PLUGIN_OUTPUT_INVALID");
}
