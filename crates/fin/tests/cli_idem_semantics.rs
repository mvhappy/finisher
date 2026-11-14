//! `fin` command semantics around `--idem-key`, the default branch and `batch` lines (review
//! batch E: #25, #26, #27, #31, #34, #44), against a real `finisherd`. Tests that need the
//! server print `SKIPPED <test>: <reason>` and pass when Node or fin-node is missing.

mod support;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use support::{Run, Server, assert_error, fin, ok};

/// A server and a project `film` (rev 0).
fn project(test: &str) -> Option<(Server, PathBuf)> {
    let server = Server::start(test)?;
    ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = server.root.join("film");
    Some((server, dir))
}

macro_rules! setup {
    ($name:literal) => {{
        let Some((server, dir)) = project($name) else {
            return;
        };
        (server, dir)
    }};
}

struct T<'a>(&'a Server, &'a PathBuf);

impl T<'_> {
    fn raw(&self, args: &[&str]) -> Run {
        fin(&self.0.sd, self.1, args)
    }
    fn run(&self, args: &[&str]) -> Value {
        ok(self.raw(args))
    }
    fn batch(&self, lines: &str) -> Run {
        use std::io::Write as _;
        let mut cmd = support::fin_cmd(&self.0.sd, self.1, &["batch"]);
        cmd.stdin(std::process::Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(lines.as_bytes())
            .unwrap();
        support::finish(&["batch"], child.wait_with_output().unwrap())
    }
    /// The head rev of `branch`.
    fn rev(&self, branch: &str) -> Value {
        self.run(&["--branch", branch, "status"])["rev"].clone()
    }
    fn config_file(&self) -> PathBuf {
        self.0.sd.join("fin-config.json")
    }
    fn config(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.config_file()).unwrap()).unwrap()
    }
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

/// #25: a `defaultBranch` section or entry of the wrong type is a broken file (spec: a broken
/// file fails loudly), never a silent `main`.
#[test]
fn malformed_default_branch_fails_closed() {
    let (server, dir) = setup!("malformed_default_branch_fails_closed");
    let t = T(&server, &dir);
    let file = t.config_file();
    let project: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("finisher.project.json")).unwrap())
            .unwrap();
    let project_id = project["id"].as_str().unwrap();
    for bad in [
        r#"{"defaultBranch":42}"#,
        r#"{"defaultBranch":{"PID":false}}"#,
        r#"{"defaultBranch":{"PID":""}}"#,
        r#"{"tries":[]}"#,
        r#"{"tries":{"PID":{"br1":7}}}"#,
    ] {
        write(&file, &bad.replace("PID", project_id));
        let run = t.raw(&["marker", "add", "--at", "0", "--label", "x"]);
        assert_error(&run, 6, "INTERNAL");
        assert!(
            run.json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("fin-config.json"),
            "{bad}: {}",
            run.stdout
        );
        assert_error(&t.raw(&["branch", "default"]), 6, "INTERNAL");
        assert_error(&t.raw(&["branch", "default", "--clear"]), 6, "INTERNAL");
    }
    std::fs::remove_file(&file).unwrap();
    assert_eq!(t.rev("main"), json!(0), "nothing was written to main");
}

/// #26: the same `--idem-key` on another explicit `--branch` is another command.
#[test]
fn explicit_branch_is_part_of_the_idempotency_identity() {
    let (server, dir) = setup!("explicit_branch_is_part_of_the_idempotency_identity");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "side"]);
    let add = |b: &str| {
        t.raw(&[
            "--idem-key",
            "same-marker",
            "--branch",
            b,
            "marker",
            "add",
            "--at",
            "0",
            "--label",
            "x",
        ])
    };
    let first = ok(add("main"));
    assert_eq!(ok(add("main"))["txId"], first["txId"], "a retry answers");
    assert_error(&add("br1"), 3, "IDEM_KEY_REUSED");
    assert_eq!(t.rev("br1"), json!(0));
}

/// #27: `fin try` and `fin try drop|keep` retried with their key answer the first attempt,
/// although the first one moved the default branch.
#[test]
fn try_retries_answer_the_first_attempt() {
    let (server, dir) = setup!("try_retries_answer_the_first_attempt");
    let t = T(&server, &dir);
    let first = t.run(&["--idem-key", "same-try", "try"]);
    assert_eq!(first["branch"]["id"], "br1", "{first}");
    let again = t.run(&["--idem-key", "same-try", "try"]);
    assert_eq!(again["branch"]["id"], "br1", "{again}");
    assert_eq!(again["txId"], first["txId"]);
    assert_eq!(t.run(&["branch", "default"])["default"], "br1");
    assert_eq!(
        t.run(&["branch", "ls", "--all"])["branches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let dropped = t.run(&["--idem-key", "same-drop", "try", "drop"]);
    assert_eq!(dropped["switchedTo"], "main", "{dropped}");
    let again = t.run(&["--idem-key", "same-drop", "try", "drop"]);
    assert_eq!(again["txId"], dropped["txId"], "{again}");
    assert_eq!(again["switchedTo"], "main");
    assert_eq!(again["branch"]["id"], "br1");
    assert_eq!(t.run(&["branch", "default"])["default"], Value::Null);
    // The same key with an explicit --branch naming another branch is another command; one
    // naming the try (an archived name, as typed) is the same.
    t.run(&["branch", "new", "other"]);
    assert_error(
        &t.raw(&[
            "--idem-key",
            "same-drop",
            "--branch",
            "other",
            "try",
            "drop",
        ]),
        3,
        "IDEM_KEY_REUSED",
    );
    let named = t.run(&["--idem-key", "same-drop", "--branch", "br1", "try", "drop"]);
    assert_eq!(named["txId"], dropped["txId"], "{named}");
    // A retry of the first `try` now leaves the default alone: its branch is archived.
    t.run(&["--idem-key", "same-try", "try"]);
    assert_eq!(t.run(&["branch", "default"])["default"], Value::Null);
    // A dry-run retry of a live try answers the commit and leaves the default alone.
    let live = t.run(&["--idem-key", "live", "try", "live"]);
    t.run(&["branch", "default", "--clear"]);
    let dry = t.run(&["--dry-run", "--idem-key", "live", "try", "live"]);
    assert_eq!(dry["txId"], live["txId"], "{dry}");
    assert_eq!(dry.get("dryRun"), None);
    assert_eq!(t.run(&["branch", "default"])["default"], Value::Null);
    t.run(&["--branch", "live", "try", "drop"]);

    t.run(&["--idem-key", "k2", "try", "kept"]);
    t.run(&["marker", "add", "--at", "0", "--label", "x"]);
    let kept = t.run(&["--idem-key", "same-keep", "try", "keep"]);
    assert_eq!(kept["switchedTo"], "main", "{kept}");
    let again = t.run(&["--idem-key", "same-keep", "try", "keep"]);
    assert_eq!(
        again["merge"]["mergeId"], kept["merge"]["mergeId"],
        "{again}"
    );
    assert_eq!(again["switchedTo"], "main");
    assert_eq!(t.rev("main"), kept["merge"]["rev"]);
    assert!(
        t.config()["tries"].as_object().is_none_or(|m| m.is_empty()),
        "{}",
        t.config()
    );
}

/// #27: a key a plain `fin merge` used is another command for `fin try keep`, even though the
/// keep would merge the same branches (the merge names the try by id, as the keep does).
#[test]
fn try_keep_refuses_a_plain_merge_key() {
    let (server, dir) = setup!("try_keep_refuses_a_plain_merge_key");
    let t = T(&server, &dir);
    t.run(&["try", "feat"]);
    t.run(&["marker", "add", "--at", "0", "--label", "x"]);
    let merged = t.run(&["--idem-key", "plain", "merge", "br1", "--into", "main"]);
    assert!(merged["merge"]["mergeId"].is_string(), "{merged}");
    assert_error(
        &t.raw(&["--idem-key", "plain", "try", "keep"]),
        3,
        "IDEM_KEY_REUSED",
    );
    // The try is still the default: nothing was kept or forgotten.
    assert_eq!(t.run(&["branch", "default"])["default"], "br1");
}

/// #31: `--dry-run --idem-key K` for a committed `K` answers that commit (not a new plan), and
/// a dry-run retry of a removal is not `NOT_FOUND`.
#[test]
fn dry_run_with_a_committed_key_answers_the_commit() {
    let (server, dir) = setup!("dry_run_with_a_committed_key_answers_the_commit");
    let t = T(&server, &dir);
    let add = [
        "--idem-key",
        "K",
        "marker",
        "add",
        "--at",
        "0",
        "--label",
        "x",
    ];
    let first = t.run(&add);
    let mut dry = vec!["--dry-run"];
    dry.extend_from_slice(&add);
    let again = t.run(&dry);
    assert_eq!(again.get("dryRun"), None, "{again}");
    assert_eq!(again["rev"], first["rev"]);
    assert_eq!(again["bindings"], first["bindings"]);
    assert_eq!(again["bindings"]["$marker"], "m1");

    let remove = ["--idem-key", "R", "marker", "remove", "m1"];
    let removed = t.run(&remove);
    let mut dry = vec!["--dry-run"];
    dry.extend_from_slice(&remove);
    let again = t.run(&dry);
    assert_eq!(again.get("dryRun"), None, "{again}");
    assert_eq!(again["rev"], removed["rev"]);
    // An unknown key still plans, and still validates.
    let fresh = t.run(&[
        "--dry-run",
        "--idem-key",
        "new",
        "marker",
        "add",
        "--at",
        "0",
        "--label",
        "y",
    ]);
    assert_eq!(fresh["dryRun"], true);
    assert_error(
        &t.raw(&["--dry-run", "--idem-key", "new2", "marker", "remove", "m1"]),
        3,
        "NOT_FOUND",
    );
    // Another command under a committed key is still refused.
    assert_error(
        &t.raw(&[
            "--dry-run",
            "--idem-key",
            "K",
            "marker",
            "add",
            "--at",
            "1",
            "--label",
            "z",
        ]),
        3,
        "IDEM_KEY_REUSED",
    );
    assert_eq!(t.rev("main"), removed["rev"]);
}

/// #34: a plain name names the active branch holding it, as `--branch` resolves it.
#[test]
fn branch_default_by_name_skips_archived_branches() {
    let (server, dir) = setup!("branch_default_by_name_skips_archived_branches");
    let t = T(&server, &dir);
    t.run(&["branch", "new", "same"]);
    t.run(&["branch", "archive", "same"]);
    t.run(&["branch", "new", "same"]);
    let set = t.run(&["branch", "default", "same"]);
    assert_eq!(set["default"], "br2", "{set}");
    t.run(&["marker", "add", "--at", "0", "--label", "x"]);
    assert_eq!(t.rev("br2"), t.rev("same"));
    // An archived branch is still named by id or `<name>#<id>`.
    assert_eq!(t.run(&["branch", "default", "same#br1"])["default"], "br1");
    assert_eq!(t.run(&["branch", "default", "SAME"])["default"], "br2");
}

/// #44: a batch line is `{cmd, args}` and nothing else; an unknown field refuses the whole
/// batch before anything is committed.
#[test]
fn batch_lines_reject_unknown_envelope_fields() {
    let (server, dir) = setup!("batch_lines_reject_unknown_envelope_fields");
    let t = T(&server, &dir);
    support::media(&dir.join("clip.mp4"), 10);
    t.run(&["asset", "add", "clip.mp4"]);
    let insert = r#""args":{"track":"t1","at":"0","asset":"a1","dur":"1s"}"#;
    let ok_line = format!(r#"{{"cmd":"clip.insert",{insert}}}"#);
    // The reported case: a lone insert marked `dryRun` committed a real clip.
    let run = t.batch(&format!(
        r#"{{"cmd":"clip.insert",{insert},"dryRun":true}}"#
    ));
    assert_error(&run, 2, "USAGE");
    assert_eq!(run.json["error"]["details"]["field"], "dryRun");
    for bad in [
        r#"{"cmd":"clip.delete","args":{"clip":"c9"},"dryRun":true}"#,
        r#"{"cmd":"clip.delete","arg":{"clip":"c9"}}"#,
        r#"{"cmd":"clip.delete","args":[]}"#,
        r#"["clip.delete"]"#,
    ] {
        let run = t.batch(&format!("{ok_line}\n{bad}\n"));
        assert_error(&run, 2, "USAGE");
        assert_eq!(
            run.json["error"]["details"]["line"], 2,
            "{bad}: {}",
            run.stdout
        );
    }
    assert_eq!(t.rev("main"), json!(1), "only the asset add");
    assert_eq!(ok(t.batch(&ok_line))["rev"], 2);
}

/// #49: a batch line whose mode is not one of the flag's values refuses the whole batch before
/// anything is committed (it used to commit a ripple delete for `"liftt"`).
#[test]
fn batch_line_with_an_unknown_mode_commits_nothing() {
    let (server, dir) = setup!("batch_line_with_an_unknown_mode_commits_nothing");
    let t = T(&server, &dir);
    support::media(&dir.join("clip.mp4"), 10);
    t.run(&["asset", "add", "clip.mp4"]);
    for _ in 0..2 {
        t.run(&[
            "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s",
        ]);
    }
    let before = t.rev("main");
    let run = t.batch(concat!(
        r#"{"cmd":"clip.delete","args":{"clip":"c2","mode":"lift"}}"#,
        "\n",
        r#"{"cmd":"clip.delete","args":{"clip":"c1","mode":"liftt"}}"#,
        "\n",
    ));
    assert_error(&run, 2, "USAGE");
    assert_eq!(run.json["error"]["details"]["line"], 2, "{}", run.stdout);
    assert_eq!(t.rev("main"), before, "nothing committed");
    let run = t.batch(concat!(
        r#"{"cmd":"clip.trim","args":{"clip":"c1","end":"-0.5s","mode":"rol"}}"#,
        "\n"
    ));
    assert_error(&run, 2, "USAGE");
    assert_eq!(t.rev("main"), before, "nothing committed");
}
