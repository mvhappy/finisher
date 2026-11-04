//! `fin` without a server: the map, schemas, the output contract and dry runs against the
//! `FIN_OFFLINE_SNAPSHOT` hook.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

/// Rate 30; asset a1 (60 s, video); t1 holds c1 [0, 5) and c2 [5, 10); t2 (audio) is empty.
fn snapshot() -> Value {
    let clip = |id: &str, start: &str| json!({ "id": id, "asset": "a1", "start": start, "sourceIn": "0", "duration": "5" });
    json!({
        "rev": 7,
        "project": {
            "id": "p1",
            "name": "Demo",
            "schemaVersion": 1,
            "rate": "30",
            "timeline": {
                "id": "tl1",
                "tracks": [
                    { "id": "t1", "kind": "video", "items": [clip("c1", "0"), clip("c2", "5")] },
                    { "id": "t2", "kind": "audio", "items": [] },
                ],
            },
            "assets": {
                "a1": {
                    "id": "a1",
                    "path": "/media/a.mp4",
                    "duration": "60",
                    "video": { "width": 1920, "height": 1080, "rate": "30" },
                    "audio": null,
                    "fingerprint": "f",
                },
            },
        },
    })
}

struct Run {
    code: i32,
    json: Value,
    stdout: String,
    stderr: String,
}

struct Env {
    dir: TempDir,
    snapshot: bool,
}

impl Env {
    fn offline() -> Env {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("snapshot.json"), snapshot().to_string()).unwrap();
        Env {
            dir,
            snapshot: true,
        }
    }

    fn no_server() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
            snapshot: false,
        }
    }

    fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_fin"));
        // Never reach a real finisherd: point server data at an empty folder.
        cmd.args(args)
            .current_dir(self.dir.path())
            .env_remove("FIN_OFFLINE_SNAPSHOT")
            .env_remove("FIN_SERVER")
            .env("FIN_SERVER_DATA", self.dir.path().join("no-server"));
        if self.snapshot {
            cmd.env(
                "FIN_OFFLINE_SNAPSHOT",
                self.dir.path().join("snapshot.json"),
            );
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out: Output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            !stdout.contains('\u{1b}') && !stderr.contains('\u{1b}'),
            "ANSI escape in output of {args:?}"
        );
        assert_eq!(
            stdout.lines().count(),
            1,
            "exactly one JSON document for {args:?}: {stdout}"
        );
        let json =
            serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"));
        let code = out.status.code().unwrap();
        if code == 0 {
            assert!(
                stderr.is_empty(),
                "stderr on success for {args:?}: {stderr}"
            );
        }
        Run {
            code,
            json,
            stdout,
            stderr,
        }
    }

    fn run(&self, args: &[&str]) -> Run {
        self.run_with_stdin(args, "")
    }
}

fn assert_error(run: &Run, code: i32, error: &str) {
    assert_eq!(run.code, code, "{}", run.stdout);
    assert_eq!(run.json["error"]["code"], error, "{}", run.stdout);
    assert!(
        run.json["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty())
    );
    assert!(
        run.json["error"]["fix"]
            .as_str()
            .is_some_and(|f| !f.is_empty()),
        "no fix: {}",
        run.stdout
    );
}

#[test]
fn map_is_small_and_lists_every_group() {
    let env = Env::no_server();
    let run = env.run(&[]);
    assert_eq!(run.code, 0);
    assert!(
        run.stdout.len() / 4 < 1500,
        "map is {} tokens",
        run.stdout.len() / 4
    );
    let groups: Vec<&str> = run.json["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g.as_str().unwrap())
        .collect();
    for group in ["project", "read", "asset", "clip", "batch", "meta"] {
        assert!(groups.contains(&group), "{group} missing");
        let help = env.run(&["help", group]);
        assert_eq!(help.code, 0, "{}", help.stdout);
        assert_eq!(help.json["group"], group);
        for c in help.json["commands"].as_array().unwrap() {
            let usage = c["usage"].as_str().unwrap();
            let listed = run.json["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l.as_str().unwrap().starts_with(usage));
            assert!(listed, "map lacks {usage}");
        }
    }
    assert_eq!(env.run(&["help"]).json, run.json);
    assert_error(&env.run(&["help", "nope"]), 2, "USAGE");
}

#[test]
fn schema_clip_insert_is_json_schema() {
    let env = Env::no_server();
    let run = env.run(&["schema", "clip.insert"]);
    assert_eq!(run.code, 0);
    let s = &run.json;
    assert_eq!(s["$schema"], "https://json-schema.org/draft/2020-12/schema");
    assert_eq!(s["type"], "object");
    assert_eq!(s["additionalProperties"], false);
    for p in ["track", "at", "asset", "in", "dur", "out"] {
        assert!(
            s["properties"][p]["type"].is_string() || s["properties"][p]["type"].is_array(),
            "{p}"
        );
    }
    assert_eq!(s["required"], json!(["track", "at", "asset"]));
    assert_eq!(
        s["oneOf"],
        json!([{ "required": ["dur"] }, { "required": ["out"] }])
    );
    assert_eq!(env.run(&["schema", "clip", "insert"]).json, run.json);
    assert_error(&env.run(&["schema", "clip.nope"]), 2, "USAGE");
}

#[test]
fn insert_echoes_resolved_rationals() {
    let env = Env::offline();
    let run = env.run(&[
        "clip", "insert", "--track", "t1", "--at", "12.4s", "--asset", "a1", "--in", "1s", "--out",
        "f75",
    ]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let r = &run.json;
    assert_eq!(r["dryRun"], true);
    assert!(r.get("rev").is_none());
    assert_eq!(r["resolved"]["at"], "62/5");
    assert_eq!(r["resolved"]["in"], "1");
    assert_eq!(r["resolved"]["duration"], "3/2");
    assert_eq!(r["resolved"]["out"], "5/2");
    assert_eq!(r["bindings"], json!({ "$clip": "c3" }));
    assert_eq!(
        r["diff"]["changes"],
        json!([{ "kind": "clip", "id": "c3", "change": "added" }])
    );
    assert_eq!(r["txId"].as_str().unwrap().len(), 26, "ULID txId");
}

#[test]
fn insert_resolves_end_and_after() {
    let env = Env::offline();
    let end = env.run(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s",
    ]);
    assert_eq!(end.json["resolved"]["at"], "10", "{}", end.stdout);
    let after = env.run(&[
        "clip", "insert", "--track", "t1", "--at", "after:c1", "--asset", "a1", "--dur", "f15",
    ]);
    assert_eq!(after.json["resolved"]["at"], "5", "{}", after.stdout);
    assert_eq!(
        after.json["diff"]["shifts"],
        json!([{ "track": "t1", "from": "5", "by": "1/2" }])
    );
    assert_error(
        &env.run(&[
            "clip", "insert", "--track", "t1", "--at", "after:c9", "--asset", "a1", "--dur", "1s",
        ]),
        3,
        "NOT_FOUND",
    );
}

#[test]
fn insert_inside_a_clip_is_rejected_with_a_fix() {
    let env = Env::offline();
    let run = env.run(&[
        "--dry-run",
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "2s",
        "--asset",
        "a1",
        "--dur",
        "1s",
    ]);
    assert_error(&run, 3, "INSERT_INSIDE_CLIP");
}

#[test]
fn idem_key_becomes_tx_id() {
    let env = Env::offline();
    let run = env.run(&["clip", "delete", "c1", "--idem-key", "k-1"]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert_eq!(run.json["txId"], "k-1");
    assert_eq!(run.json["resolved"], json!({ "clip": "c1" }));
    assert_eq!(
        run.json["diff"]["shifts"],
        json!([{ "track": "t1", "from": "5", "by": "-5" }])
    );
}

#[test]
fn trim_takes_signed_deltas() {
    let env = Env::offline();
    let run = env.run(&["clip", "trim", "c1", "--start", "+0.5s"]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert_eq!(
        run.json["resolved"],
        json!({ "clip": "c1", "edge": "start", "delta": "1/2" })
    );
    let run = env.run(&["clip", "trim", "c2", "--end", "-1s"]);
    assert_eq!(run.json["resolved"]["delta"], "-1", "{}", run.stdout);
    assert_error(
        &env.run(&["clip", "trim", "c2", "--end", "1s"]),
        3,
        "INVALID_TIME",
    );
    assert_error(
        &env.run(&["clip", "trim", "c2", "--end", "-6s"]),
        3,
        "INVALID_RANGE",
    );
}

#[test]
fn usage_errors_exit_two() {
    let env = Env::offline();
    assert_error(&env.run(&["clip", "insert", "--bogus"]), 2, "USAGE");
    assert_error(&env.run(&["--help"]), 2, "USAGE");
    assert_error(&env.run(&["clip", "delete", "--help"]), 2, "USAGE");
    assert_error(&env.run(&["clip"]), 2, "USAGE");
    let both = env.run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "1s", "--out",
        "2s",
    ]);
    assert_error(&both, 2, "USAGE");
    assert_error(&env.run(&["clip", "trim", "c1"]), 2, "USAGE");
}

#[test]
fn reads_come_from_the_snapshot() {
    let env = Env::offline();
    let status = env.run(&["status"]);
    assert_eq!(status.code, 0);
    assert_eq!(status.json["rev"], 7);
    assert_eq!(
        status.json["tracks"][0],
        json!({ "id": "t1", "kind": "video", "clips": 2, "end": "10" })
    );
    assert_eq!(status.json["assets"], json!(["a1"]));
    let view = env.run(&["view", "--track", "t1", "--range", "6s.."]);
    assert_eq!(view.code, 0, "{}", view.stdout);
    assert_eq!(
        view.json["tracks"],
        json!([{ "id": "t1", "kind": "video", "clips": [
            { "id": "c2", "start": "5", "end": "10", "asset": "a1", "sourceIn": "0" }
        ] }])
    );
    assert_eq!(
        view.json["resolved"]["range"],
        json!({ "from": "6", "to": null })
    );
    assert_error(&env.run(&["view", "--track", "t9"]), 3, "NOT_FOUND");
}

#[test]
fn batch_is_one_transaction_with_numbered_placeholders() {
    let env = Env::offline();
    let jsonl = [
        r#"{"cmd":"clip.insert","args":{"track":"t1","at":"end","asset":"a1","dur":"2s"}}"#,
        "",
        r#"{"cmd":"clip.insert","args":{"track":"t1","at":"after:$clip1","asset":"a1","dur":"1s"}}"#,
        r#"{"cmd":"clip.trim","args":{"clip":"$clip3","end":"+1s"}}"#,
    ]
    .join("\n");
    let run = env.run_with_stdin(&["batch"], &jsonl);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert_eq!(
        run.json["bindings"],
        json!({ "$clip1": "c3", "$clip3": "c4" })
    );
    assert_eq!(run.json["resolved"][1]["resolved"]["at"], "12");
    assert_eq!(run.json["resolved"][2]["line"], 4);
    let bad = env.run_with_stdin(&["batch"], r#"{"cmd":"clip.delete","args":{"clip":"c9"}}"#);
    assert_error(&bad, 3, "NOT_FOUND");
    assert_eq!(bad.json["error"]["details"]["line"], 1);
    assert_error(
        &env.run_with_stdin(
            &["batch"],
            r#"{"cmd":"clip.delete","args":{"clip":"c1","x":1}}"#,
        ),
        2,
        "USAGE",
    );
    assert_error(&env.run_with_stdin(&["batch"], ""), 2, "USAGE");
}

#[test]
fn without_a_server_online_commands_are_unavailable() {
    let env = Env::no_server();
    for args in [
        &["status", "--project", "."][..],
        &["view", "--project", "."],
        &["clip", "delete", "c1", "--project", "."],
        &["clip", "delete", "c1", "--dry-run", "--project", "."],
        &["project", "open", "."],
        &["project", "create", "x", "--name", "X"],
    ] {
        let run = env.run(args);
        assert_error(&run, 5, "SERVER_UNAVAILABLE");
        assert_eq!(run.json["error"]["fix"], "start finisherd");
    }
    // Without --project and no finisher.project.json up the tree there is nothing to open.
    for args in [
        &["status"][..],
        &["view"],
        &["clip", "delete", "c1", "--dry-run"],
    ] {
        assert_error(&env.run(args), 3, "PROJECT_NOT_FOUND");
    }
    let offline = Env::offline();
    assert_error(
        &offline.run(&["asset", "add", "a.mp4"]),
        5,
        "SERVER_UNAVAILABLE",
    );
}

#[test]
fn project_create_dry_run_validates_locally() {
    let env = Env::no_server();
    let run = env.run(&[
        "project",
        "create",
        "film",
        "--name",
        "Film",
        "--rate",
        "30000/1001",
        "--dry-run",
    ]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert_eq!(run.json["dryRun"], true);
    assert_eq!(run.json["resolved"]["rate"], "30000/1001");
    assert!(
        run.json["resolved"]["dir"]
            .as_str()
            .unwrap()
            .ends_with("film")
    );
    assert_eq!(run.json["project"]["timeline"]["tracks"][1]["id"], "t2");
    assert_error(
        &env.run(&[
            "project",
            "create",
            "f",
            "--name",
            "F",
            "--rate",
            "0",
            "--dry-run",
        ]),
        3,
        "INVALID_RANGE",
    );
    assert!(
        env.run(&[
            "project",
            "create",
            "f",
            "--name",
            "F",
            "--rate",
            "x",
            "--dry-run"
        ])
        .stderr
        .is_empty()
    );
}
