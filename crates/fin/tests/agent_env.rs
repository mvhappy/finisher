//! `FIN_CALL` as the default idempotency key when `FIN_CALL_IDEM=1` marks a single-shot `fin`
//! (plan 7A Task 8a, Decision 14; review B1): offline against the `FIN_OFFLINE_SNAPSHOT` hook
//! (every write a dry run, which still names its txId) and against a real `finisherd` for the
//! dedupe of a retried call and for a shell that runs several `fin`s under one call id.

mod support;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use support::{Run, Server, assert_error, fin_cmd, fin_env, finish, media, ok};

const CALL: &str = "01JC0000000000000000000006";

/// Rate 30; asset a1 (60 s); t1 holds c1 [0, 5) and c2 [5, 10).
fn snapshot() -> Value {
    let clip = |id: &str, start: &str| json!({ "id": id, "kind": "clip", "asset": "a1", "start": start, "sourceIn": "0", "duration": "5" });
    json!({
        "rev": 7,
        "project": {
            "id": "p1", "name": "Demo", "schemaVersion": 2, "rate": "30",
            "timelines": [{
                "id": "tl1", "name": "Timeline 1", "rate": "30",
                "resolution": { "width": 1080, "height": 1920 },
                "tracks": [{ "id": "t1", "kind": "video", "name": "V1", "items": [clip("c1", "0"), clip("c2", "5")] }],
            }],
            "assets": { "a1": {
                "id": "a1", "kind": "media", "path": "/media/a.mp4", "duration": "60",
                "video": { "width": 1920, "height": 1080, "rate": "30" }, "fingerprint": "f",
            } },
        },
    })
}

/// `fin` against the offline snapshot as the built-in tool runs it: `FIN_CALL` set to `call`
/// (or removed) with `FIN_CALL_IDEM=1`.
fn offline(call: Option<&str>, args: &[&str], stdin: &str) -> Run {
    offline_with(call, true, args, stdin)
}

fn offline_with(call: Option<&str>, single_shot: bool, args: &[&str], stdin: &str) -> Run {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("snapshot.json");
    std::fs::write(&file, snapshot().to_string()).unwrap();
    let mut cmd = fin_cmd(&dir.path().join("no-server"), dir.path(), args);
    cmd.env("FIN_OFFLINE_SNAPSHOT", &file)
        .stdin(std::process::Stdio::piped());
    match call {
        Some(c) => cmd.env("FIN_CALL", c),
        None => cmd.env_remove("FIN_CALL"),
    };
    if single_shot {
        cmd.env("FIN_CALL_IDEM", "1");
    }
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    finish(args, child.wait_with_output().unwrap())
}

#[test]
fn fin_call_becomes_idem_key_for_writes() {
    for args in [
        &["clip", "delete", "c1"][..],
        &["item", "trim", "c2", "--end", "-1s"],
        &["clip", "trim", "c1", "--start", "+1s"],
    ] {
        let doc = ok(offline(Some(CALL), args, ""));
        assert_eq!(doc["dryRun"], true, "{args:?}: {doc}");
        assert_eq!(doc["txId"], CALL, "{args:?}: {doc}");
    }
}

#[test]
fn batch_lines_share_the_call_key() {
    // A batch is one transaction, so its key is FIN_CALL itself (no per-line suffix).
    let lines = "{\"cmd\":\"clip.delete\",\"args\":{\"clip\":\"c1\"}}\n{\"cmd\":\"clip.delete\",\"args\":{\"clip\":\"c2\"}}\n";
    let doc = ok(offline(Some(CALL), &["batch"], lines));
    assert_eq!(doc["txId"], CALL, "{doc}");
    assert_eq!(doc["resolved"].as_array().unwrap().len(), 2);
}

#[test]
fn fin_call_without_the_single_shot_marker_is_not_a_key() {
    // A shell's `fin`: FIN_CALL is only the actor header's call, never the txId.
    for marker in [None, Some("0"), Some("yes")] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("snapshot.json");
        std::fs::write(&file, snapshot().to_string()).unwrap();
        let args = ["clip", "delete", "c1"];
        let mut cmd = fin_cmd(&dir.path().join("no-server"), dir.path(), &args);
        cmd.env("FIN_OFFLINE_SNAPSHOT", &file).env("FIN_CALL", CALL);
        if let Some(m) = marker {
            cmd.env("FIN_CALL_IDEM", m);
        }
        let doc = ok(finish(&args, cmd.output().unwrap()));
        assert_ne!(doc["txId"], CALL, "{marker:?}: {doc}");
        assert_eq!(doc["txId"].as_str().unwrap().len(), 26);
    }
    // Nor does a malformed FIN_CALL matter without the marker.
    ok(offline_with(
        Some("not a call"),
        false,
        &["clip", "delete", "c1"],
        "",
    ));
}

#[test]
fn explicit_idem_key_wins() {
    let doc = ok(offline(
        Some(CALL),
        &["clip", "delete", "c1", "--idem-key", "mine-1"],
        "",
    ));
    assert_eq!(doc["txId"], "mine-1", "{doc}");
}

#[test]
fn no_fin_call_no_change() {
    let a = ok(offline_with(None, false, &["clip", "delete", "c1"], ""));
    let b = ok(offline_with(None, false, &["clip", "delete", "c1"], ""));
    let (a, b) = (a["txId"].as_str().unwrap(), b["txId"].as_str().unwrap());
    assert_eq!(a.len(), 26, "a fresh ULID: {a}");
    assert_ne!(a, b);
    assert_ne!(a, CALL);
}

#[test]
fn reads_ignore_fin_call() {
    // Even a malformed FIN_CALL changes nothing for a read.
    for call in [CALL, "not a call id"] {
        ok(offline(Some(call), &["view"], ""));
        ok(offline(Some(call), &["status"], ""));
        ok(offline(Some(call), &[], ""));
        ok(offline(Some(call), &["schema", "clip.trim"], ""));
    }
}

#[test]
fn a_malformed_fin_call_is_refused_for_a_write() {
    for call in ["has a space", "", "a/b"] {
        let run = offline(Some(call), &["clip", "delete", "c1"], "");
        assert_error(&run, 2, "USAGE");
        assert_eq!(run.json["error"]["details"]["field"], "FIN_CALL");
    }
    // The single-shot marker without a call id is refused too.
    assert_error(&offline(None, &["clip", "delete", "c1"], ""), 2, "USAGE");
}

/// A server and a project `film` holding asset a1.
fn project(test: &str) -> Option<(Server, PathBuf)> {
    let server = Server::start(test)?;
    let dir = server.root.join("film");
    ok(support::fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    media(&dir.join("clip.mp4"), 10);
    ok(fin_env(
        &server.sd,
        &dir,
        &["asset", "add", "clip.mp4"],
        &[("FIN_AUTHOR", OsStr::new("setup"))],
    ));
    Some((server, dir))
}

/// The built-in `fin` tool's environment: one `fin` per call.
fn with_call(server: &Server, dir: &Path, call: &str, args: &[&str]) -> Run {
    fin_env(
        &server.sd,
        dir,
        args,
        &[
            ("FIN_CALL", OsStr::new(call)),
            ("FIN_CALL_IDEM", OsStr::new("1")),
        ],
    )
}

/// A shell tool call's environment: `FIN_CALL` for the join, no single-shot marker.
fn in_shell(server: &Server, dir: &Path, call: &str, args: &[&str]) -> Run {
    fin_env(&server.sd, dir, args, &[("FIN_CALL", OsStr::new(call))])
}

#[test]
fn a_shell_running_several_fins_under_one_call_runs_every_command() {
    let Some((server, dir)) =
        project("a_shell_running_several_fins_under_one_call_runs_every_command")
    else {
        return;
    };
    let insert = [
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s",
    ];
    // `fin clip insert …; fin clip insert …; fin clip insert --dur 2s …` in one shell call.
    let a = ok(in_shell(&server, &dir, CALL, &insert));
    let b = ok(in_shell(&server, &dir, CALL, &insert));
    let mut longer = insert.to_vec();
    longer[9] = "2s";
    let c = ok(in_shell(&server, &dir, CALL, &longer));
    for doc in [&a, &b, &c] {
        assert_ne!(doc["txId"], CALL, "{doc}");
    }
    assert!(b["rev"].as_u64() > a["rev"].as_u64() && c["rev"].as_u64() > b["rev"].as_u64());
    let view = ok(support::fin(&server.sd, &dir, &["view"]));
    assert_eq!(support::view_items(&view).len(), 3, "{view}");
    // `fin undo; fin undo`: two undos, not one deduped.
    let u1 = ok(in_shell(&server, &dir, CALL, &["undo"]));
    let u2 = ok(in_shell(&server, &dir, CALL, &["undo"]));
    assert_ne!(u1["txId"], u2["txId"]);
    let view = ok(support::fin(&server.sd, &dir, &["view"]));
    assert_eq!(support::view_items(&view).len(), 1, "{view}");
}

#[test]
fn retried_write_with_same_fin_call_is_a_dedupe() {
    let Some((server, dir)) = project("retried_write_with_same_fin_call_is_a_dedupe") else {
        return;
    };
    let insert = [
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s",
    ];
    let first = ok(with_call(&server, &dir, CALL, &insert));
    let again = ok(with_call(&server, &dir, CALL, &insert));
    assert_eq!(first["txId"], CALL, "{first}");
    assert_eq!(again["txId"], CALL);
    assert_eq!(
        again["rev"], first["rev"],
        "a dedupe answers the first commit"
    );
    let other = ok(with_call(
        &server,
        &dir,
        "01JC0000000000000000000007",
        &insert,
    ));
    assert!(other["rev"].as_u64() > first["rev"].as_u64(), "{other}");
    let view = ok(support::fin(&server.sd, &dir, &["view"]));
    assert_eq!(support::view_items(&view).len(), 2, "{view}");

    // A retried undo is one undo.
    let undo_call = "01JC0000000000000000000008";
    let undone = ok(with_call(&server, &dir, undo_call, &["undo"]));
    let retried = ok(with_call(&server, &dir, undo_call, &["undo"]));
    assert_eq!(undone["txId"], undo_call, "{undone}");
    assert_eq!(retried["rev"], undone["rev"], "{retried}");
    let view = ok(support::fin(&server.sd, &dir, &["view"]));
    assert_eq!(support::view_items(&view).len(), 1, "{view}");

    // The same call id with other arguments is IDEM_KEY_REUSED, never a second edit.
    let reused = with_call(
        &server,
        &dir,
        CALL,
        &[
            "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "2s",
        ],
    );
    assert_eq!(
        reused.json["error"]["code"], "IDEM_KEY_REUSED",
        "{}",
        reused.stdout
    );
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
