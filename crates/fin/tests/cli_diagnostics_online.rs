//! `fin` and `fin diagnostics` against a real `finisherd` (plan D, Task D6): what a run
//! records (names, never values), the actor, the panic report, `FIN_SERVER` and the server's
//! gate, and `show`, `export`, `purge`, `project` and `consent`. Entries are written by the
//! server asynchronously, so tests poll `export` until what they expect is there. Since 5A
//! Task 7 consent changes, purges, `project default` and content exports are a person's: `fin`
//! is refused (`HUMAN_ONLY`, exit 7) and the tests make those changes as the console.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::console::{Console, error_code};
use support::{Server, assert_error, fin, fin_cmd, fin_env, ok};

const SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const CALL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

/// A server and a project `<root>/film`.
fn project(test: &str, env: Vec<(String, OsString)>) -> Option<(Server, PathBuf, String)> {
    let server = Server::start_with(test, env, None)?;
    let created = ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let id = created["project"]["projectId"]
        .as_str()
        .or_else(|| created["projectId"].as_str());
    let id = id
        .unwrap_or_else(|| panic!("no project id: {created}"))
        .to_string();
    let dir = server.root.join("film");
    Some((server, dir, id))
}

/// Runs `fin`, returning the exit code and raw stdout (`export` prints JSONL).
fn raw(sd: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let mut cmd = fin_cmd(sd, cwd, args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

fn lines(sd: &Path, cwd: &Path, args: &[&str]) -> Vec<Value> {
    let mut a = vec!["diagnostics", "export"];
    a.extend_from_slice(args);
    let (code, out) = raw(sd, cwd, &a, &[]);
    assert_eq!(code, 0, "{out}");
    out.lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect()
}

/// A whole number of seconds from the environment variable `k`, else `d`.
fn env_secs(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

/// Polls `export` until `want` holds (the server writes entries a moment after the call). The
/// deadline is generous (60 s, `FIN_TEST_WAIT_SECS`): a loaded machine is slow, never wrong.
fn wait(sd: &Path, cwd: &Path, args: &[&str], want: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    let end = Instant::now() + Duration::from_secs(env_secs("FIN_TEST_WAIT_SECS", 60));
    loop {
        let found = lines(sd, cwd, args);
        if want(&found) || Instant::now() > end {
            assert!(want(&found), "timed out: {found:?}");
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `fin` is not a person: each of these is `HUMAN_ONLY` (exit 7) and changes nothing.
fn refused_to_fin(sd: &Path, cwd: &Path, args: &[&str]) {
    let r = fin(sd, cwd, args);
    assert_error(&r, 7, "HUMAN_ONLY");
    assert_eq!(
        r.json["error"]["details"]["scope"], "diagnostics.admin",
        "{args:?}"
    );
}

fn show(sd: &Path, cwd: &Path) -> Value {
    ok(fin(sd, cwd, &["diagnostics", "show"]))
}

fn commands(entries: &[Value], name: &str) -> Vec<Value> {
    entries
        .iter()
        .filter(|e| e["kind"] == "cli.command" && e["payload"]["name"] == name)
        .cloned()
        .collect()
}

/// Every file under `dir`, as (path, bytes).
fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push((p.clone(), std::fs::read(&p).unwrap()));
        }
    }
    out
}

fn contains(hay: &[u8], needle: &str) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// `uname -n`, else `hostname` (macOS has no /etc/hostname).
fn host_name() -> String {
    for (program, args) in [("uname", &["-n"][..]), ("hostname", &[][..])] {
        if let Ok(out) = std::process::Command::new(program).args(args).output()
            && out.status.success()
        {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
    }
    String::new()
}

fn marker_edit(dir: &Path, sd: &Path, label: &str) -> Value {
    ok(fin(
        sd,
        dir,
        &["marker", "add", "--at", "1s", "--label", label],
    ))
}

#[test]
fn diagnostics_show() {
    let Some((server, dir, _)) = project("diagnostics_show", vec![]) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    marker_edit(&dir, sd, "one");
    wait(sd, root, &["--kind", "cli.command"], |e| {
        commands(e, "marker.add").len() == 1
    });
    let r = show(sd, root);
    assert_eq!(r["consent"]["diagnostics"], "on");
    assert_eq!(r["consent"]["basis"], "default");
    assert_eq!(r["consent"]["training"], "undecided");
    assert_eq!(r["env"], r["consent"]["env"]);
    assert_eq!(r["state"], "on");
    assert!(r["kinds"]["cli.command"].as_u64().unwrap() >= 2, "{r}");
    assert!(r["kinds"]["session.start"].as_u64().unwrap() >= 1, "{r}");
    assert!(
        r["files"].as_u64().unwrap() >= 1 && r["bytes"].as_u64().unwrap() > 0,
        "{r}"
    );
    // `export --limit n` lists the newest entries; `show` takes no `--limit`.
    assert!(r.get("recent").is_none());
    assert_error(
        &fin(sd, root, &["diagnostics", "show", "--limit", "2"]),
        2,
        "USAGE",
    );
}

#[test]
fn cli_command_has_no_values() {
    let Some((server, dir, _)) = project("cli_command_has_no_values", vec![]) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    let marker = "ZQXMARKER4471";
    ok(fin(
        sd,
        &dir,
        &[
            "marker", "add", "--at", "1s", "--label", marker, "--color", "#ff00aa",
        ],
    ));
    ok(fin(
        sd,
        &dir,
        &[
            "marker",
            "add",
            "--at",
            "2s",
            "--label",
            "second",
            "--idem-key",
            &format!("idem-{marker}"),
        ],
    ));
    ok(fin(
        sd,
        root,
        &[
            "project",
            "create",
            &format!("dir-{marker}"),
            "--name",
            &format!("name {marker}"),
        ],
    ));
    let all = wait(sd, root, &["--kind", "cli.command"], |e| {
        commands(e, "marker.add").len() == 2 && commands(e, "project.create").len() == 2
    });
    let add = &commands(&all, "marker.add")[0]["payload"];
    assert_eq!(
        add["flags"],
        json!(["at", "color", "label"]),
        "names only: {add}"
    );
    // Everything on disk: no marker, no temp path, no home, no user, and no unkeyed hash.
    let host = host_name();
    assert!(!host.is_empty(), "the host name is part of the leak check");
    let secrets = [
        marker.to_string(),
        root.display().to_string(),
        std::env::var("HOME").unwrap_or_default(),
        std::env::var("USER").unwrap_or_default(),
        host,
        "blake3:".to_string(),
    ];
    let files = files_under(&sd.join("events"));
    assert!(!files.is_empty());
    for (path, bytes) in files
        .iter()
        .filter(|(p, _)| p.file_name().is_none_or(|n| n != "key"))
    {
        for s in &secrets {
            assert!(!contains(bytes, s), "{s:?} in {}", path.display());
        }
    }
}

#[test]
fn args_hash_is_keyed_on_disk() {
    let Some((server, dir, _)) = project("args_hash_is_keyed_on_disk", vec![]) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    marker_edit(&dir, sd, "one");
    let all = wait(sd, root, &["--kind", "cli.command"], |e| {
        !commands(e, "marker.add").is_empty()
    });
    let hash = commands(&all, "marker.add")[0]["payload"]["argsHash"]
        .as_str()
        .unwrap()
        .to_string();
    let hex_part = hash
        .strip_prefix("k1:")
        .unwrap_or_else(|| panic!("not keyed: {hash}"));
    assert!(
        hex_part.len() == 32 && hex_part.bytes().all(|c| c.is_ascii_hexdigit()),
        "{hash}"
    );
    for (path, bytes) in files_under(&sd.join("events")) {
        assert!(!contains(&bytes, "blake3:"), "{}", path.display());
    }
}

#[test]
fn fin_server_sends_and_server_gates() {
    let off = vec![("FIN_DIAGNOSTICS".to_string(), OsString::from("off"))];
    let Some(mut server) = Server::start_with("fin_server_sends_and_server_gates", off, None)
    else {
        return;
    };
    let (sd, root) = (server.sd.clone(), server.root.clone());
    let socket = OsString::from(&server.socket);
    // `FIN_SERVER` skips finisherd.json, so `fin` sends; the server, recording off, stores nothing.
    ok(fin_env(
        &sd,
        &root,
        &["project", "create", "film", "--name", "Film"],
        &[("FIN_SERVER", &socket)],
    ));
    std::thread::sleep(Duration::from_millis(500));
    assert!(!sd.join("events").exists(), "recording off creates nothing");
    // The same run against a recording server stores it.
    server.env.clear();
    server.restart();
    let socket = OsString::from(&server.socket);
    ok(fin_env(
        &sd,
        &root,
        &["project", "open", "film"],
        &[("FIN_SERVER", &socket)],
    ));
    let (code, out) = raw(
        &sd,
        &root,
        &["diagnostics", "export", "--kind", "cli.command"],
        &[("FIN_SERVER", socket.to_str().unwrap())],
    );
    assert_eq!(code, 0);
    let _ = out;
    wait(&sd, &root, &["--kind", "cli.command"], |e| {
        !commands(e, "project.open").is_empty()
    });
}

#[test]
fn agent_actor_from_fin_session_and_fin_call() {
    let Some((server, dir, _)) = project("agent_actor_from_fin_session_and_fin_call", vec![])
    else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    fin_env(
        sd,
        &dir,
        &["marker", "add", "--at", "1s", "--label", "a"],
        &[
            ("FIN_SESSION", SESSION.as_ref()),
            ("FIN_CALL", CALL.as_ref()),
        ],
    );
    // A user run, and an agent whose ids are not ULIDs: keyed, and the same key every time.
    marker_edit(&dir, sd, "u");
    let odd = [
        ("FIN_SESSION", "sess-1".as_ref()),
        ("FIN_CALL", "call-1".as_ref()),
    ];
    fin_env(
        sd,
        &dir,
        &["marker", "add", "--at", "3s", "--label", "o"],
        &odd,
    );
    fin_env(
        sd,
        &dir,
        &["marker", "add", "--at", "4s", "--label", "p"],
        &odd,
    );
    let all = wait(sd, root, &["--kind", "cli.command"], |e| {
        commands(e, "marker.add").len() == 4
    });
    let actors: Vec<Value> = commands(&all, "marker.add")
        .iter()
        .map(|e| e["ctx"]["actor"].clone())
        .collect();
    assert_eq!(
        actors[0],
        json!({ "kind": "agent", "session": SESSION, "call": CALL })
    );
    assert_eq!(actors[1], json!({ "kind": "user" }));
    assert_eq!(actors[2]["kind"], "agent");
    assert!(
        actors[2]["session"].as_str().unwrap().starts_with("k1:"),
        "{}",
        actors[2]
    );
    assert_eq!(actors[2], actors[3]);
    // Every `cli.command` carries the project it ran against.
    assert!(
        commands(&all, "marker.add")
            .iter()
            .all(|e| e["ctx"]["project"].is_string())
    );
}

#[test]
fn panic_reports_then_prints_internal() {
    let Some((server, dir, _)) = project("panic_reports_then_prints_internal", vec![]) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    let (code, out) = raw(sd, &dir, &["status"], &[("FIN_TEST_PANIC", "1")]);
    assert_eq!(code, 6, "{out}");
    let doc: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(doc["error"]["code"], "INTERNAL");
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("fin crashed:"),
        "{doc}"
    );
    let found = wait(sd, root, &["--kind", "error.panic"], |e| e.len() == 1);
    let p = &found[0]["payload"];
    assert_eq!(
        (&p["where"], &p["thirdParty"], &p["thread"]),
        (&json!("fin"), &json!(false), &json!("main"))
    );
    assert!(
        p["location"]
            .as_str()
            .unwrap()
            .starts_with("crates/fin/src/"),
        "{p}"
    );
    assert!(p["messageHash"].as_str().unwrap().starts_with("k1:"), "{p}");
    assert!(
        p.get("message").is_none(),
        "the message is content: training only, {p}"
    );
    assert_eq!(found[0]["source"], "fin");
    // `FIN_DIAGNOSTICS=off`: the same crash reports nothing, and still prints its document.
    let (code, _) = raw(
        sd,
        &dir,
        &["status"],
        &[("FIN_TEST_PANIC", "1"), ("FIN_DIAGNOSTICS", "off")],
    );
    assert_eq!(code, 6);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(lines(sd, root, &["--kind", "error.panic"]).len(), 1);
}

#[test]
fn export_is_redacted_for_fin_and_content_needs_a_person() {
    let training = vec![("FIN_TRAINING".to_string(), OsString::from("on"))];
    let Some((server, dir, _)) = project("export_redacted_for_fin", training) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    let (code, _) = raw(sd, &dir, &["status"], &[("FIN_TEST_PANIC", "1")]);
    assert_eq!(code, 6);
    // Under training the panic's message is stored; `fin` is not a person and gets it removed.
    let found = wait(sd, root, &["--kind", "error.panic"], |e| e.len() == 1);
    assert!(found[0]["payload"].get("message").is_none(), "{}", found[0]);
    assert!(found[0]["payload"]["messageHash"].is_string());
    let r = fin(sd, root, &["diagnostics", "export", "--content"]);
    assert_error(&r, 7, "HUMAN_ONLY");
    assert_eq!(r.json["error"]["details"]["scope"], "diagnostics.admin");
    // The console, a person, reads what was stored (reading needs no typed code).
    let mut console = Console::open(&server);
    let page = console.call(
        "diagnostics.export",
        json!({ "filter": { "kinds": ["error.panic"] }, "content": true }),
    );
    assert_eq!(page["_tag"], "Success", "{page}");
    assert!(page["value"].get("redacted").is_none(), "{page}");
    assert!(
        page["value"]["lines"][0]["payload"]["message"].is_string(),
        "{page}"
    );
}

#[test]
fn diagnostics_export_filters_limit_and_as_upload() {
    let Some((server, dir, id)) = project("diagnostics_export_filters", vec![]) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    for l in ["a", "b", "c"] {
        marker_edit(&dir, sd, l);
    }
    let all = wait(sd, root, &[], |e| commands(e, "marker.add").len() == 3);
    let stamps: Vec<u64> = all.iter().map(|e| e["ts"].as_u64().unwrap()).collect();
    assert!(stamps.windows(2).all(|w| w[0] <= w[1]), "newest last");
    let kinds = lines(
        sd,
        root,
        &["--kind", "session.start", "--kind", "consent.changed"],
    );
    assert!(
        kinds
            .iter()
            .all(|e| e["kind"] == "session.start" || e["kind"] == "consent.changed")
    );
    assert!(!kinds.is_empty());
    let cli = lines(sd, root, &["--kind", "cli.command"]);
    let two = lines(sd, root, &["--kind", "cli.command", "--limit", "2"]);
    assert_eq!(two.len(), 2);
    // Each export records itself, so the lists grow between calls: compare order, not identity.
    assert!(cli.len() >= 3);
    assert!(two[0]["ts"].as_u64() <= two[1]["ts"].as_u64());
    assert!(lines(sd, root, &["--since", "2999-01-01"]).is_empty());
    assert!(!lines(sd, root, &["--since", "2000-01-01"]).is_empty());
    let mine = lines(sd, root, &["--project", &id]);
    assert!(
        mine.len() >= 3 && mine.iter().all(|e| e["ctx"]["project"] == id.as_str()),
        "{mine:?}"
    );
    // `--project` takes the folder as well.
    let by_dir = raw(
        sd,
        root,
        &[
            "diagnostics",
            "export",
            "--project",
            dir.to_str().unwrap(),
            "--kind",
            "cli.command",
        ],
        &[],
    );
    assert_eq!(by_dir.0, 0);
    assert!(by_dir.1.lines().count() >= 3);
    // The upload view: no training entries, nothing the full export lacks.
    assert!(lines(sd, root, &["--as-upload", "training"]).is_empty());
    let upload = lines(sd, root, &["--as-upload", "diagnostics"]);
    let stored = lines(sd, root, &[]);
    assert!(
        upload
            .iter()
            .all(|e| stored.iter().any(|s| s["id"] == e["id"]))
    );
    let (code, out) = raw(
        sd,
        root,
        &["diagnostics", "export", "--since", "last tuesday"],
        &[],
    );
    assert_eq!(code, 2, "{out}");
    let (code, _) = raw(
        sd,
        root,
        &["diagnostics", "export", "--as-upload", "everything"],
        &[],
    );
    assert_eq!(code, 2);
}

#[test]
fn diagnostics_purge_selectors() {
    let Some((server, dir, id)) = project("diagnostics_purge_selectors", vec![]) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    marker_edit(&dir, sd, "a");
    wait(sd, root, &[], |e| !commands(e, "marker.add").is_empty());
    for args in [
        &["diagnostics", "purge", "--before", "2000-01-01"][..],
        &["diagnostics", "purge", "--project", &id],
        &["diagnostics", "purge", "--all"],
    ] {
        refused_to_fin(sd, root, args);
    }
    assert!(!lines(sd, root, &["--project", &id]).is_empty());
    let mut console = Console::open(&server);
    let purge = |c: &mut Console, selector: Value| {
        c.confirmed_ok("diagnostics.purge", json!({ "selector": selector }))
    };
    let none = purge(&mut console, json!({ "before": 946_684_800_000_u64 }));
    assert_eq!(none["removed"], 0, "{none}");
    let mine = purge(&mut console, json!({ "project": id }));
    assert!(mine["removed"].as_u64().unwrap() >= 1, "{mine}");
    assert!(lines(sd, root, &["--project", &id]).is_empty());
    assert!(!lines(sd, root, &[]).is_empty(), "other entries stay");
    marker_edit(&dir, sd, "b");
    wait(sd, root, &[], |e| !commands(e, "marker.add").is_empty());
    let before = purge(&mut console, json!({ "before": 32_503_680_000_000_u64 }));
    assert!(before["removed"].as_u64().unwrap() >= 1, "{before}");
    assert!(lines(sd, root, &[]).is_empty());
    marker_edit(&dir, sd, "c");
    wait(sd, root, &[], |e| !commands(e, "marker.add").is_empty());
    let all = purge(&mut console, json!("all"));
    assert!(all["removed"].as_u64().unwrap() >= 1, "{all}");
    assert!(all["files"].is_u64() && all["blobs"].is_u64());
    // A purge is not recorded: nothing, not even a key, is left behind.
    assert!(!sd.join("events/key").exists());
    assert!(!sd.join("events/blobs").exists());
}

#[test]
fn diagnostics_purge_needs_a_selector_is_usage() {
    let dir = tempfile::tempdir().unwrap();
    let sd = dir.path().join("no-server");
    for args in [
        &["diagnostics", "purge"][..],
        &["diagnostics", "purge", "--all", "--before", "2026-11-01"],
        &[
            "diagnostics",
            "purge",
            "--all",
            "--project",
            "01hzzzzzzzzzzzzzzzzzzzzzzz",
        ],
        &["diagnostics", "purge", "--before", "yesterday"],
    ] {
        let r = fin(&sd, dir.path(), args);
        assert_error(&r, 2, "USAGE");
    }
}

#[test]
fn diagnostics_project_never_then_default() {
    let Some((server, dir, id)) = project("diagnostics_project_never_then_default", vec![]) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    marker_edit(&dir, sd, "a");
    wait(sd, root, &["--project", &id], |e| {
        !commands(e, "marker.add").is_empty()
    });
    let never = ok(fin(sd, &dir, &["diagnostics", "project", "never"]));
    assert_eq!(never, json!({ "project": id, "collect": "never" }));
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("finisher.project.json")).unwrap())
            .unwrap();
    assert_eq!(file["collect"], "never");
    assert!(
        lines(sd, root, &["--project", &id]).is_empty(),
        "its entries are purged"
    );
    marker_edit(&dir, sd, "b");
    ok(fin(sd, &dir, &["status"]));
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        lines(sd, root, &["--project", &id]).is_empty(),
        "and no new ones are written"
    );
    assert_eq!(show(sd, root)["projects"][&id], "never");
    // Clearing the flag binds every collaborator: a person's.
    refused_to_fin(sd, &dir, &["diagnostics", "project", "default"]);
    assert_eq!(show(sd, root)["projects"][&id], "never");
    let back = Console::open(&server).confirmed_ok(
        "diagnostics.project",
        json!({ "projectId": id, "collect": "default" }),
    );
    assert_eq!(back, json!({ "project": id, "collect": "default" }));
    marker_edit(&dir, sd, "c");
    wait(sd, root, &["--project", &id], |e| {
        !commands(e, "marker.add").is_empty()
    });
    assert_eq!(show(sd, root)["projects"][&id], "default");
    // By id, from anywhere.
    let by_id = ok(fin(
        sd,
        root,
        &["--project", &id, "diagnostics", "project", "never"],
    ));
    assert_eq!(by_id["collect"], "never");
}

#[test]
fn diagnostics_consent_diagnostics_on_off_unset() {
    let Some((server, dir, _)) = project("diagnostics_consent_diagnostics", vec![]) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    for value in ["off", "on", "unset"] {
        refused_to_fin(sd, root, &["diagnostics", "consent", "diagnostics", value]);
    }
    assert!(!sd.join("settings.json").exists());
    let mut console = Console::open(&server);
    let mut consent = |value: &str| {
        console.confirmed_ok(
            "diagnostics.consent",
            json!({ "purpose": "diagnostics", "value": value }),
        )
    };
    let off = consent("off");
    assert_eq!(off["diagnostics"], "off", "{off}");
    assert_eq!(show(sd, root)["state"], "off");
    let counted = show(sd, root)["kinds"]["cli.command"].as_u64().unwrap_or(0);
    // finisherd.json says off now: `fin` sends nothing, and nothing is stored.
    marker_edit(&dir, sd, "while off");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        show(sd, root)["kinds"]["cli.command"].as_u64().unwrap_or(0),
        counted
    );
    let on = consent("on");
    assert_eq!(on["diagnostics"], "on");
    marker_edit(&dir, sd, "on again");
    wait(sd, root, &["--kind", "cli.command"], |e| {
        !commands(e, "marker.add").is_empty()
    });
    let unset = consent("unset");
    assert_eq!(
        (&unset["diagnostics"], &unset["basis"]),
        (&json!("on"), &json!("default")),
        "{unset}"
    );
    let changed = lines(sd, root, &["--kind", "consent.changed"]);
    assert!(changed.len() >= 2, "{changed:?}");
    // An environment override wins and says so.
    let settings: Value = serde_json::from_str(
        &std::fs::read_to_string(sd.join("settings.json")).unwrap_or("{}".into()),
    )
    .unwrap();
    assert!(
        settings["privacy"].get("diagnostics").is_none(),
        "unset removes the key: {settings}"
    );
}

#[test]
fn diagnostics_consent_training_off_unset() {
    let Some(server) = Server::start("diagnostics_consent_training") else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    assert_eq!(show(sd, root)["consent"]["training"], "undecided");
    for value in ["off", "unset"] {
        refused_to_fin(sd, root, &["diagnostics", "consent", "training", value]);
    }
    let mut console = Console::open(&server);
    let off = console.confirmed_ok(
        "diagnostics.consent",
        json!({ "purpose": "training", "value": "off" }),
    );
    assert_eq!(off["training"], "off", "{off}");
    assert_eq!(show(sd, root)["consent"]["training"], "off");
    let unset = console.confirmed_ok(
        "diagnostics.consent",
        json!({ "purpose": "training", "value": "unset" }),
    );
    assert_eq!(unset["training"], "undecided", "{unset}");
    assert_eq!(show(sd, root)["consent"]["training"], "undecided");
    // Training on is the app's alone: the console is refused as well (5A Decision 21).
    let on = console.confirmed(
        "diagnostics.consent",
        json!({ "purpose": "training", "value": "on" }),
    );
    assert_eq!(error_code(&on).as_deref(), Some("CONSENT_UI_ONLY"), "{on}");
    assert_eq!(show(sd, root)["consent"]["training"], "undecided");
}

#[test]
fn consent_training_on_is_usage_naming_ways_in() {
    let dir = tempfile::tempdir().unwrap();
    let sd = dir.path().join("no-server");
    let r = fin(
        &sd,
        dir.path(),
        &["diagnostics", "consent", "training", "on"],
    );
    assert_error(&r, 2, "USAGE");
    let text = r.json["error"].to_string();
    for way in [
        "Finisher app",
        "settings.json",
        "privacy.training",
        "FIN_TRAINING=on",
    ] {
        assert!(text.contains(way), "{way} missing: {text}");
    }
    assert!(
        r.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("person")
    );
    // And with a server: the answer is the same, and nothing changed.
    let Some(server) = Server::start("consent_training_on") else {
        return;
    };
    let r = fin(
        &server.sd,
        &server.root,
        &["diagnostics", "consent", "training", "on"],
    );
    assert_error(&r, 2, "USAGE");
    assert_eq!(
        show(&server.sd, &server.root)["consent"]["training"],
        "undecided"
    );
}

#[test]
fn diagnostics_offline_is_server_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let sd = dir.path().join("no-server");
    for args in [
        &["diagnostics", "show"][..],
        &["diagnostics", "export"],
        &["diagnostics", "purge", "--all"],
        &["diagnostics", "consent", "diagnostics", "off"],
    ] {
        assert_error(&fin(&sd, dir.path(), args), 5, "SERVER_UNAVAILABLE");
    }
    // The offline snapshot hook has no server either.
    let snapshot = json!({ "rev": 1, "project": { "id": "p1", "name": "d", "schemaVersion": 2, "rate": "30", "timelines": [], "assets": {} } });
    let r = support::offline(&snapshot, &["diagnostics", "show"]);
    assert_error(&r, 5, "SERVER_UNAVAILABLE");
}

#[test]
fn journal_unavailable_exit_5() {
    let Some(mut server) = Server::start("journal_unavailable_exit_5") else {
        return;
    };
    let (sd, root) = (server.sd.clone(), server.root.clone());
    wait(&sd, &root, &[], |e| !e.is_empty());
    server.stop();
    // `events` is a file where the journal wants a directory: it is suspended, and says why.
    std::fs::remove_dir_all(sd.join("events")).unwrap();
    std::fs::write(sd.join("events"), b"not a directory").unwrap();
    server.restart();
    let r = fin(&sd, &root, &["diagnostics", "show"]);
    assert_error(&r, 5, "JOURNAL_UNAVAILABLE");
    // `settings.json` that does not parse is never overwritten by a consent change.
    std::fs::remove_file(sd.join("events")).unwrap();
    server.stop();
    std::fs::write(sd.join("settings.json"), b"{ not json").unwrap();
    server.restart();
    let exit = Console::open(&server).confirmed(
        "diagnostics.consent",
        json!({ "purpose": "diagnostics", "value": "off" }),
    );
    assert_eq!(
        error_code(&exit).as_deref(),
        Some("SETTINGS_INVALID"),
        "{exit}"
    );
    assert_eq!(
        std::fs::read(sd.join("settings.json")).unwrap(),
        b"{ not json"
    );
}

#[test]
fn dry_run_is_refused_by_the_mutating_diagnostics_commands() {
    let Some((server, dir, id)) = project("diagnostics_dry_run", vec![]) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    marker_edit(&dir, sd, "a");
    wait(sd, root, &["--project", &id], |e| {
        !commands(e, "marker.add").is_empty()
    });
    let before = lines(sd, root, &[]).len();
    for args in [
        &["diagnostics", "purge", "--all", "--dry-run"][..],
        &[
            "diagnostics",
            "purge",
            "--before",
            "2999-01-01",
            "--dry-run",
        ],
        &["diagnostics", "project", "never", "--dry-run"],
        &["diagnostics", "consent", "diagnostics", "off", "--dry-run"],
        &["diagnostics", "consent", "training", "off", "--dry-run"],
    ] {
        let r = fin(sd, &dir, args);
        assert_error(&r, 2, "USAGE");
        assert_eq!(r.json["error"]["details"]["flag"], "dry-run", "{args:?}");
    }
    // Nothing happened: the journal is whole, the project is not `never`, consent is as it was.
    assert!(lines(sd, root, &[]).len() >= before);
    assert!(!sd.join("settings.json").exists());
    let file = std::fs::read_to_string(dir.join("finisher.project.json")).unwrap();
    assert!(!file.contains("collect"), "{file}");
    let r = show(sd, root);
    assert_eq!(
        (&r["state"], &r["projects"][&id]),
        (&json!("on"), &json!("default"))
    );
    // Read-only ones still take it.
    assert_eq!(fin(sd, root, &["diagnostics", "show", "--dry-run"]).code, 0);
}

#[test]
fn fire_and_forget_runs_leave_no_unhandled_error_in_the_server_log() {
    let Some(server) = Server::start("no_unhandled_errors") else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    for _ in 0..30 {
        assert_eq!(fin(sd, root, &["diagnostics", "show"]).code, 0);
    }
    // Runs that connect only to say what they were (help, a usage error).
    for args in [
        &["help", "clip"][..],
        &["clip", "insert"],
        &["schema", "param"],
    ] {
        let _ = fin(sd, root, args);
    }
    wait(sd, root, &["--kind", "cli.command"], |e| {
        commands(e, "diagnostics.show").len() >= 30
    });
    std::thread::sleep(Duration::from_millis(300));
    let log = server.log();
    assert!(
        !log.contains("Unhandled error"),
        "{}",
        &log[..log.len().min(2000)]
    );
    assert!(
        !log.contains("level=ERROR"),
        "{}",
        &log[..log.len().min(2000)]
    );
}

#[test]
fn help_schema_and_refusals_are_recorded_with_online_false() {
    let Some(server) = Server::start("help_recorded") else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    // Each record is best effort within `fin`'s 50 ms budget (connect, handshake, write), which a
    // loaded machine can miss (Windows, full suite): a command is run again until it lands.
    let recorded = |name: &str, run: &dyn Fn()| {
        for _ in 0..10 {
            run();
            let end = Instant::now() + Duration::from_secs(3);
            while Instant::now() < end {
                if !commands(&lines(sd, root, &["--kind", "cli.command"]), name).is_empty() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        panic!("{name} was never recorded");
    };
    recorded("help", &|| {
        assert_eq!(fin(sd, root, &["help", "clip"]).code, 0)
    });
    recorded("other", &|| {
        assert_error(&fin(sd, root, &["clip", "insert"]), 2, "USAGE")
    });
    // A refusal the client makes itself (the server was never asked), with a server running.
    recorded("diagnostics.consent", &|| {
        let r = fin(sd, root, &["diagnostics", "consent", "training", "on"]);
        assert_error(&r, 2, "USAGE");
    });
    let found = wait(sd, root, &["--kind", "cli.command"], |e| {
        !commands(e, "help").is_empty()
            && !commands(e, "other").is_empty()
            && !commands(e, "diagnostics.consent").is_empty()
    });
    for name in ["help", "other", "diagnostics.consent"] {
        let p = &commands(&found, name)[0]["payload"];
        assert_eq!(p["online"], false, "{p}");
    }
    assert_eq!(commands(&found, "other")[0]["payload"]["code"], "USAGE");
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
