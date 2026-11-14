//! The invocation log (`invocations`: one line per process, any command), at `@store/` (the
//! sign-in store a sandboxed CLI may write: detection's and every session's) or a path, so a
//! test can count CLI processes the host's own runs included (7D Task 16).

mod common;

use std::process::Stdio;

use common::*;
use serde_json::Value;

fn lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn fake_logs_each_invocation_in_the_store() {
    let dir = Dir::new("invocations-store");
    let store = dir.path("store");
    std::fs::create_dir_all(&store).unwrap();
    let store_s = store.to_string_lossy().into_owned();
    for argv in [&["--version"][..], &["auth", "status", "--json"][..]] {
        let status = std::process::Command::new(BIN)
            .args(argv)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", &store_s)
            .env("FAKE_CLAUDE_INVOCATIONS", "@store/invocations.jsonl")
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
    let got = lines(&store.join("invocations.jsonl"));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0]["argv"], serde_json::json!(["--version"]));
    assert_eq!(
        got[1]["argv"],
        serde_json::json!(["auth", "status", "--json"])
    );
    assert!(got[0]["pid"].is_u64());
}

#[test]
fn fake_without_the_store_variable_logs_nothing_there() {
    let dir = Dir::new("invocations-nostore");
    let status = std::process::Command::new(BIN)
        .arg("--version")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("FAKE_CLAUDE_INVOCATIONS", "@store/invocations.jsonl")
        .current_dir(&dir.0)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn fake_session_process_logs_its_invocation_at_a_path() {
    let dir = Dir::new("invocations-path");
    let log = dir.path("inv.jsonl");
    let log_s = log.to_string_lossy().into_owned();
    let f = Fake::start(
        &dir,
        None,
        &["--session-id", UUID],
        &[("FAKE_CLAUDE_INVOCATIONS", &log_s)],
    );
    assert!(f.finish().success());
    let got = lines(&log);
    assert_eq!(got.len(), 1);
    assert!(
        got[0]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "--session-id")
    );
}
