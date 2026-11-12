//! `fin drop` against a real `finisherd` (5E plan Task 5, spec §8.3). Tests that need the server
//! print `SKIPPED <test>: <reason>` and pass when Node or fin-node is missing.

mod support;

use std::ffi::OsStr;
use std::path::PathBuf;

use serde_json::{Value, json};
use support::{Run, Server, assert_error, fin, fin_env, media_av, ok};

/// A server and a project `film` holding asset a1.
fn project(test: &str) -> Option<(Server, PathBuf)> {
    let server = Server::start(test)?;
    let dir = server.root.join("film");
    ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    media_av(&dir.join("clip.mp4"), 10);
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
    /// Inserts a 1 s clip at the end of `track` on branch `side`; returns its txId.
    fn insert(&self, track: &str) -> String {
        let doc = self.run(&[
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
            "--branch",
            "side",
        ]);
        doc["txId"].as_str().unwrap().to_string()
    }
    fn clips(&self) -> Vec<String> {
        support::view_items(&self.run(&["view"]))
            .into_iter()
            .map(|(id, _, _)| id)
            .collect()
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

#[test]
fn cli_drop_and_undo() {
    let (server, dir) = setup!("cli_drop_and_undo");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "side"]);
    let (a, b) = (t.insert("t1"), t.insert("t2"));

    // A dry run plans and keeps nothing.
    let dry = t.run(&["--dry-run", "drop", &a, "--from", "side"]);
    assert_eq!(dry["drop"]["dryRun"], true, "{dry}");

    // Drop a: a drop record on main (the current branch), and a plain merge leaves a out.
    let dropped = t.run(&["--idem-key", "d1", "drop", &a, "--from", "side"]);
    assert_eq!(dropped["drop"]["dropped"], json!([a]), "{dropped}");
    assert_eq!(dropped["drop"]["from"], "side");
    assert_eq!(dropped["drop"]["into"], "main");
    assert_eq!(dropped["drop"]["txId"], "d1");
    assert!(dropped["drop"]["rev"].is_u64(), "{dropped}");
    // The same key answers the same record; another request under it is refused.
    let again = t.run(&["--idem-key", "d1", "drop", &a, "--from", "side"]);
    assert_eq!(again["drop"]["rev"], dropped["drop"]["rev"], "{again}");
    assert_error(
        &t.raw(&["--idem-key", "d1", "drop", &b, "--from", "side"]),
        3,
        "IDEM_KEY_REUSED",
    );

    let m = t.run(&["merge", "side", "--skip-conflicting"]);
    assert_eq!(m["merge"]["merged"], json!([b]), "{m}");
    let excluded = m["merge"]["excluded"].as_array().unwrap();
    assert!(
        excluded
            .iter()
            .any(|e| e["txId"] == a.as_str() && e["reason"] == "DROPPED"),
        "{m}"
    );
    assert_eq!(t.clips().len(), 1);

    // Undo brings it back.
    let undone = t.run(&["drop", "--undo", &a, "--from", "side"]);
    assert_eq!(undone["drop"]["undropped"], json!([a]), "{undone}");
    let m = t.run(&["merge", "side"]);
    assert_eq!(m["merge"]["merged"], json!([a]), "{m}");
    assert_eq!(t.clips().len(), 2);
}

#[test]
fn cli_drop_refusals() {
    let (server, dir) = setup!("cli_drop_refusals");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "side"]);
    let a = t.insert("t1");
    let b = t.insert("t2");
    t.run(&["merge", "side", "--include", &b]);
    // --from is required; at least one txId.
    assert_error(&t.raw(&["drop", &a]), 2, "USAGE");
    assert_error(&t.raw(&["drop", "--from", "side"]), 2, "USAGE");
    // Only changes the target does not hold yet can be dropped.
    let unknown = t.raw(&["drop", "no-such-tx", "--from", "side"]);
    assert_error(&unknown, 3, "INVALID_OP");
    assert_eq!(unknown.json["error"]["details"]["reason"], "notOnSource");
    let merged = t.raw(&["drop", &b, "--from", "side"]);
    assert_error(&merged, 3, "INVALID_OP");
    assert_eq!(merged.json["error"]["details"]["reason"], "alreadyPresent");
    // Only a drop in force can be lifted.
    let not = t.raw(&["drop", "--undo", &a, "--from", "side"]);
    assert_error(&not, 3, "INVALID_OP");
    assert_eq!(not.json["error"]["details"]["reason"], "notDropped");
    // Nothing was written by any of them: a merge still copies a.
    let m = t.run(&["merge", "side"]);
    assert_eq!(m["merge"]["merged"], json!([a]), "{m}");
}
