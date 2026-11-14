//! Branch and history commands against a real `finisherd` (plan 4B Task 19). Tests that need
//! the server print `SKIPPED <test>: <reason>` and pass when Node or fin-node is missing.

mod support;

use std::ffi::OsStr;
use std::path::PathBuf;

use serde_json::{Value, json};
use support::{Run, Server, assert_error, fin, fin_env, media_av, ok};

/// A server and a project `film` holding asset a1 (rev 1).
fn project(test: &str) -> Option<(Server, PathBuf)> {
    let server = Server::start(test)?;
    let dir = server.root.join("film");
    ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    // Video and audio (clips go on both tracks); inserts pass --no-audio to stay one item each.
    media_av(&dir.join("clip.mp4"), 10);
    // Added by another author, so `fin`'s undo stack starts empty.
    ok(fin_env(
        &server.sd,
        &dir,
        &["asset", "add", "clip.mp4"],
        &[("FIN_AUTHOR", OsStr::new("setup"))],
    ));
    Some((server, dir))
}

struct T<'a>(&'a Server, &'a PathBuf);

impl T<'_> {
    fn raw(&self, args: &[&str]) -> Run {
        fin(&self.0.sd, self.1, args)
    }
    fn run(&self, args: &[&str]) -> Value {
        ok(self.raw(args))
    }
    fn as_author(&self, author: &str, args: &[&str]) -> Run {
        fin_env(
            &self.0.sd,
            self.1,
            args,
            &[("FIN_AUTHOR", OsStr::new(author))],
        )
    }
    /// Inserts a 1 s clip at the end of `track`; returns the write document.
    fn insert(&self, extra: &[&str], track: &str) -> Value {
        let mut args = vec![
            "clip",
            "insert",
            "--track",
            track,
            "--at",
            "end",
            "--asset",
            "a1",
            "--no-audio",
            "--dur",
            "1s",
        ];
        args.extend_from_slice(extra);
        self.run(&args)
    }
    fn clips(&self, extra: &[&str]) -> Vec<String> {
        let mut args = extra.to_vec();
        args.push("view");
        let v = self.run(&args);
        support::view_items(&v)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect()
    }
    fn branches(&self, all: bool) -> Vec<Value> {
        let args: &[&str] = if all {
            &["branch", "ls", "--all"]
        } else {
            &["branch", "ls"]
        };
        self.run(args)["branches"].as_array().unwrap().clone()
    }
    fn names(&self, all: bool) -> Vec<String> {
        self.branches(all)
            .iter()
            .map(|b| b["display"].as_str().unwrap().to_string())
            .collect()
    }
    /// The try records of every project in the file, as one `{branchId: parentId}` map
    /// (single-project tests).
    fn tries(&self) -> Value {
        let mut all = serde_json::Map::new();
        for project in self.config()["tries"]
            .as_object()
            .into_iter()
            .flat_map(|t| t.values())
        {
            all.extend(project.as_object().unwrap().clone());
        }
        Value::Object(all)
    }
    fn config(&self) -> Value {
        match std::fs::read_to_string(self.0.sd.join("fin-config.json")) {
            Ok(t) => serde_json::from_str(&t).unwrap(),
            Err(_) => Value::Null,
        }
    }
}

macro_rules! setup {
    ($name:literal) => {{
        let Some((server, dir)) = project($name) else {
            return;
        };
        (server, dir)
    }};
}

/// What `fin status` says of the project's content (not of exports or the lease, which a
/// background export may change between two reads).
fn state(status: &Value) -> Value {
    json!([
        status["head"],
        status["rev"],
        status["timelines"],
        status["assets"],
        status["unmergedBranches"]
    ])
}

#[test]
fn branch_new_ls_rename_archive() {
    let (server, dir) = setup!("branch_new_ls_rename_archive");
    let t = T(&server, &dir);
    let made = t.run(&["branch", "new", "b1"]);
    assert_eq!(made["branch"]["id"], "br1", "{made}");
    assert_eq!(made["branch"]["name"], "b1");
    assert_eq!(made["branch"]["parent"], "main");
    assert!(made["rev"].is_u64() && made["txId"].is_string(), "{made}");
    assert_eq!(t.names(false), ["main", "b1"]);

    let renamed = t.run(&["branch", "rename", "b1", "b2"]);
    assert!(renamed["rev"].is_u64(), "{renamed}");
    assert_eq!(t.names(false), ["main", "b2"]);

    t.run(&["branch", "archive", "b2"]);
    assert_eq!(t.names(false), ["main"]);

    // A taken name is NAME_TAKEN (3); main and ids are not names (INVALID_NAME, 3).
    t.run(&["branch", "new", "dup"]);
    assert_error(&t.raw(&["branch", "new", "dup"]), 3, "NAME_TAKEN");
    assert_error(&t.raw(&["branch", "new", "main"]), 3, "INVALID_NAME");
    assert_error(&t.raw(&["branch", "archive", "main"]), 3, "INVALID_OP");
    assert_error(
        &t.raw(&["branch", "new", "x", "--from", "nope"]),
        3,
        "BRANCH_NOT_FOUND",
    );
}

#[test]
fn branch_new_from_seq_and_checkpoint() {
    let (server, dir) = setup!("branch_new_from_seq_and_checkpoint");
    let t = T(&server, &dir);
    t.insert(&[], "t1"); // seq 2
    t.run(&["checkpoint", "cp"]); // seq 3
    t.insert(&[], "t1"); // seq 4
    assert_eq!(t.clips(&[]), ["c1", "c2"]);
    t.run(&["branch", "new", "s", "--from", "main@2"]);
    t.run(&["branch", "new", "c", "--from", "main@cp"]);
    t.run(&["branch", "new", "h", "--from", "main"]);
    assert_eq!(t.clips(&["--branch", "s"]), ["c1"]);
    assert_eq!(t.clips(&["--branch", "c"]), ["c1"]);
    assert_eq!(t.clips(&["--branch", "h"]), ["c1", "c2"]);
    let forks: Vec<Value> = t
        .branches(false)
        .iter()
        .map(|b| b["forkSeq"].clone())
        .collect();
    assert_eq!(forks, [json!(0), json!(2), json!(3), json!(6)]);
    assert_error(
        &t.raw(&["branch", "new", "z", "--from", "main@nothere"]),
        3,
        "NOT_FOUND",
    );
    assert_error(
        &t.raw(&["branch", "new", "z", "--from", "main@"]),
        2,
        "USAGE",
    );
}

#[test]
fn branch_ls_all_shows_archived() {
    let (server, dir) = setup!("branch_ls_all_shows_archived");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "old"]);
    t.run(&["branch", "archive", "old"]);
    assert_eq!(t.names(false), ["main"]);
    let all = t.branches(true);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[1]["state"], "archived");
    assert_eq!(all[1]["display"], "old#br1");
}

#[test]
fn archive_restore_as() {
    let (server, dir) = setup!("archive_restore_as");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "x"]);
    t.run(&["branch", "archive", "x"]);
    // An archived branch refuses writes.
    let refused = t.raw(&[
        "--branch",
        "br1",
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "0",
        "--asset",
        "a1",
        "--no-audio",
        "--dur",
        "1s",
    ]);
    assert_error(&refused, 3, "BRANCH_ARCHIVED");
    t.run(&["branch", "new", "x"]);
    let taken = t.raw(&["branch", "archive", "br1", "--restore"]);
    assert_error(&taken, 3, "NAME_TAKEN");
    assert!(
        taken.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains("--as"),
        "{}",
        taken.stdout
    );
    t.run(&["branch", "archive", "br1", "--restore", "--as", "y"]);
    assert_eq!(t.names(false), ["main", "y", "x"]);
    assert_error(
        &t.raw(&["branch", "archive", "br1", "--as", "z"]),
        2,
        "USAGE",
    );
}

#[test]
fn global_branch_flag_targets_writes_and_views() {
    let (server, dir) = setup!("global_branch_flag_targets_writes_and_views");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "side"]);
    let wrote = t.run(&[
        "--branch",
        "side",
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "0",
        "--asset",
        "a1",
        "--no-audio",
        "--dur",
        "2s",
    ]);
    assert_eq!(wrote["bindings"], json!({ "$clip": "c1" }));
    assert_eq!(t.clips(&["--branch", "side"]), ["c1"]);
    assert_eq!(t.clips(&[]), Vec::<String>::new());
    assert_eq!(
        t.run(&["--branch", "br1", "status"])["timelines"][0]["tracks"][0]["items"],
        1
    );
    assert_eq!(t.run(&["status"])["timelines"][0]["tracks"][0]["items"], 0);
    assert_error(&t.raw(&["--branch", "nope", "view"]), 3, "BRANCH_NOT_FOUND");
    // `project acknowledge` takes the global flag now.
    let ack = t.raw(&[
        "project",
        "acknowledge",
        "--rewind",
        "1",
        "--branch",
        "main",
    ]);
    assert_ne!(ack.json["error"]["code"], "USAGE", "{}", ack.stdout);
}

#[test]
fn undo_redo_roundtrip() {
    let (server, dir) = setup!("undo_redo_roundtrip");
    let t = T(&server, &dir);
    let first = t.insert(&[], "t1");
    let undone = t.run(&["undo"]);
    assert_eq!(undone["undone"], json!([first["txId"]]), "{undone}");
    assert!(undone["rev"].as_u64().unwrap() > first["rev"].as_u64().unwrap());
    assert_eq!(t.clips(&[]), Vec::<String>::new());
    let redone = t.run(&["redo"]);
    assert_eq!(redone["redone"], json!([undone["txId"]]), "{redone}");
    assert_eq!(t.clips(&[]), ["c1"]);
    assert_error(&t.raw(&["redo"]), 3, "NOTHING_TO_REDO");
    t.run(&["undo"]);
    assert_error(&t.raw(&["undo"]), 3, "NOTHING_TO_UNDO");
}

#[test]
fn undo_to_checkpoint() {
    let (server, dir) = setup!("undo_to_checkpoint");
    let t = T(&server, &dir);
    t.insert(&[], "t1");
    t.run(&["checkpoint", "cp"]);
    t.insert(&[], "t1");
    t.insert(&[], "t2");
    assert_eq!(t.clips(&[]).len(), 3);
    let r = t.run(&["undo", "--to", "cp"]);
    assert_eq!(r["undone"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(t.clips(&[]), ["c1"]);
    assert_error(&t.raw(&["undo", "--to", "nope"]), 3, "NOT_FOUND");
    assert_error(&t.raw(&["checkpoint", "cp"]), 3, "NAME_TAKEN");
    assert_error(&t.raw(&["checkpoint", "123"]), 3, "INVALID_NAME");
}

/// main has clip c1; `side` forks from it and trims it, main deletes it; `side` also adds a
/// clip on t2 that merges cleanly. Returns the txId of the side trim.
fn conflicting(t: &T) -> String {
    t.insert(&[], "t1");
    t.run(&["branch", "new", "side"]);
    let trim = t.run(&["--branch", "side", "clip", "trim", "c1", "--end", "-0.5s"]);
    t.run(&[
        "--branch",
        "side",
        "clip",
        "insert",
        "--track",
        "t2",
        "--at",
        "0",
        "--asset",
        "a1",
        "--no-audio",
        "--dur",
        "1s",
    ]);
    t.run(&["clip", "delete", "c1"]);
    trim["txId"].as_str().unwrap().to_string()
}

#[test]
fn merge_with_conflict_exit_4_lists_conflicts() {
    let (server, dir) = setup!("merge_with_conflict_exit_4_lists_conflicts");
    let t = T(&server, &dir);
    conflicting(&t);
    let before = state(&t.run(&["status"]));
    let run = t.raw(&["merge", "side"]);
    assert_error(&run, 4, "CONFLICT");
    let conflicts = run.json["error"]["details"]["conflicts"]
        .as_array()
        .unwrap();
    assert_eq!(conflicts.len(), 1, "{}", run.stdout);
    assert!(conflicts[0]["reason"].is_string() && conflicts[0]["fix"].is_string());
    assert_eq!(
        state(&t.run(&["status"])),
        before,
        "a refused merge writes nothing"
    );
}

#[test]
fn merge_skip_conflicting() {
    let (server, dir) = setup!("merge_skip_conflicting");
    let t = T(&server, &dir);
    conflicting(&t);
    let m = t.run(&["merge", "side", "--into", "main", "--skip-conflicting"]);
    assert_eq!(m["merge"]["skipped"].as_array().unwrap().len(), 1, "{m}");
    assert_eq!(m["merge"]["merged"].as_array().unwrap().len(), 1, "{m}");
    assert_eq!(m["merge"]["records"], 1);
    assert!(m["merge"]["rev"].is_u64() && m["merge"]["mergeId"].is_string());
    assert!(m["merge"].get("committed").is_none());
    assert_eq!(t.clips(&[]), ["c2"]);
}

#[test]
fn cherry_pick_and_revert() {
    let (server, dir) = setup!("cherry_pick_and_revert");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "side"]);
    let a = t.run(&[
        "--branch",
        "side",
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "0",
        "--asset",
        "a1",
        "--no-audio",
        "--dur",
        "1s",
    ]);
    let b = t.run(&[
        "--branch",
        "side",
        "clip",
        "insert",
        "--track",
        "t2",
        "--at",
        "0",
        "--asset",
        "a1",
        "--no-audio",
        "--dur",
        "1s",
    ]);
    let tx = a["txId"].as_str().unwrap();
    let picked = t.run(&["cherry-pick", tx]);
    assert!(picked["rev"].is_u64(), "{picked}");
    assert_eq!(t.clips(&[]), ["c1"]);
    let again = t.run(&["cherry-pick", tx, "--into", "main"]);
    assert_eq!(again["noOp"], "ALREADY_PRESENT", "{again}");
    assert_eq!(again["rev"], Value::Null);
    assert_error(&t.raw(&["cherry-pick", "nope"]), 3, "NOT_FOUND");

    let merged = t.run(&["--idem-key", "m1", "merge", "side"]);
    assert_eq!(merged["merge"]["mergeId"], "m1", "{merged}");
    assert_eq!(t.clips(&[]), ["c1", "c2"]);
    let reverted = t.run(&["revert", "m1"]);
    assert_eq!(reverted["reverted"], json!(["m1"]), "{reverted}");
    assert_eq!(t.clips(&[]), ["c1"]);
    let _ = b;
    assert_error(&t.raw(&["revert", "nope"]), 3, "NOT_FOUND");
}

#[test]
fn try_without_name_then_keep() {
    let (server, dir) = setup!("try_without_name_then_keep");
    let t = T(&server, &dir);
    let started = t.run(&["try"]);
    assert_eq!(started["branch"]["name"], "try-1", "{started}");
    let cfg = t.config();
    assert_eq!(t.tries(), json!({ "br1": "main" }), "{cfg}");
    assert!(
        cfg["defaultBranch"]
            .as_object()
            .unwrap()
            .values()
            .any(|v| v == "br1")
    );
    // Edits now land on the try branch; main is untouched.
    t.insert(&[], "t1");
    assert_eq!(t.clips(&[]), ["c1"]);
    assert_eq!(t.clips(&["--branch", "main"]), Vec::<String>::new());
    let kept = t.run(&["try", "keep"]);
    assert_eq!(
        kept["merge"]["merged"].as_array().unwrap().len(),
        1,
        "{kept}"
    );
    assert_eq!(kept["switchedTo"], "main");
    assert_eq!(t.clips(&["--branch", "main"]), ["c1"]);
    assert_eq!(t.run(&["status"])["timelines"][0]["tracks"][0]["items"], 1);
    let cfg = t.config();
    assert_eq!(t.tries(), json!({}), "{cfg}");
    assert_eq!(cfg["defaultBranch"], json!({}), "{cfg}");
    // Named, and from another branch than main.
    t.run(&["try", "scratch"]);
    assert_eq!(t.tries(), json!({ "br2": "main" }));
}

#[test]
fn try_drop_archives() {
    let (server, dir) = setup!("try_drop_archives");
    let t = T(&server, &dir);
    t.run(&["try", "scratch"]);
    t.insert(&[], "t1");
    let dropped = t.run(&["try", "drop"]);
    assert_eq!(dropped["branch"]["state"], "archived", "{dropped}");
    assert_eq!(t.names(false), ["main"]);
    assert_eq!(t.tries(), json!({}));
    assert_eq!(t.clips(&[]), Vec::<String>::new());
    // Back on main: another drop is not on a try branch.
    assert_error(&t.raw(&["try", "drop"]), 2, "USAGE");
}

#[test]
fn try_keep_outside_try_is_usage() {
    let (server, dir) = setup!("try_keep_outside_try_is_usage");
    let t = T(&server, &dir);
    for sub in ["keep", "drop"] {
        let run = t.raw(&["try", sub]);
        assert_error(&run, 2, "USAGE");
        assert_eq!(run.json["error"]["fix"], "fin try");
    }
    // A branch fin did not make as a try is no try either.
    t.run(&["branch", "new", "plain"]);
    assert_error(&t.raw(&["--branch", "plain", "try", "keep"]), 2, "USAGE");
    assert_eq!(t.names(false), ["main", "plain"]);
}

#[test]
fn fin_author_env_scopes_undo() {
    let (server, dir) = setup!("fin_author_env_scopes_undo");
    let t = T(&server, &dir);
    let by_alice = ok(t.as_author(
        "alice",
        &[
            "clip",
            "insert",
            "--track",
            "t1",
            "--at",
            "0",
            "--asset",
            "a1",
            "--no-audio",
            "--dur",
            "1s",
        ],
    ));
    assert_eq!(by_alice["bindings"]["$clip"], "c1");
    assert_error(&t.raw(&["undo"]), 3, "NOTHING_TO_UNDO");
    assert_eq!(t.clips(&[]), ["c1"]);
    ok(t.as_author("alice", &["undo"]));
    assert_eq!(t.clips(&[]), Vec::<String>::new());
    // Empty means the default author.
    assert_error(&t.as_author("", &["undo"]), 3, "NOTHING_TO_UNDO");
}

#[test]
fn recover_lists_groups() {
    let (server, dir) = setup!("recover_lists_groups");
    let t = T(&server, &dir);
    let r = t.run(&["project", "recover"]);
    assert_eq!(r["groups"], json!([]), "{r}");
    assert_eq!(r["created"], json!([]));
    assert_eq!(r["imported"], 0);
    assert_eq!(r["manual"], Value::Null);
    assert_eq!(r["written"], 0);
    assert!(r.get("committed").is_none());
    let dry = t.run(&["project", "recover", "--create", "all", "--dry-run"]);
    assert_eq!(dry["dryRun"], true, "{dry}");
    let none = t.run(&["project", "recover", "--create", "all"]);
    assert_eq!(none["created"], json!([]), "{none}");
    let missing = t.raw(&["project", "recover", "--import", "no-such-dir"]);
    assert_ne!(missing.code, 0, "{}", missing.stdout);
    assert!(missing.json["error"]["code"].is_string());
    assert_error(
        &t.raw(&["project", "recover", "--create", "abc123abc123"]),
        3,
        "NOT_FOUND",
    );
}

#[test]
fn idem_key_retry_merge_returns_first() {
    let (server, dir) = setup!("idem_key_retry_merge_returns_first");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "side"]);
    t.run(&["branch", "new", "other"]);
    t.run(&[
        "--branch",
        "side",
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "0",
        "--asset",
        "a1",
        "--no-audio",
        "--dur",
        "1s",
    ]);
    let first = t.run(&["--idem-key", "mk", "merge", "side"]);
    let rev = t.run(&["status"])["rev"].clone();
    let again = t.run(&["--idem-key", "mk", "merge", "side"]);
    assert_eq!(again["merge"]["mergeId"], first["merge"]["mergeId"]);
    assert_eq!(again["merge"]["rev"], first["merge"]["rev"], "{again}");
    assert_eq!(t.run(&["status"])["rev"], rev, "the retry wrote nothing");
    // The same key for another command is refused.
    assert_error(
        &t.raw(&["--idem-key", "mk", "merge", "other"]),
        3,
        "IDEM_KEY_REUSED",
    );
    // Keyed checkpoint and undo retry too.
    let cp = t.run(&["--idem-key", "ck", "checkpoint", "one"]);
    assert_eq!(
        t.run(&["--idem-key", "ck", "checkpoint", "one"])["rev"],
        cp["rev"]
    );
    assert_error(
        &t.raw(&["--idem-key", "ck", "checkpoint", "two"]),
        3,
        "IDEM_KEY_REUSED",
    );
    assert_error(&t.raw(&["--idem-key", " ", "undo"]), 2, "USAGE");
}

/// Every write command with `--dry-run`: it answers `dryRun: true` and changes nothing.
#[test]
fn dry_run_each_history_command_writes_nothing() {
    let (server, dir) = setup!("dry_run_each_history_command_writes_nothing");
    let t = T(&server, &dir);
    t.insert(&[], "t1");
    t.run(&["branch", "new", "side"]);
    let picked = t.run(&[
        "--branch",
        "side",
        "clip",
        "insert",
        "--track",
        "t2",
        "--at",
        "0",
        "--asset",
        "a1",
        "--no-audio",
        "--dur",
        "1s",
    ]);
    t.run(&["--idem-key", "m0", "merge", "side"]);
    t.run(&["undo"]);
    t.run(&["branch", "new", "arch"]);
    t.run(&["branch", "archive", "arch"]);
    let snapshot = |t: &T| (t.branches(true), t.config(), state(&t.run(&["status"])));
    let before = snapshot(&t);
    let pick = picked["txId"].as_str().unwrap();
    let cases: Vec<Vec<&str>> = vec![
        vec!["branch", "new", "fresh"],
        vec!["branch", "new", "fresh", "--from", "main@1"],
        vec!["branch", "rename", "side", "renamed"],
        vec!["branch", "archive", "side"],
        vec!["branch", "archive", "arch#br2", "--restore"],
        vec!["checkpoint", "cp"],
        vec!["redo"],
        vec!["merge", "side", "--into", "main"],
        vec!["cherry-pick", pick, "--into", "main"],
        vec!["revert", "m0"],
        vec!["try"],
        vec!["try", "named"],
        vec!["project", "recover", "--create", "all"],
    ];
    for case in cases {
        let mut args = case.clone();
        args.push("--dry-run");
        let run = t.raw(&args);
        // A planned operation or a reported no-op, never an unrelated failure.
        if run.code == 0 {
            let doc = &run.json;
            let flagged = doc["dryRun"] == true || doc["merge"]["dryRun"] == true;
            assert!(flagged, "{args:?}: {}", run.stdout);
        } else {
            panic!("{args:?}: {}", run.stdout);
        }
        assert_eq!(snapshot(&t), before, "{args:?} changed something");
    }
    // undo on a dry run; then keep and drop on a try branch.
    let undo = t.run(&["undo", "--dry-run"]);
    assert_eq!(undo["dryRun"], true, "{undo}");
    assert_eq!(snapshot(&t), before);
    t.run(&["try", "tr"]);
    let before = snapshot(&t);
    for sub in ["keep", "drop"] {
        let run = t.run(&["try", sub, "--dry-run"]);
        assert!(
            run["dryRun"] == true || run["merge"]["dryRun"] == true,
            "{run}"
        );
        assert_eq!(
            snapshot(&t),
            before,
            "try {sub} --dry-run changed something"
        );
    }
}

/// Two projects in one server-data both make `br1`: each keeps its own try and default.
#[test]
fn tries_are_per_project() {
    let (server, dir) = setup!("tries_are_per_project");
    let other = server.root.join("film2");
    ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film2", "--name", "Two"],
    ));
    media_av(&other.join("clip.mp4"), 10);
    ok(fin(&server.sd, &other, &["asset", "add", "clip.mp4"]));
    let (a, b) = (T(&server, &dir), T(&server, &other));
    a.run(&["try", "a-try"]);
    b.run(&["try", "b-try"]);
    let cfg = a.config();
    assert_eq!(cfg["defaultBranch"].as_object().unwrap().len(), 2, "{cfg}");
    assert_eq!(cfg["tries"].as_object().unwrap().len(), 2, "{cfg}");
    a.insert(&[], "t1");
    b.insert(&[], "t1");
    b.insert(&[], "t1");
    let kept = b.run(&["try", "keep"]);
    assert_eq!(
        kept["merge"]["merged"].as_array().unwrap().len(),
        2,
        "{kept}"
    );
    // A's try record and default survived B's keep, and A keeps its own branch.
    let cfg = a.config();
    assert_eq!(cfg["tries"].as_object().unwrap().len(), 1, "{cfg}");
    assert_eq!(a.clips(&[]), ["c1"]);
    assert_eq!(a.clips(&["--branch", "main"]), Vec::<String>::new());
    let kept = a.run(&["try", "keep"]);
    assert_eq!(
        kept["merge"]["merged"].as_array().unwrap().len(),
        1,
        "{kept}"
    );
    assert_eq!(a.clips(&[]), ["c1"]);
    assert_eq!(b.clips(&[]), ["c1", "c2"]);
    let cfg = a.config();
    assert!(
        cfg["tries"]
            .as_object()
            .unwrap()
            .values()
            .all(|t| t == &json!({})),
        "{cfg}"
    );
}

#[test]
fn a_stale_default_names_its_source_and_can_be_cleared() {
    let (server, dir) = setup!("a_stale_default_names_its_source");
    let t = T(&server, &dir);
    t.run(&["try", "scratch"]);
    // The branch goes away behind the file's back: the default now names br99.
    let mut cfg = t.config();
    let pid = cfg["defaultBranch"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    cfg["defaultBranch"][&pid] = json!("br99");
    cfg["tries"][&pid] = json!({ "br99": "main" });
    std::fs::write(server.sd.join("fin-config.json"), cfg.to_string()).unwrap();
    for args in [
        &["view"][..],
        &["clip", "delete", "c1"],
        &["undo"],
        &["try", "keep"],
    ] {
        let run = t.raw(args);
        assert_error(&run, 3, "BRANCH_NOT_FOUND");
        let text = run.stdout.clone();
        assert!(
            text.contains("fin-config.json") && text.contains("branch default --clear"),
            "{args:?}: {text}"
        );
    }
    assert_eq!(t.clips(&["--branch", "main"]), Vec::<String>::new());
    // `try drop` is the way out and leaves the file clean.
    let dropped = t.run(&["try", "drop"]);
    assert_eq!(dropped["cleared"], "br99", "{dropped}");
    assert_eq!(t.clips(&[]), Vec::<String>::new());
    // The same through `branch default`.
    t.run(&["branch", "new", "x"]);
    assert_eq!(t.run(&["branch", "default", "x"])["default"], "br2");
    assert_eq!(t.run(&["branch", "default"])["default"], "br2");
    assert_eq!(
        t.run(&["--dry-run", "branch", "default", "--clear"])["dryRun"],
        true
    );
    assert_eq!(t.run(&["branch", "default"])["default"], "br2");
    assert_eq!(
        t.run(&["branch", "default", "--clear"])["default"],
        Value::Null
    );
    assert_eq!(t.run(&["branch", "default"])["default"], Value::Null);
    assert_error(
        &t.raw(&["branch", "default", "nope"]),
        3,
        "BRANCH_NOT_FOUND",
    );
}

#[test]
fn acknowledge_rewind_resolves_branch_names() {
    let (server, dir) = setup!("acknowledge_rewind_resolves_branch_names");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "side"]);
    let ack = t.run(&[
        "project",
        "acknowledge",
        "--rewind",
        "1",
        "--branch",
        "side",
    ]);
    assert_eq!(ack["acknowledged"], false, "{ack}");
    assert_error(
        &t.raw(&[
            "project",
            "acknowledge",
            "--rewind",
            "1",
            "--branch",
            "nope",
        ]),
        3,
        "BRANCH_NOT_FOUND",
    );
}

#[test]
fn undo_refusals_name_the_author_and_seqs_are_refused() {
    let (server, dir) = setup!("undo_refusals_name_the_author");
    let t = T(&server, &dir);
    let nothing = t.raw(&["undo"]);
    assert_error(&nothing, 3, "NOTHING_TO_UNDO");
    assert_eq!(nothing.json["error"]["details"]["author"], "fin");
    assert!(
        nothing.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains("FIN_AUTHOR")
    );
    let digits = t.raw(&["undo", "--to", "12"]);
    assert_error(&digits, 2, "USAGE");
    assert!(
        digits.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("checkpoint name")
    );
}

#[test]
fn dry_run_with_a_committed_key_is_not_reported_as_dry() {
    let (server, dir) = setup!("dry_run_with_a_committed_key");
    let t = T(&server, &dir);
    let first = t.run(&["--idem-key", "ck", "checkpoint", "one"]);
    let again = t.run(&["--idem-key", "ck", "--dry-run", "checkpoint", "one"]);
    assert_eq!(again["rev"], first["rev"], "{again}");
    assert!(again.get("dryRun").is_none(), "{again}");
    let fresh = t.run(&["--idem-key", "ck2", "--dry-run", "checkpoint", "two"]);
    assert_eq!(fresh["dryRun"], true, "{fresh}");
}

#[test]
fn recover_dismiss_of_an_unknown_group_is_not_found() {
    let (server, dir) = setup!("recover_dismiss_unknown");
    let t = T(&server, &dir);
    assert_error(
        &t.raw(&["project", "recover", "--dismiss", "000000000000"]),
        3,
        "NOT_FOUND",
    );
}

#[test]
fn cli_merge_include_and_up_to() {
    let (server, dir) = setup!("cli_merge_include_and_up_to");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "side"]);
    let side = ["--branch", "side"];
    let a = t.insert(&side, "t1");
    let b = t.insert(&side, "t2");
    let c = t.insert(&side, "t1");
    let id = |v: &Value| v["txId"].as_str().unwrap().to_string();
    let (a, b_rev, b, c) = (id(&a), b["rev"].as_u64().unwrap(), id(&b), id(&c));
    // c is named but above --up-to; b is under it but not named.
    let up_to = b_rev.to_string();
    let m = t.run(&[
        "merge",
        "side",
        "--include",
        &format!("{c},{a}"),
        "--include",
        &a,
        "--up-to",
        &up_to,
    ]);
    assert_eq!(m["merge"]["merged"], json!([a]), "{m}");
    assert_eq!(m["merge"]["notIncluded"], json!([b, c]), "{m}");
    assert_eq!(m["merge"]["records"], 1);
    assert_eq!(t.clips(&[]), ["c1"]);
    // --include takes one id per flag, so the positional after it stays the source;
    // --up-to bounds the source branch: b merges, c stays.
    let m = t.run(&["merge", "--include", &b, "side", "--up-to", &up_to]);
    assert_eq!(m["merge"]["merged"], json!([b]), "{m}");
    assert_eq!(m["merge"]["notIncluded"], json!([c]), "{m}");
    // Without limits the rest merges and nothing is left out.
    let m = t.run(&["merge", "side"]);
    assert_eq!(m["merge"]["merged"], json!([c]), "{m}");
    assert!(m["merge"].get("notIncluded").is_none(), "{m}");
    assert_error(&t.raw(&["merge", "side", "--up-to", "x"]), 2, "USAGE");
}

/// Owner decisions of 2026-11-09 (SP4 §4.3 *Placement of restored items*), as `fin` shows
/// them. c1 c2 c3; the author deletes c2, bob deletes c1 and c3. Under an agent session
/// (`FIN_SESSION`) the undo is exit 4 `CONFLICT` with reason `NEIGHBOURS_GONE` and changes
/// nothing; run by the user it goes back at the nearest boundary with a `notice`.
#[test]
fn undo_with_both_neighbours_gone_by_agent_and_by_user() {
    let (server, dir) = setup!("undo_with_both_neighbours_gone_by_agent_and_by_user");
    let t = T(&server, &dir);
    for _ in 0..3 {
        t.insert(&[], "t1");
    }
    let session = [
        ("FIN_SESSION", OsStr::new("sess-1")),
        ("FIN_AUTHOR", OsStr::new("agent")),
    ];
    ok(fin_env(
        &server.sd,
        &dir,
        &["clip", "delete", "c2"],
        &session,
    ));
    ok(t.as_author("bob", &["clip", "delete", "c1"]));
    ok(t.as_author("bob", &["clip", "delete", "c3"]));
    let refused = fin_env(&server.sd, &dir, &["undo"], &session);
    assert_error(&refused, 4, "CONFLICT");
    let e = &refused.json["error"];
    assert_eq!(e["details"]["reason"], "NEIGHBOURS_GONE", "{e}");
    assert_eq!(
        e["details"]["conflicts"][0]["details"]["placement"]["causes"][0]["author"], "bob",
        "{e}"
    );
    assert!(e["fix"].as_str().unwrap().contains("decide"), "{e}");
    assert_eq!(t.clips(&[]), Vec::<String>::new());
    let done = ok(t.as_author("agent", &["undo"]));
    assert_eq!(done["placements"][0]["outcome"], "relocated", "{done}");
    assert!(
        done["notice"]
            .as_str()
            .unwrap()
            .starts_with("relocated: c2"),
        "{done}"
    );
    assert_eq!(t.clips(&[]), ["c2"]);
    // A clean undo carries the placements and no notice.
    t.insert(&[], "t1");
    ok(t.as_author("agent", &["clip", "delete", "c2"]));
    let clean = ok(t.as_author("agent", &["undo"]));
    assert_eq!(clean["placements"][0]["outcome"], "clean", "{clean}");
    assert!(clean.get("notice").is_none(), "{clean}");
}
