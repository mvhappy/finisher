//! Seatbelt (plan 7C Task 8): the S8 file cases of `cases/files.json` under the real kernel,
//! each in a fresh fixture; a denial passes only on an observed `EPERM` or `EACCES`, and every
//! denied case succeeds unsandboxed. A case's `macos` field overrides its expectation where
//! Seatbelt states the model more exactly than Landlock's enumeration can (the
//! reference model of `SandboxPolicy::writable` agrees).
#![cfg(target_os = "macos")]

mod macos_support;

use std::path::Path;

use fin_sandbox::{Limits, Net, Profile, ReadPolicy, SandboxPolicy};
use macos_support::{Outcome, json_file, listener, probe, s, tempdir, write};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    op: Vec<String>,
    expect: String,
    #[serde(default)]
    macos: Option<String>,
    #[serde(default, rename = "macosOp")]
    macos_op: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Cases {
    cases: Vec<Case>,
}

fn cases() -> Vec<Case> {
    let c: Cases = serde_json::from_str(include_str!("cases/files.json")).unwrap();
    c.cases
}

/// The fixture of `cases/files.json` under `t`.
fn fixture(t: &Path) {
    write(&t.join("w/a/f"), "a");
    std::fs::create_dir_all(t.join("w/b")).unwrap();
    write(&t.join("w/prot/secret"), "p");
    write(&t.join("w/denied/secret"), "d");
    std::fs::create_dir_all(t.join("w2")).unwrap();
    write(&t.join("r/f"), "r");
    write(&t.join("r/sub/g"), "g");
    write(&t.join("r/deny/secret"), "rd");
    write(&t.join("out/secret"), "o");
    std::os::unix::fs::symlink(t.join("out/secret"), t.join("w/a/to-out")).unwrap();
}

fn policy(t: &Path) -> SandboxPolicy {
    SandboxPolicy {
        profile: Profile::Worker,
        write: vec![t.join("w"), t.join("w2")],
        read: ReadPolicy::Only(vec![t.join("r")]),
        protect: vec![t.join("w/prot")],
        deny_read: vec![t.join("r/deny"), t.join("w/denied")],
        unix_sockets: vec![],
        net: Net::None,
        mach: vec![],
        devices: vec![],
        limits: Limits {
            processes: Some(1),
            ..Limits::default()
        },
    }
}

fn run(case: &Case, sandboxed: bool, port: u16) -> Outcome {
    let (_d, t) = tempdir();
    fixture(&t);
    let op: Vec<String> = case
        .macos_op
        .as_ref()
        .unwrap_or(&case.op)
        .iter()
        .map(|a| a.replace("$T", &s(&t)).replace("$PORT", &port.to_string()))
        .collect();
    let flags = if sandboxed {
        vec!["--policy".into(), json_file(&t, "policy.json", &policy(&t))]
    } else {
        vec!["--none".into()]
    };
    probe(&flags, &op)
}

#[test]
fn the_s8_file_cases_hold_under_seatbelt() {
    let (_l, port) = listener();
    let mut failures = Vec::new();
    let all = cases();
    for case in &all {
        let expect = case.macos.as_deref().unwrap_or(&case.expect);
        let got = run(case, true, port);
        let ok = match expect {
            "deny" => got.denied(),
            "allow" => got == Outcome::Ran(None),
            other => panic!("{}: unknown expectation {other}", case.name),
        };
        if !ok {
            failures.push(format!("{}: {got:?}", case.name));
        }
        if expect == "deny" {
            // An `exec` that worked replaced the probe, which then exited 0 saying nothing.
            let control = run(case, false, port);
            let replaced = case.op[0] == "exec"
                && control
                    == (Outcome::Other {
                        code: Some(0),
                        stdout: String::new(),
                    });
            if control != Outcome::Ran(None) && !replaced {
                failures.push(format!("{} unsandboxed (control): {control:?}", case.name));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases:\n{}",
        failures.len(),
        all.len(),
        failures.join("\n")
    );
}
