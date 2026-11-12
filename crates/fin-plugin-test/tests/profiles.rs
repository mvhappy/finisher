//! The `command` profile on real processes: both example plugins pass it; a deliberately
//! broken one (`tests/fixtures/broken`: an undeclared permission, a slow command, a crash, a
//! leaky log) fails with the expected findings.
//!
//! The Node plugins run on `node` from `PATH` (or `FIN_PLUGIN_TEST_NODE`); the Rust example's
//! binary is the workspace's build of `example-rust-tools` (built here when this test runs
//! alone).

use std::path::{Path, PathBuf};
use std::process::Command;

use fin_plugin_test::fake_https::PAGE_TITLE;
use fin_plugin_test::{Finding, Options, Profile, Report};
use serde_json::json;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

fn examples(name: &str) -> PathBuf {
    Path::new(ROOT).join("examples/plugins").join(name)
}

fn node() -> Option<PathBuf> {
    std::env::var_os("FIN_PLUGIN_TEST_NODE").map(PathBuf::from)
}

/// The TypeScript example with its bundle built (`dist/` is git-ignored).
fn ts_tools() -> PathBuf {
    static BUILT: std::sync::Once = std::sync::Once::new();
    let dir = examples("ts-tools");
    BUILT.call_once(|| {
        let node = node().unwrap_or_else(|| PathBuf::from("node"));
        let status = Command::new(node)
            .arg(dir.join("build.mjs"))
            .status()
            .unwrap();
        assert!(status.success(), "build the ts-tools bundle");
    });
    dir
}

/// `target/<profile>/rust-tools`, next to this test's `deps/` folder.
fn rust_tools() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap();
    let bin = dir.join(format!("rust-tools{}", std::env::consts::EXE_SUFFIX));
    if !bin.is_file() {
        let mut cargo = Command::new(env!("CARGO"));
        cargo.args(["build", "-p", "example-rust-tools"]);
        if !cfg!(debug_assertions) {
            cargo.arg("--release");
        }
        assert!(
            cargo.status().unwrap().success(),
            "build example-rust-tools"
        );
    }
    bin
}

fn run(dir: PathBuf, exec: Option<PathBuf>) -> Report {
    let opts = Options {
        exec,
        node: node(),
        ..Options::new(dir)
    };
    fin_plugin_test::run(&opts).unwrap_or_else(|e| panic!("{e}: {}", json!(e.details)))
}

fn assert_passes(r: &Report) {
    assert!(
        r.ok,
        "{}",
        serde_json::to_string_pretty(&r.findings).unwrap()
    );
    for c in &r.commands {
        assert!(c.outside_args_refused, "{}", c.name);
        let warm = c.warm.as_ref().unwrap();
        assert!(
            matches!(warm.outcome, "tx" | "data"),
            "{}: {warm:?}",
            c.name
        );
        let cold = c.cold.as_ref().unwrap();
        assert_eq!(cold.outcome, warm.outcome, "{}", c.name);
        // Both SDKs make a host call first, so the cancel lands on it and ends the job.
        let cancel = c.cancel.as_ref().unwrap();
        assert_eq!(
            cancel.code.as_deref(),
            Some("JOB_CANCELLED"),
            "{}: {cancel:?}",
            c.name
        );
        assert!(cancel.after_cancel_ms.unwrap() < 2000);
    }
    let methods: Vec<&str> = r.calls.iter().map(|c| c.method.as_str()).collect();
    for m in ["project.snapshot", "net.fetch", "net.read", "log"] {
        assert!(methods.contains(&m), "{m} in {methods:?}");
    }
    assert!(
        r.calls.iter().any(|c| c.held),
        "the cancelled calls are held"
    );
}

#[test]
fn rust_example_passes() {
    let r = run(examples("rust-tools"), Some(rust_tools()));
    assert_passes(&r);
    assert_eq!(
        (r.plugin.as_str(), r.runtime),
        ("@example/rust-tools@0.1.0", "native")
    );
    let trim = &r.commands[0];
    assert_eq!(trim.name, "trim-silence");
    assert_eq!(trim.warm.as_ref().unwrap().ops, Some(4));
    // `path` is optional: the sample is `{}` (the command fetches `/`).
    assert_eq!(r.commands[1].args, json!({}));
}

#[test]
fn ts_example_passes() {
    let r = run(ts_tools(), None);
    assert_passes(&r);
    assert_eq!(r.runtime, "node");
    // The required `url` comes from its pattern's literal prefix.
    assert_eq!(
        r.commands[1].args,
        json!({ "url": "https://api.example.org/" })
    );
    assert_eq!(r.commands[0].warm.as_ref().unwrap().ops, Some(1));
}

fn has(
    findings: &[Finding],
    check: &str,
    code: &str,
    command: Option<&str>,
    phase: Option<&str>,
) -> bool {
    findings.iter().any(|f| {
        f.check == check
            && f.code == code
            && f.command.as_deref() == command
            && (phase.is_none() || f.phase == phase)
    })
}

#[test]
fn broken_plugin_fails_with_its_findings() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/broken");
    let r = run(dir, None);
    assert!(!r.ok);
    let f = &r.findings;
    let shown = serde_json::to_string_pretty(f).unwrap();
    // The undeclared permission: refused, and a finding.
    assert!(
        has(
            f,
            "permission",
            "PLUGIN_PERMISSION_DENIED",
            Some("sneaky"),
            Some("warm")
        ),
        "{shown}"
    );
    let denied = f
        .iter()
        .find(|x| x.code == "PLUGIN_PERMISSION_DENIED")
        .unwrap();
    assert_eq!(
        denied.details,
        json!({ "perm": "project:read", "missing": "grant" })
    );
    // The slow command: past its timeout, and deaf to the cancel.
    assert!(
        has(f, "answer", "PLUGIN_TIMEOUT", Some("slow"), Some("warm")),
        "{shown}"
    );
    assert!(
        has(f, "cancel", "PLUGIN_TIMEOUT", Some("slow"), Some("cancel")),
        "{shown}"
    );
    // The crash, reported as PLUGIN_CRASHED with its exit code; the next command still ran.
    assert!(
        has(f, "answer", "PLUGIN_CRASHED", Some("crash"), Some("warm")),
        "{shown}"
    );
    let crash = f.iter().find(|x| x.code == "PLUGIN_CRASHED").unwrap();
    assert_eq!(crash.details, json!({ "exitCode": 3 }));
    assert!(r.commands[3].warm.is_some(), "leaky ran after the crash");
    // The leaky log: the fake API's secret cookie and a path.
    let leaks: Vec<_> = f
        .iter()
        .filter(|x| {
            x.code == "LOG_LEAK" && x.command.as_deref() == Some("leaky") && x.phase == Some("warm")
        })
        .map(|x| x.details["reason"].clone())
        .collect();
    assert!(
        leaks.contains(&json!("secret")) && leaks.contains(&json!("path")),
        "{shown}"
    );
    // The secret itself is in no finding.
    assert!(!shown.contains("fin-test-secret-"));
    // Every command's out-of-schema arguments stayed with the host.
    assert!(r.commands.iter().all(|c| c.outside_args_refused));
    // The fetch went through the broker to the fake API.
    assert!(
        r.calls
            .iter()
            .any(|c| c.command == "leaky" && c.method == "net.fetch" && c.code.is_none())
    );
    // Nothing that failed warm runs cold (`leaky` answered: its leaks are the logs').
    let cold: Vec<_> = r
        .commands
        .iter()
        .filter(|c| c.cold.is_some())
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(cold, ["leaky"]);
}

#[test]
fn fixture_file_and_refusals() {
    // A fixture file is served in place of the built-in one.
    let tmp = tempfile::tempdir().unwrap();
    let fixture = tmp.path().join("snapshot.json");
    let mut p = fin_plugin_test::fixtures::Snapshot::builtin().project;
    p.timelines[0].tracks[1].items.clear();
    std::fs::write(&fixture, json!({ "rev": 7, "project": p }).to_string()).unwrap();
    let opts = Options {
        fixture: Some(fixture),
        exec: Some(rust_tools()),
        ..Options::new(examples("rust-tools"))
    };
    let r = fin_plugin_test::run(&opts).unwrap();
    assert!(r.ok, "{:?}", r.findings);
    assert_eq!(r.fixture, json!({ "source": "file", "rev": 7 }));
    assert_eq!(r.commands[0].warm.as_ref().unwrap().ops, Some(3));

    // A source tree without its build output names what to do.
    let e = fin_plugin_test::run(&Options::new(examples("rust-tools"))).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("PLUGIN_RUNTIME_UNAVAILABLE", &json!("noExecutable"))
    );
    assert!(e.fix.is_some());

    // Later batches' profiles.
    let e = Profile::parse("analyzer").unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["availableFrom"]),
        ("USAGE", &json!("8C"))
    );
    assert_eq!(Profile::parse("command"), Ok(Profile::Command));
    assert!(PAGE_TITLE.contains("Finisher"));
}
