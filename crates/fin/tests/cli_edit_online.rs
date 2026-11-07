//! `fin` edit commands (plan 4C Task 23) committed through a real `finisherd`: linked audio,
//! composed splits and transitions, every structural group, refusals with their exit codes,
//! undo of a composed tx, and `--idem-key` retries of composed commands. Image and graphic
//! inserts need probed image and template assets (Task 21), so only their dry runs are tested
//! (`cli_edit.rs`).

mod support;

use std::path::PathBuf;

use serde_json::{Value, json};
use support::{Server, assert_error, fin, media, media_av, ok};

/// A server and a project `<root>/film` (rate 30, t1 video, t2 audio) holding `av.mp4`
/// (10 s, video and audio) as a1 and `v.mp4` (10 s, video only) as a2.
fn project(test: &str) -> Option<(Server, PathBuf)> {
    let server = Server::start(test)?;
    let dir = server.root.join("film");
    ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    media_av(&dir.join("av.mp4"), 10);
    media(&dir.join("v.mp4"), 10);
    for f in ["av.mp4", "v.mp4"] {
        ok(fin(&server.sd, &dir, &["asset", "add", f]));
    }
    Some((server, dir))
}

#[test]
fn linked_audio_split_and_transition_commit() {
    let Some((server, dir)) = project("linked_audio_split_and_transition_commit") else {
        return;
    };
    let run = |args: &[&str]| ok(fin(&server.sd, &dir, args));
    let first = run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s",
    ]);
    assert_eq!(first["rev"], 3);
    assert_eq!(
        first["bindings"],
        json!({ "$clip": "c1", "$audio": "c2", "$link": "lk1" })
    );
    assert_eq!(
        first["resolved"]["audio"],
        json!({ "track": "t2", "id": "$audio" })
    );
    let second = run(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--in", "4s", "--dur",
        "3s",
    ]);
    assert_eq!(
        second["bindings"],
        json!({ "$clip": "c3", "$audio": "c4", "$link": "lk2" })
    );
    assert_eq!(second["resolved"]["at"], "4", "echoed from the commit");

    // A split cuts both legs and links the right parts.
    let split = run(&["item", "split", "c3", "--at", "5s"]);
    assert_eq!(
        split["bindings"],
        json!({ "$split": "c5", "$split2": "c6", "$link": "lk3" })
    );

    // A transition at the cut, and a crossfade at the linked audio's cut.
    let t = run(&["transition", "set", "--between", "c1", "c3", "--dur", "1s"]);
    assert_eq!(
        t["bindings"],
        json!({ "$transition": "tr1", "$transition2": "tr2" })
    );
    assert_eq!(
        t["resolved"]["transitions"][1]["between"],
        json!(["c2", "c4"])
    );
    // Setting it again updates both.
    let again = run(&[
        "transition",
        "set",
        "--between",
        "c1",
        "c3",
        "--dur",
        "f20",
        "--curve",
        "equalPower",
    ]);
    assert_eq!(again["bindings"], json!({}));
    assert_eq!(again["resolved"]["transitions"][0]["updated"], true);
    run(&["transition", "remove", "tr2"]);

    // An overwrite inside an item splits it first.
    let ow = run(&[
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "1s",
        "--asset",
        "a2",
        "--dur",
        "1s",
        "--mode",
        "overwrite",
    ]);
    assert_eq!(ow["bindings"], json!({ "$split": "c7", "$clip": "c8" }));
    assert_eq!(ow["resolved"]["splits"][0]["item"], "c1");

    // Undo takes the whole composed tx back.
    let undone = ok(fin(&server.sd, &dir, &["undo"]));
    let view = ok(fin(&server.sd, &dir, &["view", "--track", "t1"]));
    let ids: Vec<&str> = view["tracks"][0]["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&"c7") && !ids.contains(&"c8"),
        "{ids:?} after {undone}"
    );
}

#[test]
fn item_commands_commit_with_echoes() {
    let Some((server, dir)) = project("item_commands_commit_with_echoes") else {
        return;
    };
    let run = |args: &[&str]| ok(fin(&server.sd, &dir, args));
    run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s",
    ]);
    run(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--in", "4s", "--dur",
        "4s",
    ]);
    let trimmed = run(&["item", "trim", "c3", "--end", "-1s"]);
    assert_eq!(trimmed["resolved"]["edgeAt"], "7");
    assert_eq!(trimmed["resolved"]["items"], json!(["c3", "c4"]));
    let rolled = run(&["item", "trim", "c1", "--end", "+1s", "--mode", "roll"]);
    assert_eq!(rolled["resolved"]["edgeAt"], "5");
    let slipped = run(&["item", "slip", "c3", "-1s"]);
    assert_eq!(slipped["resolved"]["sourceIn"], "4");
    let moved = run(&["item", "move", "c3", "--to", "20s"]);
    assert_eq!(moved["resolved"]["mode"], "ripple");
    run(&["item", "unlink", "lk1"]);
    let linked = run(&["item", "link", "c1", "c2"]);
    assert_eq!(linked["bindings"], json!({ "$link": "lk3" }));
    let deleted = run(&["item", "delete", "c1", "--mode", "lift"]);
    assert_eq!(deleted["resolved"]["items"], json!(["c1", "c2"]));

    // Refusals keep their exit codes and say what to run.
    let roll = fin(
        &server.sd,
        &dir,
        &["item", "trim", "c3", "--end", "+1s", "--mode", "roll"],
    );
    assert_error(&roll, 3, "NOT_ADJACENT");
    let none = fin(
        &server.sd,
        &dir,
        &[
            "clip", "insert", "--track", "t2", "--at", "1s", "--asset", "a2", "--dur", "1s",
        ],
    );
    assert_error(&none, 3, "TRACK_KIND_MISMATCH");
}

#[test]
fn linked_legs_meeting_on_one_track_is_refused() {
    let Some((server, dir)) = project("linked_legs_meeting_on_one_track_is_refused") else {
        return;
    };
    let run = |args: &[&str]| ok(fin(&server.sd, &dir, args));
    run(&["track", "add", "--kind", "video"]);
    run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a2", "--dur", "2s",
    ]);
    run(&[
        "clip", "insert", "--track", "t3", "--at", "0", "--asset", "a2", "--dur", "2s",
    ]);
    run(&["item", "link", "c1", "c2"]);
    let r = fin(
        &server.sd,
        &dir,
        &["item", "move", "c1", "--to", "5s", "--track", "t3"],
    );
    assert_error(&r, 3, "LINK_LEGS_OVERLAP");
    assert!(
        r.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains("fin item unlink")
    );
}

#[test]
fn structure_commands_commit() {
    let Some((server, dir)) = project("structure_commands_commit") else {
        return;
    };
    let run = |args: &[&str]| ok(fin(&server.sd, &dir, args));
    let added = run(&["track", "add", "--kind", "audio", "--name", "Music"]);
    assert_eq!(added["bindings"], json!({ "$track": "t3" }));
    assert_eq!(
        run(&["track", "set", "t3", "gain", "-3dB"])["resolved"]["value"],
        -3.0
    );
    run(&["track", "set", "t3", "gain", "--reset"]);
    run(&["track", "move", "t3", "--index", "0"]);
    run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a2", "--dur", "1s",
    ]);
    assert_error(
        &fin(&server.sd, &dir, &["track", "remove", "t1"]),
        3,
        "TRACK_NOT_EMPTY",
    );
    run(&["track", "remove", "t3"]);

    let tl = run(&[
        "timeline",
        "add",
        "--name",
        "Square",
        "--resolution",
        "1080x1080",
    ]);
    assert_eq!(tl["bindings"], json!({ "$timeline": "tl2" }));
    run(&["timeline", "set", "tl2", "rate", "25"]);
    let m = run(&[
        "--timeline",
        "tl2",
        "marker",
        "add",
        "--at",
        "f50",
        "--label",
        "drop",
    ]);
    assert_eq!(
        (m["bindings"].clone(), m["resolved"]["at"].clone()),
        (json!({ "$marker": "m1" }), json!("2"))
    );
    run(&["marker", "set", "m1", "color", "#0f0"]);
    run(&["marker", "remove", "m1"]);
    run(&["timeline", "remove", "tl2"]);
    assert_error(
        &fin(&server.sd, &dir, &["timeline", "set", "tl1", "rate", "25"]),
        3,
        "TIMELINE_NOT_EMPTY",
    );

    assert_eq!(
        run(&["bin", "add", "--name", "Footage"])["bindings"],
        json!({ "$bin": "b1" })
    );
    run(&["bin", "add", "--name", "Sub", "--parent", "b1"]);
    run(&["asset", "move", "a1", "--bin", "b2"]);
    assert_error(
        &fin(&server.sd, &dir, &["bin", "remove", "b2"]),
        3,
        "BIN_NOT_EMPTY",
    );
    assert_error(
        &fin(&server.sd, &dir, &["bin", "move", "b1", "--parent", "b2"]),
        3,
        "BIN_CYCLE",
    );
    run(&["asset", "move", "a1"]);
    run(&["bin", "move", "b2"]);
    run(&["bin", "remove", "b2"]);
    let used = fin(&server.sd, &dir, &["asset", "remove", "a2"]);
    assert_error(&used, 3, "ASSET_IN_USE");
    assert_eq!(used.json["error"]["details"]["items"], json!(["c1"]));
    run(&["asset", "remove", "a1"]);
    let status = run(&["status"]);
    assert_eq!(status["assets"], json!(["a2"]));
}

#[test]
fn composed_retry_with_idem_key_is_answered_once() {
    let Some((server, dir)) = project("composed_retry_with_idem_key_is_answered_once") else {
        return;
    };
    let args = [
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "0",
        "--asset",
        "a1",
        "--dur",
        "2s",
        "--idem-key",
        "k1",
    ];
    let first = ok(fin(&server.sd, &dir, &args));
    let again = ok(fin(&server.sd, &dir, &args));
    assert_eq!(first, again);
    assert_eq!(
        first["bindings"],
        json!({ "$clip": "c1", "$audio": "c2", "$link": "lk1" })
    );
    // The same key with other args (here: no audio) is another command.
    let mut other: Vec<&str> = args.to_vec();
    other.push("--no-audio");
    assert_error(&fin(&server.sd, &dir, &other), 3, "IDEM_KEY_REUSED");
    let status: Value = ok(fin(&server.sd, &dir, &["status"]));
    assert_eq!(status["rev"], 3);
    // A retry whose command no longer plans against the new state (lk1 is gone after the
    // first unlink) is answered from the log, not refused.
    let unlink = ["item", "unlink", "lk1", "--idem-key", "k2"];
    let first = ok(fin(&server.sd, &dir, &unlink));
    let again = ok(fin(&server.sd, &dir, &unlink));
    assert_eq!(first["rev"], again["rev"]);
    assert_eq!(first["txId"], again["txId"]);
    // Without a key it is refused as before.
    assert_error(
        &fin(&server.sd, &dir, &["item", "unlink", "lk1"]),
        3,
        "NOT_FOUND",
    );
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
