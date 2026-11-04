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
    let created = ok(fin(&server.sd, &server.root, &["project", "create", "film", "--name", "Film"]));
    assert_eq!(created["resolved"]["dir"], dir.display().to_string());
    assert_eq!(created["project"]["rev"], 0);
    media(&dir.join("clip.mp4"), 10);
    Some((server, dir))
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn full_editing_flow() {
    let Some((server, dir)) = project("full_editing_flow") else { return };
    let run = |args: &[&str]| ok(fin(&server.sd, &dir, args));

    let added = run(&["asset", "add", "clip.mp4"]);
    assert_eq!(added["rev"], 1);
    assert_eq!(added["bindings"], json!({ "$asset": "a1" }));
    assert_eq!(added["resolved"], json!({ "path": s(&dir.join("clip.mp4")), "duration": "10" }));

    let first = run(&["clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s"]);
    assert_eq!((first["rev"].clone(), first["bindings"].clone()), (json!(2), json!({ "$clip": "c1" })));
    let second = run(&["clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--in", "4s", "--dur", "3s"]);
    assert_eq!(second["bindings"], json!({ "$clip": "c2" }));
    assert_eq!(second["resolved"]["at"], "4");
    assert_eq!(second["resolved"]["out"], "7");

    let trimmed = run(&["clip", "trim", "c1", "--end", "-1s"]);
    assert_eq!(trimmed["rev"], 4);
    assert_eq!(trimmed["diff"]["shifts"], json!([{ "track": "t1", "from": "4", "by": "-1" }]));

    let deleted = run(&["clip", "delete", "c1"]);
    assert_eq!(deleted["rev"], 5);
    assert_eq!(deleted["diff"]["changes"], json!([{ "kind": "clip", "id": "c1", "change": "removed" }]));

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
    let status = ok(fin(&server.sd, &server.root, &["status", "--project", "film"]));
    assert_eq!(status["rev"], 5);
    assert_eq!(status["assets"], json!(["a1"]));
    // A dry run reads the head but commits nothing.
    let dry = run(&["clip", "delete", "c2", "--dry-run"]);
    assert_eq!(dry["dryRun"], true);
    assert_eq!(run(&["status"])["rev"], 5);
}

#[test]
fn asset_paths_resolve_to_absolute_and_are_echoed() {
    let Some((server, dir)) = project("asset_paths_resolve_to_absolute_and_are_echoed") else { return };
    let (_home_tmp, home) = real_tempdir();
    let files = [
        ("media/rel.mp4", dir.join("media/rel.mp4")),
        ("~/home clip.mp4", home.join("home clip.mp4")),
        ("my clip with spaces.mp4", dir.join("my clip with spaces.mp4")),
        ("відео/кліп один.mp4", dir.join("відео/кліп один.mp4")),
    ];
    for (n, (input, expected)) in files.iter().enumerate() {
        media(expected, 2);
        let run = fin_env(&server.sd, &dir, &["asset", "add", input], &[("HOME", home.as_os_str())]);
        let doc = ok(run);
        assert_eq!(doc["resolved"]["path"], s(expected), "{input}");
        assert_eq!(doc["bindings"]["$asset"], format!("a{}", n + 1), "{input}");
    }
    // An absolute path passes through unchanged, and `~` in --project expands too.
    let abs = dir.join("відео/кліп один.mp4");
    assert_eq!(ok(fin(&server.sd, &dir, &["asset", "add", &s(&abs)]))["resolved"]["path"], s(&abs));
    let with_home = |args: &[&str]| ok(fin_env(&server.sd, &server.root, args, &[("HOME", home.as_os_str())]));
    let created = with_home(&["project", "create", "~/другий проєкт", "--name", "Second"]);
    assert_eq!(created["resolved"]["dir"], s(&home.join("другий проєкт")));
    assert_eq!(with_home(&["status", "--project", "~/другий проєкт"])["project"]["name"], "Second");
}

#[test]
fn missing_media_is_asset_not_found_with_the_resolved_path() {
    let Some((server, dir)) = project("missing_media_is_asset_not_found_with_the_resolved_path") else { return };
    let run = fin(&server.sd, &dir, &["asset", "add", "nope/кліп.mp4"]);
    assert_error(&run, 3, "ASSET_NOT_FOUND");
    assert_eq!(run.json["error"]["details"]["path"], s(&dir.join("nope/кліп.mp4")));
    assert_eq!(ok(fin(&server.sd, &dir, &["status"]))["rev"], 0);
}

#[test]
fn server_down_is_exit_5() {
    let Some((mut server, dir)) = project("server_down_is_exit_5") else { return };
    server.stop();
    for args in [&["status"][..], &["clip", "delete", "c1"], &["asset", "add", "clip.mp4"]] {
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
    assert_error(&fin(&sd, &root, &["view", "--project", "."]), 5, "SERVER_UNAVAILABLE");
    // FIN_SERVER overrides server data.
    let gone = root.join("gone.sock");
    let run = fin_env(&sd, &root, &["status", "--project", "."], &[("FIN_SERVER", gone.as_os_str())]);
    assert_error(&run, 5, "SERVER_UNAVAILABLE");
    assert_eq!(run.json["error"]["details"]["endpoint"], s(&gone));
    // No project anywhere up the tree and no --project: nothing to connect for.
    let run = fin(&sd, &root, &["clip", "delete", "c1"]);
    assert_error(&run, 3, "PROJECT_NOT_FOUND");
    let fix = run.json["error"]["fix"].as_str().unwrap();
    assert!(fix.contains("fin project create <dir>") && fix.contains("--project"), "{fix}");
}

#[test]
fn same_idem_key_twice_gives_identical_output_and_one_rev() {
    let Some((server, dir)) = project("same_idem_key_twice_gives_identical_output_and_one_rev") else { return };
    ok(fin(&server.sd, &dir, &["asset", "add", "clip.mp4"]));
    let args = ["clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "2s", "--idem-key", "insert-1"];
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
    let Some((server, dir)) = project("same_idem_key_delete_twice_replays_the_commit") else { return };
    let run = |args: &[&str]| fin(&server.sd, &dir, args);
    ok(run(&["asset", "add", "clip.mp4"]));
    ok(run(&["clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "2s"]));
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
fn concurrent_inserts_on_one_snapshot_both_commit_rebased() {
    let Some((server, dir)) = project("concurrent_inserts_on_one_snapshot_both_commit_rebased") else { return };
    ok(fin(&server.sd, &dir, &["asset", "add", "clip.mp4"]));
    let gate = server.root.join("gate");
    let args = ["clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "1s"];
    let children: Vec<_> = (0..2)
        .map(|_| fin_cmd(&server.sd, &dir, &args).env("FIN_TEST_COMMIT_GATE", &gate).spawn().unwrap())
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
        assert!(Instant::now() < deadline, "writers never reached the commit gate");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(&gate, b"").unwrap();
    let mut docs: Vec<Value> = children.into_iter().map(|c| ok(finish(&args, c.wait_with_output().unwrap()))).collect();
    docs.sort_by_key(|d| d["rev"].as_u64());
    assert_eq!((docs[0]["rev"].clone(), docs[1]["rev"].clone()), (json!(2), json!(3)));
    assert_eq!(docs[0]["resolved"]["at"], "0");
    let view = ok(fin(&server.sd, &dir, &["view", "--track", "t1"]));
    let clips = view["tracks"][0]["clips"].as_array().unwrap().clone();
    assert_eq!(clips.len(), 2, "{view}");
    assert_eq!((clips[0]["start"].clone(), clips[0]["end"].clone()), (json!("0"), json!("1")));
    assert_eq!((clips[1]["start"].clone(), clips[1]["end"].clone()), (json!("1"), json!("2")));
    // The later writer echoes where its clip actually landed after the rebase.
    let later = docs[1]["bindings"]["$clip"].as_str().unwrap();
    let landed = clips.iter().find(|c| c["id"] == later).unwrap();
    assert_eq!(docs[1]["resolved"]["at"], landed["start"]);
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
