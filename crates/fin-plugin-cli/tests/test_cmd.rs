//! `fin-plugin test`: the report on stdout for a pass, `PLUGIN_TEST_FAILED` with the findings
//! on stderr (exit 3) for a failed profile, and the usual codes when nothing could run.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// Runs `fin-plugin test <args>`: (exit code, stdout, stderr).
fn test(args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_fin-plugin"));
    cmd.arg("test").args(args);
    if let Some(node) = std::env::var_os("FIN_PLUGIN_TEST_NODE") {
        cmd.arg("--node").arg(node);
    }
    let o = cmd.output().unwrap();
    (
        o.status.code().unwrap(),
        String::from_utf8(o.stdout).unwrap(),
        String::from_utf8(o.stderr).unwrap(),
    )
}

fn path(rel: &str) -> String {
    Path::new(ROOT).join(rel).display().to_string()
}

/// The TypeScript example with its bundle built (`dist/` is git-ignored).
fn ts_tools() -> String {
    let dir = path("examples/plugins/ts-tools");
    let node = std::env::var_os("FIN_PLUGIN_TEST_NODE").unwrap_or_else(|| "node".into());
    let status = Command::new(node)
        .arg(Path::new(&dir).join("build.mjs"))
        .status()
        .unwrap();
    assert!(status.success(), "build the ts-tools bundle");
    dir
}

#[test]
fn a_passing_plugin_prints_its_report() {
    let (code, out, err) = test(&[&ts_tools(), "--profile", "command"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.is_empty(), "{err}");
    let r: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(r["profile"], "command");
    assert_eq!(r["plugin"], "@example/ts-tools@0.1.0");
    assert_eq!(r["findings"], serde_json::json!([]));
    assert_eq!(r["commands"].as_array().unwrap().len(), 2);
}

#[test]
fn a_failing_plugin_is_plugin_test_failed() {
    let broken = path("crates/fin-plugin-test/tests/fixtures/broken");
    let (code, out, err) = test(&[&broken]);
    assert_eq!(code, 3, "{err}");
    assert!(out.is_empty());
    let e: Value = serde_json::from_str(&err).unwrap();
    assert_eq!(e["code"], "PLUGIN_TEST_FAILED");
    assert!(e["fix"].is_string());
    let codes: Vec<&str> = e["details"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["code"].as_str())
        .collect();
    for c in [
        "PLUGIN_PERMISSION_DENIED",
        "PLUGIN_TIMEOUT",
        "PLUGIN_CRASHED",
        "LOG_LEAK",
    ] {
        assert!(codes.contains(&c), "{c} in {codes:?}");
    }
    assert_eq!(e["details"]["report"]["ok"], false);
}

#[test]
fn nothing_to_run_is_an_error_with_its_code() {
    // A source tree without its native build output: unavailable (5), with a fix.
    let (code, _, err) = test(&[&path("examples/plugins/rust-tools")]);
    assert_eq!(code, 5, "{err}");
    let e: Value = serde_json::from_str(&err).unwrap();
    assert_eq!(e["code"], "PLUGIN_RUNTIME_UNAVAILABLE");
    assert!(e["fix"].as_str().unwrap().contains("--exec"));
    // A later batch's profile, and a missing folder.
    let (code, _, err) = test(&[&path("examples/plugins/ts-tools"), "--profile", "analyzer"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("8C"));
    let (code, _, err) = test(&[&path("examples/plugins/no-such-plugin")]);
    assert_eq!(code, 5, "{err}");
    assert!(err.contains("IO_ERROR"));
}
