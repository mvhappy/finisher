//! The launcher `fin-sandbox-exec` (plan 7C Task 5): the request only through `--policy-fd`,
//! an invalid request refused with `SANDBOX_UNAVAILABLE` (exit 78), and the program's own exit
//! code and signals (the launcher `exec`s it, so they are the program's).
#![cfg(target_os = "linux")]

mod linux_support;

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fin_core::events::payloads::agent::{AgentMode, Harness};
use fin_sandbox::launch::LaunchRequest;
use fin_sandbox::{AgentInput, Os};
use linux_support::{failure, landlock_or_skip, launch_cmd, launch_raw, tempdir, write};

const PATH: (&str, &str) = ("PATH", "/usr/bin:/bin");

/// An agent request for a project and server data under `t`, with the base toolchain.
fn agent(t: &Path) -> LaunchRequest {
    let project = t.join("p");
    let data = t.join("data");
    for d in [
        "p/workspace/s1",
        "data/agents/s1/tmp",
        "data/agents/s1/provider",
    ] {
        std::fs::create_dir_all(t.join(d)).unwrap();
    }
    write(&t.join("data/auth/local.key"), "k");
    LaunchRequest::Agent(AgentInput {
        session_id: "s1".into(),
        mode: AgentMode::Draft,
        harness: Harness::Codex,
        workspace_dir: project.join("workspace").join("s1"),
        shared_dir: None,
        project_dir: project,
        library_roots: vec![],
        session_dir: data.join("agents").join("s1"),
        sign_in_store: None,
        toolchains: fin_sandbox::data::toolchain_base_resolved(Os::Linux),
        server_data: data,
        home: t.join("home"),
        fin: None,
    })
}

fn bytes(r: &LaunchRequest) -> Vec<u8> {
    serde_json::to_vec(r).unwrap()
}

fn key(t: &Path) -> String {
    t.join("data/auth/local.key").to_string_lossy().into_owned()
}

#[test]
fn request_only_through_the_descriptor() {
    let (_d, t) = tempdir();
    let req = bytes(&agent(&t));
    // A policy on argv, a second flag, no descriptor, a standard stream.
    for args in [
        &["--policy", "{}", "--", "true"][..],
        &["--", "true"],
        &["--policy-fd", "0", "--", "true"],
        &["--policy-fd", "3", "--policy-fd", "3", "--", "true"],
        &["--policy-fd", "3"],
    ] {
        let out = launch_raw(Some(&req), args, &[PATH]);
        assert_eq!(out.status.code(), Some(64), "{args:?}");
        assert_eq!(failure(&out)["code"], "USAGE", "{args:?}");
    }
    // A request in the environment, even beside a valid descriptor.
    for var in fin_sandbox::launch::REFUSED_ENV {
        let out = launch_raw(
            Some(&req),
            &["--policy-fd", "3", "--", "true"],
            &[PATH, (var, "{}")],
        );
        assert_eq!(out.status.code(), Some(64), "{var}");
    }
}

#[test]
fn invalid_request_is_sandbox_unavailable() {
    let (_d, t) = tempdir();
    let mut relative = agent(&t);
    if let LaunchRequest::Agent(i) = &mut relative {
        i.home = PathBuf::from("home");
    }
    for (what, req) in [
        ("empty", b"".to_vec()),
        ("not json", b"{".to_vec()),
        (
            "unknown profile",
            br#"{"profile":"worker","input":{}}"#.to_vec(),
        ),
        ("relative path", bytes(&relative)),
    ] {
        let out = launch_raw(Some(&req), &["--policy-fd", "3", "--", "true"], &[PATH]);
        assert_eq!(out.status.code(), Some(78), "{what}");
        let f = failure(&out);
        assert_eq!(f["code"], "SANDBOX_UNAVAILABLE", "{what}");
        assert_eq!(f["os"], "linux", "{what}");
    }
    // A descriptor that is not open.
    let out = launch_raw(None, &["--policy-fd", "9", "--", "true"], &[PATH]);
    assert_eq!(out.status.code(), Some(78));
}

#[test]
fn exit_code_and_signals_are_the_programs() {
    let Some(_) = landlock_or_skip("exit_code_and_signals_are_the_programs") else {
        return;
    };
    let (_d, t) = tempdir();
    let req = bytes(&agent(&t));
    let fd = ["--policy-fd", "3", "--"];
    let run = |argv: &[&str]| launch_raw(Some(&req), &[&fd[..], argv].concat(), &[PATH]);
    assert_eq!(run(&["sh", "-c", "exit 7"]).status.code(), Some(7));
    assert_eq!(
        run(&["sh", "-c", "kill -TERM $$"]).status.signal(),
        Some(libc::SIGTERM)
    );
    assert_eq!(run(&["no-such-program-here"]).status.code(), Some(127));

    // A signal sent to the launcher's process reaches the program: it is the same process.
    let (mut cmd, keep) = launch_cmd(Some(&req), &[&fd[..], &["sleep", "30"]].concat(), &[PATH]);
    let mut child = cmd.spawn().unwrap();
    drop(keep);
    std::thread::sleep(Duration::from_millis(300));
    // SAFETY: signalling our own child.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}

#[test]
fn the_program_runs_confined_and_inherits_no_descriptor() {
    let Some(_) = landlock_or_skip("the_program_runs_confined_and_inherits_no_descriptor") else {
        return;
    };
    let (_d, t) = tempdir();
    let req = bytes(&agent(&t));
    let out = launch_raw(
        Some(&req),
        &["--policy-fd", "3", "--", "cat", &key(&t)],
        &[PATH],
    );
    assert_ne!(out.status.code(), Some(0), "local.key was readable");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Permission denied"),
        "{out:?}"
    );
    // A program outside the toolchain cannot be started.
    let copy = t.join("home/bin/true");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy("/usr/bin/true", &copy).unwrap();
    let out = launch_raw(
        Some(&req),
        &["--policy-fd", "3", "--", &copy.to_string_lossy()],
        &[PATH],
    );
    assert_eq!(out.status.code(), Some(126), "{out:?}");
    assert_eq!(failure(&out)["code"], "EXEC_FAILED");
}

#[test]
fn landlock_disabled_runs_unconfined_and_unavailable_refuses() {
    let (_d, t) = tempdir();
    let req = bytes(&agent(&t));
    let stub = |v| [PATH, (fin_sandbox::status::TEST_ENV, v)];
    // F2: the program runs, unconfined, and still inherits nothing but its streams.
    let out = launch_raw(
        Some(&req),
        &[
            "--policy-fd",
            "3",
            "--",
            "sh",
            "-c",
            &format!("cat {} && test ! -e /proc/self/fd/3", key(&t)),
        ],
        &stub("disabled"),
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "k");
    let out = launch_raw(
        Some(&req),
        &["--policy-fd", "3", "--", "true"],
        &stub("unavailable"),
    );
    assert_eq!(out.status.code(), Some(78));
    let f = failure(&out);
    assert_eq!(f["code"], "SANDBOX_UNAVAILABLE");
    assert_eq!(f["profile"], "agent");
}
