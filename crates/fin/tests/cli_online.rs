//! `fin` against a real `finisherd` (spawned per test in a temp `FIN_SERVER_DATA`). Tests that
//! need the server print `SKIPPED <test>: <reason>` and pass when Node or fin-node is missing.

mod support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{Server, assert_error, fin, fin_cmd, fin_env, finish, media, ok, real_tempdir};

/// A server, a project `<root>/film` (rate 30) and a 10 s `clip.mp4` inside it.
fn project(test: &str) -> Option<(Server, PathBuf)> {
    let server = Server::start(test)?;
    let dir = server.root.join("film");
    let created = ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    assert_eq!(created["resolved"]["dir"], dir.display().to_string());
    assert_eq!(created["project"]["rev"], 0);
    media(&dir.join("clip.mp4"), 10);
    Some((server, dir))
}

/// `base` joined with each `/`-separated part: the OS-native form fin echoes (Windows
/// absolutising turns `/` into `\`).
fn native(base: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(base.to_path_buf(), |p, part| p.join(part))
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn full_editing_flow() {
    let Some((server, dir)) = project("full_editing_flow") else {
        return;
    };
    let run = |args: &[&str]| ok(fin(&server.sd, &dir, args));

    let added = run(&["asset", "add", "clip.mp4"]);
    assert_eq!(added["rev"], 1);
    assert_eq!(added["bindings"], json!({ "$asset": "a1" }));
    assert_eq!(
        added["resolved"],
        json!({ "path": s(&dir.join("clip.mp4")), "duration": "10" })
    );

    let first = run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s",
    ]);
    assert_eq!(
        (first["rev"].clone(), first["bindings"].clone()),
        (json!(2), json!({ "$clip": "c1" }))
    );
    let second = run(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--in", "4s", "--dur",
        "3s",
    ]);
    assert_eq!(second["bindings"], json!({ "$clip": "c2" }));
    assert_eq!(second["resolved"]["at"], "4");
    assert_eq!(second["resolved"]["out"], "7");

    let trimmed = run(&["clip", "trim", "c1", "--end", "-1s"]);
    assert_eq!(trimmed["rev"], 4);
    assert_eq!(
        trimmed["diff"]["shifts"],
        json!([{ "track": "t1", "from": "4", "by": "-1", "op": 0 }])
    );

    let deleted = run(&["clip", "delete", "c1"]);
    assert_eq!(deleted["rev"], 5);
    assert_eq!(
        deleted["diff"]["changes"],
        json!([{ "kind": "item", "id": "c1", "change": "removed", "op": 0 }])
    );

    let view = run(&["view"]);
    assert_eq!(
        view,
        json!({
            "rev": 5,
            "rate": "30",
            "tracks": [
                { "id": "t1", "kind": "video", "clips": [{ "id": "c2", "start": "0", "end": "3", "asset": "a1", "sourceIn": "4" }] },
                { "id": "t2", "kind": "audio", "clips": [] },
            ],
            "resolved": {},
        })
    );
    // From outside the project folder, --project finds it; status agrees with view.
    let status = ok(fin(
        &server.sd,
        &server.root,
        &["status", "--project", "film"],
    ));
    assert_eq!(status["rev"], 5);
    assert_eq!(status["assets"], json!(["a1"]));
    // A dry run reads the head but commits nothing.
    let dry = run(&["clip", "delete", "c2", "--dry-run"]);
    assert_eq!(dry["dryRun"], true);
    assert_eq!(run(&["status"])["rev"], 5);
}

#[test]
fn asset_paths_resolve_to_absolute_and_are_echoed() {
    let Some((server, dir)) = project("asset_paths_resolve_to_absolute_and_are_echoed") else {
        return;
    };
    let (_home_tmp, home) = real_tempdir();
    let files = [
        ("media/rel.mp4", native(&dir, "media/rel.mp4")),
        ("~/home clip.mp4", home.join("home clip.mp4")),
        (
            "my clip with spaces.mp4",
            dir.join("my clip with spaces.mp4"),
        ),
        ("відео/кліп один.mp4", native(&dir, "відео/кліп один.mp4")),
    ];
    for (n, (input, expected)) in files.iter().enumerate() {
        media(expected, 2);
        let run = fin_env(
            &server.sd,
            &dir,
            &["asset", "add", input],
            &[("HOME", home.as_os_str())],
        );
        let doc = ok(run);
        assert_eq!(doc["resolved"]["path"], s(expected), "{input}");
        assert_eq!(doc["bindings"]["$asset"], format!("a{}", n + 1), "{input}");
    }
    // An absolute path passes through unchanged, and `~` in --project expands too.
    let abs = native(&dir, "відео/кліп один.mp4");
    assert_eq!(
        ok(fin(&server.sd, &dir, &["asset", "add", &s(&abs)]))["resolved"]["path"],
        s(&abs)
    );
    let with_home = |args: &[&str]| {
        ok(fin_env(
            &server.sd,
            &server.root,
            args,
            &[("HOME", home.as_os_str())],
        ))
    };
    let created = with_home(&["project", "create", "~/другий проєкт", "--name", "Second"]);
    assert_eq!(created["resolved"]["dir"], s(&home.join("другий проєкт")));
    assert_eq!(
        with_home(&["status", "--project", "~/другий проєкт"])["project"]["name"],
        "Second"
    );
}

#[test]
fn missing_media_is_asset_not_found_with_the_resolved_path() {
    let Some((server, dir)) = project("missing_media_is_asset_not_found_with_the_resolved_path")
    else {
        return;
    };
    let run = fin(&server.sd, &dir, &["asset", "add", "nope/кліп.mp4"]);
    assert_error(&run, 3, "ASSET_NOT_FOUND");
    assert_eq!(
        run.json["error"]["details"]["path"],
        s(&native(&dir, "nope/кліп.mp4"))
    );
    assert_eq!(ok(fin(&server.sd, &dir, &["status"]))["rev"], 0);
}

#[test]
fn project_create_under_a_file_is_project_dir_unwritable() {
    let Some(server) = Server::start("project_create_under_a_file_is_project_dir_unwritable")
    else {
        return;
    };
    std::fs::write(server.root.join("notes.txt"), b"x").unwrap();
    let run = fin(
        &server.sd,
        &server.root,
        &["project", "create", "notes.txt/film", "--name", "Film"],
    );
    assert_error(&run, 3, "PROJECT_DIR_UNWRITABLE");
    let details = &run.json["error"]["details"];
    assert_eq!(details["path"], s(&native(&server.root, "notes.txt/film")));
    assert!(details["reason"].as_str().is_some_and(|r| !r.is_empty()));
}

#[test]
fn project_create_retry_with_idem_key_returns_the_project() {
    let Some(server) = Server::start("project_create_retry_with_idem_key_returns_the_project")
    else {
        return;
    };
    let args = [
        "project",
        "create",
        "film",
        "--name",
        "Film",
        "--idem-key",
        "create-1",
    ];
    let first = ok(fin(&server.sd, &server.root, &args));
    let again = ok(fin(&server.sd, &server.root, &args));
    assert_eq!(
        again["project"]["projectId"], first["project"]["projectId"],
        "{again}"
    );
    // Without the key, or with the key and another name: not a retry.
    assert_error(
        &fin(&server.sd, &server.root, &args[..5]),
        3,
        "PROJECT_EXISTS",
    );
    let other = [
        "project",
        "create",
        "film",
        "--name",
        "Other",
        "--idem-key",
        "create-1",
    ];
    assert_error(&fin(&server.sd, &server.root, &other), 3, "IDEM_KEY_REUSED");
    let empty = ["project", "create", "x", "--name", " ", "--idem-key", "k"];
    assert_error(&fin(&server.sd, &server.root, &empty), 3, "INVALID_NAME");
}

#[test]
fn server_down_is_exit_5() {
    let Some((mut server, dir)) = project("server_down_is_exit_5") else {
        return;
    };
    server.stop();
    for args in [
        &["status"][..],
        &["clip", "delete", "c1"],
        &["asset", "add", "clip.mp4"],
    ] {
        let run = fin(&server.sd, &dir, args);
        assert_error(&run, 5, "SERVER_UNAVAILABLE");
        assert_eq!(run.json["error"]["fix"], "start finisherd");
    }
}

#[test]
fn no_server_and_no_project_are_typed_errors() {
    let (_tmp, root) = real_tempdir();
    let sd = root.join("sd");
    // No finisherd.json at all.
    let run = fin(&sd, &root, &["status", "--project", "."]);
    assert_error(&run, 5, "SERVER_UNAVAILABLE");
    assert_eq!(run.json["error"]["fix"], "start finisherd");
    // finisherd.json names a socket nobody listens on.
    std::fs::create_dir_all(&sd).unwrap();
    let info = json!({ "pid": 1, "wsPort": 1, "socket": s(&sd.join("fin.sock")), "token": "t", "protocolVersion": 1 });
    std::fs::write(sd.join("finisherd.json"), info.to_string()).unwrap();
    assert_error(
        &fin(&sd, &root, &["view", "--project", "."]),
        5,
        "SERVER_UNAVAILABLE",
    );
    // FIN_SERVER overrides server data.
    let gone = root.join("gone.sock");
    let run = fin_env(
        &sd,
        &root,
        &["status", "--project", "."],
        &[("FIN_SERVER", gone.as_os_str())],
    );
    assert_error(&run, 5, "SERVER_UNAVAILABLE");
    assert_eq!(run.json["error"]["details"]["endpoint"], s(&gone));
    // No project anywhere up the tree and no --project: nothing to connect for.
    let run = fin(&sd, &root, &["clip", "delete", "c1"]);
    assert_error(&run, 3, "PROJECT_NOT_FOUND");
    let fix = run.json["error"]["fix"].as_str().unwrap();
    assert!(
        fix.contains("fin project create <dir>") && fix.contains("--project"),
        "{fix}"
    );
}

#[test]
fn same_idem_key_twice_gives_identical_output_and_one_rev() {
    let Some((server, dir)) = project("same_idem_key_twice_gives_identical_output_and_one_rev")
    else {
        return;
    };
    ok(fin(&server.sd, &dir, &["asset", "add", "clip.mp4"]));
    let args = [
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "end",
        "--asset",
        "a1",
        "--dur",
        "2s",
        "--idem-key",
        "insert-1",
    ];
    let first = fin(&server.sd, &dir, &args);
    let again = fin(&server.sd, &dir, &args);
    assert_eq!(first.code, 0, "{}", first.stdout);
    assert_eq!(first.stdout, again.stdout);
    assert_eq!(first.json["txId"], "insert-1");
    let view = ok(fin(&server.sd, &dir, &["view"]));
    assert_eq!(view["rev"], 2);
    assert_eq!(view["tracks"][0]["clips"].as_array().unwrap().len(), 1);
}

#[test]
fn same_idem_key_delete_twice_replays_the_commit() {
    let Some((server, dir)) = project("same_idem_key_delete_twice_replays_the_commit") else {
        return;
    };
    let run = |args: &[&str]| fin(&server.sd, &dir, args);
    ok(run(&["asset", "add", "clip.mp4"]));
    ok(run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "2s",
    ]));
    let args = ["clip", "delete", "c1", "--idem-key", "delete-1"];
    let first = run(&args);
    let again = run(&args);
    assert_eq!(first.code, 0, "{}", first.stdout);
    assert_eq!(again.code, 0, "{}", again.stdout);
    assert_eq!(first.stdout, again.stdout);
    assert_eq!(ok(run(&["status"]))["rev"], 3);
    // A new key for a delete that really is invalid: the local error, nothing committed.
    let fresh = run(&["clip", "delete", "c1", "--idem-key", "delete-2"]);
    assert_error(&fresh, 3, "NOT_FOUND");
    assert_eq!(fresh.json["error"]["details"]["opIndex"], 0);
    // Without a key, local validation still answers.
    assert_error(&run(&["clip", "delete", "c1"]), 3, "NOT_FOUND");
    assert_eq!(ok(run(&["status"]))["rev"], 3);
}

#[test]
fn idem_key_reused_for_a_different_command_is_rejected() {
    let Some((server, dir)) = project("idem_key_reused_for_a_different_command_is_rejected") else {
        return;
    };
    let run = |args: &[&str]| fin(&server.sd, &dir, args);
    ok(run(&["asset", "add", "clip.mp4"]));
    for _ in 0..2 {
        ok(run(&[
            "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "2s",
        ]));
    }
    let delete = ["clip", "delete", "c2", "--idem-key", "k1"];
    let first = run(&delete);
    assert_eq!(first.code, 0, "{}", first.stdout);
    assert_eq!(first.json["rev"], 4);
    let insert = [
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "end",
        "--asset",
        "a1",
        "--dur",
        "1s",
        "--idem-key",
        "k1",
    ];
    let reused = run(&insert);
    assert_error(&reused, 3, "IDEM_KEY_REUSED");
    let error = &reused.json["error"];
    assert_eq!(error["details"]["txId"], "k1");
    assert_eq!(error["details"]["rev"], 4);
    assert_eq!(error["details"]["committedOps"][0]["op"], "item.delete");
    assert_eq!(error["details"]["committedOps"][0]["item"], "c2");
    assert_eq!(error["fix"], "use a new --idem-key for a different command");
    // The same kind of op on another clip is a different command too.
    ok(run(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "2s",
    ]));
    assert_error(
        &run(&["clip", "delete", "c3", "--idem-key", "k1"]),
        3,
        "IDEM_KEY_REUSED",
    );
    // A genuine retry still replays the original commit, byte for byte.
    let again = run(&delete);
    assert_eq!(again.code, 0, "{}", again.stdout);
    assert_eq!(first.stdout, again.stdout);
    assert_eq!(ok(run(&["status"]))["rev"], 5);
}

/// Same op kind, other args: the ops alone cannot tell (a retry's positions move), the stored
/// command-input hash can. A retry of `--at end` after another commit is still the same command.
#[test]
fn idem_key_same_key_different_args_rejected() {
    let Some((server, dir)) = project("idem_key_same_key_different_args_rejected") else {
        return;
    };
    let run = |args: &[&str]| fin(&server.sd, &dir, args);
    ok(run(&["asset", "add", "clip.mp4"]));
    let at_end = [
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "end",
        "--asset",
        "a1",
        "--dur",
        "2s",
        "--idem-key",
        "k1",
    ];
    let first = run(&at_end);
    assert_eq!(first.code, 0, "{}", first.stdout);
    ok(run(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s",
    ]));
    // The retry plans `end` at 3s now, yet it is the command that committed.
    let again = run(&at_end);
    assert_eq!(again.code, 0, "{}", again.stdout);
    assert_eq!(again.json["rev"], first.json["rev"]);
    let mut at_zero = at_end;
    at_zero[5] = "0";
    assert_error(&run(&at_zero), 3, "IDEM_KEY_REUSED");
    let mut longer = at_end;
    longer[9] = "3s";
    assert_error(&run(&longer), 3, "IDEM_KEY_REUSED");
    assert_eq!(ok(run(&["status"]))["rev"], 3);
}

#[test]
fn idem_key_with_a_locally_invalid_step_keeps_the_servers_conflict() {
    let Some((server, dir)) =
        project("idem_key_with_a_locally_invalid_step_keeps_the_servers_conflict")
    else {
        return;
    };
    ok(fin(&server.sd, &dir, &["asset", "add", "clip.mp4"]));
    ok(fin(
        &server.sd,
        &dir,
        &[
            "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "2s",
        ],
    ));
    // Locally invalid (c1 is only 2 s long), so it is deferred and sent because of the key...
    let gate = server.root.join("gate");
    let args = [
        "clip",
        "trim",
        "c1",
        "--end",
        "-10s",
        "--idem-key",
        "trim-1",
    ];
    let writer = fin_cmd(&server.sd, &dir, &args)
        .env("FIN_TEST_COMMIT_GATE", &gate)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let ready = || {
        std::fs::read_dir(&server.root)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("gate.ready."))
    };
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "writer never reached the commit gate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // ...but c1 is deleted before it commits, so the server's answer is a rebase conflict.
    ok(fin(&server.sd, &dir, &["clip", "delete", "c1"]));
    std::fs::write(&gate, b"").unwrap();
    let run = finish(&args, writer.wait_with_output().unwrap());
    assert_error(&run, 4, "CONFLICT");
    assert_eq!(run.json["error"]["details"]["txId"], "trim-1");
    assert_eq!(ok(fin(&server.sd, &dir, &["status"]))["rev"], 3);
}

#[test]
fn concurrent_inserts_on_one_snapshot_both_commit_rebased() {
    let Some((server, dir)) = project("concurrent_inserts_on_one_snapshot_both_commit_rebased")
    else {
        return;
    };
    ok(fin(&server.sd, &dir, &["asset", "add", "clip.mp4"]));
    let gate = server.root.join("gate");
    let args = [
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "1s",
    ];
    let children: Vec<_> = (0..2)
        .map(|_| {
            fin_cmd(&server.sd, &dir, &args)
                .env("FIN_TEST_COMMIT_GATE", &gate)
                .spawn()
                .unwrap()
        })
        .collect();
    // Both have read the snapshot (rev 1) and wait before committing; then let them race.
    let deadline = Instant::now() + Duration::from_secs(30);
    let ready = || {
        std::fs::read_dir(&server.root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("gate.ready."))
            .count()
    };
    while ready() < 2 {
        assert!(
            Instant::now() < deadline,
            "writers never reached the commit gate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(&gate, b"").unwrap();
    let mut docs: Vec<Value> = children
        .into_iter()
        .map(|c| ok(finish(&args, c.wait_with_output().unwrap())))
        .collect();
    docs.sort_by_key(|d| d["rev"].as_u64());
    assert_eq!(
        (docs[0]["rev"].clone(), docs[1]["rev"].clone()),
        (json!(2), json!(3))
    );
    assert_eq!(docs[0]["resolved"]["at"], "0");
    let view = ok(fin(&server.sd, &dir, &["view", "--track", "t1"]));
    let clips = view["tracks"][0]["clips"].as_array().unwrap().clone();
    assert_eq!(clips.len(), 2, "{view}");
    assert_eq!(
        (clips[0]["start"].clone(), clips[0]["end"].clone()),
        (json!("0"), json!("1"))
    );
    assert_eq!(
        (clips[1]["start"].clone(), clips[1]["end"].clone()),
        (json!("1"), json!("2"))
    );
    // The later writer echoes where its clip actually landed after the rebase.
    let later = docs[1]["bindings"]["$clip"].as_str().unwrap();
    let landed = clips.iter().find(|c| c["id"] == later).unwrap();
    assert_eq!(docs[1]["resolved"]["at"], landed["start"]);
}

#[test]
fn project_status_prints_json() {
    let Some((server, dir)) = project("project_status_prints_json") else {
        return;
    };
    let run = |args: &[&str]| fin(&server.sd, &dir, args);
    ok(run(&["asset", "add", "clip.mp4"]));
    let status = ok(run(&["project", "status"]));
    let st = &status["status"];
    assert_eq!(st["lease"]["state"], "held", "{status}");
    assert_eq!(st["readOnly"], Value::Null);
    assert_eq!(st["busy"], Value::Null);
    assert_eq!(st["storage"], "local");
    assert_eq!(st["branches"][0]["head"], 1);
    assert!(st["incarnation"].as_str().is_some_and(|i| !i.is_empty()));
    // From outside the folder too.
    let outside = ok(fin(
        &server.sd,
        &server.root,
        &["project", "status", "--project", "film"],
    ));
    assert_eq!(outside["status"]["projectId"], st["projectId"]);

    let held = ok(run(&["project", "takeover"]));
    assert_eq!(held["status"]["lease"]["state"], "held");
    let verify = ok(run(&["project", "verify"]));
    assert_eq!(verify["report"]["ok"], true, "{verify}");
    assert!(ok(run(&["project", "compact"]))["report"].is_object());
    // 4B: the report of `project.recover`.
    let recovered = ok(run(&["project", "recover"]));
    assert_eq!(recovered["groups"], json!([]), "{recovered}");
    assert_eq!(recovered["manual"], Value::Null);
    let ack = ok(run(&["project", "acknowledge", "--rewind", "1"]));
    assert_eq!(ack, json!({ "acknowledged": false }));
    let ack = ok(run(&["project", "acknowledge", "--salvage", "nope"]));
    assert_eq!(ack, json!({ "acknowledged": false }));
    assert_error(&run(&["project", "acknowledge"]), 2, "USAGE");
    // Online, the rebuild goes through the server (it holds the database).
    let rebuilt = ok(run(&["project", "rebuild"]));
    assert_eq!(rebuilt["offline"], false, "{rebuilt}");
    assert_eq!(ok(run(&["status"]))["rev"], 1);
}

#[test]
fn read_only_commit_exits_4() {
    let Some((server, dir)) = project("read_only_commit_exits_4") else {
        return;
    };
    ok(fin(&server.sd, &dir, &["asset", "add", "clip.mp4"]));
    // A second server (its own server data) on the same folder follows, read-only.
    let Some(follower) = Server::start("read_only_commit_exits_4 (follower)") else {
        return;
    };
    let run = |args: &[&str]| fin(&follower.sd, &dir, args);
    let status = ok(run(&["project", "status"]));
    assert_eq!(
        status["status"]["lease"]["state"], "heldElsewhere",
        "{status}"
    );
    assert_eq!(status["status"]["readOnly"]["reason"], "LEASE_HELD");
    // It imports the writer's edit once exported (2 s), then refuses edits of its own.
    let deadline = Instant::now() + Duration::from_secs(20);
    while ok(run(&["status"]))["assets"] != json!(["a1"]) {
        assert!(Instant::now() < deadline, "the follower never imported a1");
        std::thread::sleep(Duration::from_millis(200));
    }
    let refused = run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "1s",
    ]);
    assert_error(&refused, 4, "PROJECT_READ_ONLY");
    assert_eq!(refused.json["error"]["details"]["reason"], "LEASE_HELD");
    // The holder is alive: no takeover, confirmed or not.
    assert_error(
        &run(&["project", "takeover", "--confirm"]),
        4,
        "LEASE_TAKEOVER_REFUSED",
    );
    // Reads work on the follower.
    assert_eq!(ok(run(&["status"]))["assets"], json!(["a1"]));
}

#[test]
fn a_commit_against_a_rebuilt_database_exits_4() {
    let Some((server, dir)) = project("a_commit_against_a_rebuilt_database_exits_4") else {
        return;
    };
    ok(fin(&server.sd, &dir, &["asset", "add", "clip.mp4"]));
    // The writer reads the snapshot (and its generation), then waits at the gate...
    let gate = server.root.join("gate");
    let args = [
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "1s",
    ];
    let writer = fin_cmd(&server.sd, &dir, &args)
        .env("FIN_TEST_COMMIT_GATE", &gate)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !std::fs::read_dir(&server.root)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("gate.ready."))
    {
        assert!(
            Instant::now() < deadline,
            "writer never reached the commit gate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // ...while the database is rebuilt: what it read may not be what is there now.
    ok(fin(&server.sd, &dir, &["project", "rebuild"]));
    std::fs::write(&gate, b"").unwrap();
    let run = finish(&args, writer.wait_with_output().unwrap());
    assert_error(&run, 4, "HISTORY_REWOUND");
    assert_eq!(ok(fin(&server.sd, &dir, &["status"]))["rev"], 1);
    // Read again, it commits.
    ok(fin(&server.sd, &dir, &args));
}

#[test]
fn offline_rebuild_exports_unexported_commits_first() {
    let Some((mut server, dir)) = project("offline_rebuild_exports_unexported_commits_first")
    else {
        return;
    };
    // The folder takes no segments while the server runs, so the commit stays in the
    // database only (the server is killed: no flush either).
    let segments = dir.join(".finisher").join("segments");
    std::fs::create_dir_all(&segments).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&segments, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    let args = ["asset", "add", "clip.mp4", "--idem-key", "add-1"];
    let first = fin(&server.sd, &dir, &args);
    assert_eq!(first.json["rev"], 1, "{}", first.stdout);
    server.stop();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&segments, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // No server: fin opens the project in-process, exports what the database holds, then
    // rebuilds. Nothing is set aside: the commit is in the folder and the new database.
    let rebuilt = ok(fin(&server.sd, &dir, &["project", "rebuild"]));
    assert_eq!(rebuilt["offline"], true, "{rebuilt}");
    assert_eq!(rebuilt["report"]["salvaged"], 0, "{rebuilt}");
    assert_eq!(rebuilt["report"]["records"], 1, "{rebuilt}");

    server.restart();
    assert_eq!(ok(fin(&server.sd, &dir, &["status"]))["rev"], 1);
    // The retry finds its commit: the same answer, nothing committed again.
    let retry = fin(&server.sd, &dir, &args);
    assert_eq!(retry.stdout, first.stdout);
    assert_eq!(ok(fin(&server.sd, &dir, &["status"]))["rev"], 1);
    // While a server has the project open, an in-process rebuild is never attempted: it goes
    // through the server.
    assert_eq!(
        ok(fin(&server.sd, &dir, &["project", "rebuild"]))["offline"],
        false
    );
}

#[test]
fn a_failed_verify_exits_3() {
    let Some((server, dir)) = project("a_failed_verify_exits_3") else {
        return;
    };
    ok(fin(&server.sd, &dir, &["asset", "add", "clip.mp4"]));
    let main = dir.join(".finisher").join("segments").join("main");
    let deadline = Instant::now() + Duration::from_secs(20);
    let segment = loop {
        let found = std::fs::read_dir(&main).ok().and_then(|d| {
            d.flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "fseg"))
        });
        if let Some(p) = found {
            break p;
        }
        assert!(Instant::now() < deadline, "no segment was exported");
        std::thread::sleep(Duration::from_millis(100));
    };
    // One byte of a record changes: its checksum no longer matches.
    let mut bytes = std::fs::read(&segment).unwrap();
    let i = bytes.iter().position(|b| *b == b'"').unwrap() + 1;
    bytes[i] = if bytes[i] == b'x' { b'y' } else { b'x' };
    std::fs::write(&segment, bytes).unwrap();
    let run = fin(&server.sd, &dir, &["project", "verify"]);
    assert_error(&run, 3, "VERIFY_FAILED");
    let details = &run.json["error"]["details"];
    assert_eq!(details["report"]["ok"], false, "{}", run.stdout);
    assert!(
        details["failed"]
            .as_array()
            .unwrap()
            .contains(&json!("segment_checksums")),
        "{}",
        run.stdout
    );
    assert!(
        run.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains("fin project rebuild")
    );
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
