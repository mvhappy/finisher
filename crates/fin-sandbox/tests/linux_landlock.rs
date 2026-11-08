//! Landlock (plan 7C Task 2): the ABI floor, the version query, the S8 file cases of
//! `cases/files.json` under the real kernel at ABI 3, 4 and the kernel's own, and the thread
//! rule. Each case runs `fin-sandbox-probe` in a fresh fixture; a denial passes only on an
//! observed `EACCES`, `EPERM` or `EXDEV`, and every denied case succeeds unsandboxed.
#![cfg(target_os = "linux")]

mod linux_support;

use std::path::Path;

use fin_sandbox::linux::landlock::{Abi, MIN_ABI, NEED, Support, TSYNC_ABI, classify, ruleset};
use fin_sandbox::{ApplyError, Limits, Net, Profile, ReadPolicy, SandboxPolicy};
use linux_support::{Outcome, json_file, landlock_or_skip, listener, probe, s, tempdir, write};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    op: Vec<String>,
    expect: String,
    #[serde(default)]
    min_abi: Option<u32>,
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

/// The ABIs the cases run at: 3, 4 and the kernel's, as far as the kernel has them.
fn abis(kernel: Abi) -> Vec<u32> {
    let mut v: Vec<u32> = [MIN_ABI, 4, kernel.0]
        .into_iter()
        .filter(|a| *a <= kernel.0)
        .collect();
    v.dedup();
    v
}

/// Runs `case` in a fresh fixture, sandboxed at `abi` (`None`: unsandboxed).
fn run(case: &Case, abi: Option<u32>, port: u16) -> Outcome {
    let (_d, t) = tempdir();
    fixture(&t);
    let op: Vec<String> = case
        .op
        .iter()
        .map(|a| a.replace("$T", &s(&t)).replace("$PORT", &port.to_string()))
        .collect();
    let flags = match abi {
        Some(abi) => vec![
            "--abi".into(),
            abi.to_string(),
            "--policy".into(),
            json_file(&t, "policy.json", &policy(&t)),
        ],
        None => vec!["--none".into()],
    };
    probe(&flags, &op)
}

/// Runs the cases whose name passes `pick` at every ABI and returns the failures.
fn check(pick: impl Fn(&str) -> bool) -> Vec<String> {
    let Some(kernel) = landlock_or_skip("linux_landlock") else {
        return vec![];
    };
    let (_l, port) = listener();
    let mut failures = Vec::new();
    let mut ran = 0;
    for case in cases().iter().filter(|c| pick(&c.name)) {
        for abi in abis(kernel) {
            if case.min_abi.is_some_and(|m| abi < m) {
                continue;
            }
            ran += 1;
            let got = run(case, Some(abi), port);
            let ok = match case.expect.as_str() {
                "deny" => got.denied(),
                "allow" => got == Outcome::Ran(None),
                other => panic!("{}: unknown expectation {other}", case.name),
            };
            if !ok {
                failures.push(format!("{} at ABI {abi}: {got:?}", case.name));
            }
        }
    }
    assert!(ran > 0, "no case ran");
    failures
}

#[test]
fn s8_file_cases() {
    let failures = check(|_| true);
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn hard_link_into_write_root_refused() {
    let failures = check(|n| n.starts_with("hard link"));
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn rename_across_roots_refused() {
    let failures = check(|n| n.starts_with("rename"));
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The control: without the sandbox each denied case succeeds, so the denial is the
/// sandbox's and not the fixture's (`exec` replaces the probe and is left out).
#[test]
fn denied_cases_succeed_unsandboxed() {
    let (_l, port) = listener();
    for case in cases()
        .iter()
        .filter(|c| c.expect == "deny" && c.op[0] != "exec")
    {
        assert_eq!(
            run(case, None, port),
            Outcome::Ran(None),
            "{} unsandboxed",
            case.name
        );
    }
}

#[test]
fn abi_below_3_refused() {
    let (_d, t) = tempdir();
    fixture(&t);
    for abi in [1, 2] {
        let err = ruleset(&policy(&t), Abi(abi)).unwrap_err();
        assert_eq!(err, ApplyError::Unavailable { need: NEED.into() });
        assert_eq!(err.to_string(), "the sandbox needs kernel ≥ 6.2");
    }
    // And through the probe: nothing is applied, the start is refused.
    let flags = [
        "--abi".to_string(),
        "2".into(),
        "--policy".into(),
        json_file(&t, "p.json", &policy(&t)),
    ];
    assert_eq!(
        probe(&flags, &["read".into(), s(&t.join("r/f"))]),
        Outcome::Apply("the sandbox needs kernel ≥ 6.2".into())
    );
}

#[test]
fn version_query_classification() {
    // The S9 rule: the version query alone decides; these errnos mean "disabled" (F2).
    for e in [libc::EPERM, libc::ENOSYS, libc::EOPNOTSUPP] {
        assert_eq!(classify(-1, e), Support::Disabled { errno: e });
        assert!(matches!(
            classify(-1, e).abi(),
            Err(ApplyError::Disabled { .. })
        ));
    }
    assert_eq!(classify(4, 0), Support::Abi(Abi(4)));
    assert!(matches!(
        classify(-1, libc::EINVAL).abi(),
        Err(ApplyError::Unavailable { .. })
    ));
    assert!(matches!(classify(0, 0), Support::Unknown { .. }));
}

#[test]
fn a_second_thread_is_refused_below_tsync() {
    let Some(_) = landlock_or_skip("a_second_thread_is_refused_below_tsync") else {
        return;
    };
    let (_d, t) = tempdir();
    fixture(&t);
    let flags = [
        "--abi".to_string(),
        "4".into(),
        "--thread".into(),
        "--policy".into(),
        json_file(&t, "p.json", &policy(&t)),
    ];
    match probe(&flags, &["read".into(), s(&t.join("out/secret"))]) {
        Outcome::Apply(e) => assert!(e.contains("2 threads"), "{e}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn tsync_restricts_every_thread() {
    let Some(kernel) = landlock_or_skip("tsync_restricts_every_thread") else {
        return;
    };
    if kernel.0 < TSYNC_ABI {
        eprintln!(
            "tsync_restricts_every_thread: skipped, ABI {} < {TSYNC_ABI}",
            kernel.0
        );
        return;
    }
    let (_d, t) = tempdir();
    fixture(&t);
    let flags = [
        "--thread".to_string(),
        "--policy".into(),
        json_file(&t, "p.json", &policy(&t)),
    ];
    // The attempt runs on the thread that existed before the restriction.
    assert!(probe(&flags, &["read".into(), s(&t.join("out/secret"))]).denied());
}

#[test]
fn a_root_that_is_a_link_is_refused() {
    let Some(_) = landlock_or_skip("a_root_that_is_a_link_is_refused") else {
        return;
    };
    let (_d, t) = tempdir();
    fixture(&t);
    std::os::unix::fs::symlink(t.join("out"), t.join("link")).unwrap();
    let mut p = policy(&t);
    p.read = ReadPolicy::Only(vec![t.join("link")]);
    let flags = ["--policy".to_string(), json_file(&t, "p.json", &p)];
    match probe(&flags, &["read".into(), s(&t.join("out/secret"))]) {
        Outcome::Apply(e) => assert!(e.contains("is a symbolic link"), "{e}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_root_grants_nothing() {
    let Some(_) = landlock_or_skip("a_missing_root_grants_nothing") else {
        return;
    };
    let (_d, t) = tempdir();
    fixture(&t);
    let mut p = policy(&t);
    p.write.push(t.join("later"));
    let flags = ["--policy".to_string(), json_file(&t, "p.json", &p)];
    // Its parent is not granted, so it cannot be created after the restriction either.
    assert!(probe(&flags, &["mkdir".into(), s(&t.join("later"))]).denied());
}
