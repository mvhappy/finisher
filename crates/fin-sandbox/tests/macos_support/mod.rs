//! Shared by the macOS escape suites: runs `fin-sandbox-probe` and the launcher, and reads
//! their answers. Seatbelt refuses with `EPERM` (and `EACCES` for some file operations).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub const PROBE: &str = env!("CARGO_BIN_EXE_fin-sandbox-probe");
pub const LAUNCHER: &str = env!("CARGO_BIN_EXE_fin-sandbox-exec");

/// The errnos a Seatbelt denial shows. `ENOENT` is never one.
pub const DENIALS: [i32; 2] = [libc::EPERM, libc::EACCES];

/// What one probe run showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The attempt ran: `None` succeeded, `Some(errno)` failed.
    Ran(Option<i32>),
    /// The sandbox could not be applied (exit 3).
    Apply(String),
    /// Anything else (an `exec` that replaced the probe, a crash).
    Other { code: Option<i32>, stdout: String },
}

impl Outcome {
    pub fn denied(&self) -> bool {
        matches!(self, Outcome::Ran(Some(e)) if DENIALS.contains(e))
    }
}

/// Runs the probe with `flags` (sandbox selection) and `op`, with an empty environment.
pub fn probe(flags: &[String], op: &[String]) -> Outcome {
    let out = Command::new(PROBE)
        .args(flags)
        .args(op)
        .env_clear()
        .output()
        .expect("run fin-sandbox-probe");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: Option<serde_json::Value> = serde_json::from_str(stdout.trim()).ok();
    match (out.status.code(), v) {
        (Some(0), Some(v)) if v.get("op").is_some() => {
            Outcome::Ran(v["errno"].as_i64().map(|e| e as i32))
        }
        (Some(3), Some(v)) => Outcome::Apply(v["apply"].as_str().unwrap_or("").to_owned()),
        (code, _) => Outcome::Other { code, stdout },
    }
}

/// The probe under `policy` (written next to `dir`) and unsandboxed.
pub fn both(dir: &Path, policy: &impl serde::Serialize, op: &[&str]) -> (Outcome, Outcome) {
    let op: Vec<String> = op.iter().map(|s| s.to_string()).collect();
    let file = json_file(dir, "policy.json", policy);
    (
        probe(&["--policy".into(), file], &op),
        probe(&["--none".into()], &op),
    )
}

/// Asserts that `op` is denied under `policy` and succeeds unsandboxed (the denial is the
/// sandbox's).
#[track_caller]
pub fn assert_denied(dir: &Path, policy: &impl serde::Serialize, op: &[&str]) {
    let (sandboxed, control) = both(dir, policy, op);
    assert!(sandboxed.denied(), "{op:?} under the policy: {sandboxed:?}");
    assert_eq!(control, Outcome::Ran(None), "{op:?} unsandboxed (control)");
}

/// Asserts that `op` succeeds under `policy`.
#[track_caller]
pub fn assert_allowed(dir: &Path, policy: &impl serde::Serialize, op: &[&str]) {
    let (sandboxed, _) = both(dir, policy, op);
    assert_eq!(sandboxed, Outcome::Ran(None), "{op:?} under the policy");
}

/// Writes `value` as JSON to `dir/name` and returns the path as a string.
pub fn json_file(dir: &Path, name: &str, value: &impl serde::Serialize) -> String {
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_vec(value).unwrap()).unwrap();
    p.to_string_lossy().into_owned()
}

/// A canonical temporary directory (`/var/folders/…` is `/private/var/folders/…`).
pub fn tempdir() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = std::fs::canonicalize(d.path()).unwrap();
    (d, p)
}

pub fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A loopback listener (tests never use the network) and its port.
pub fn listener() -> (std::net::TcpListener, u16) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

pub fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Runs `fin-sandbox-exec` with `request` on descriptor 3 and `args` after its own flags, with
/// exactly `env`.
pub fn launch(request: &[u8], args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    launch_at(None, request, args, env)
}

/// [`launch`] in the working directory `cwd`.
pub fn launch_at(
    cwd: Option<&Path>,
    request: &[u8],
    args: &[&str],
    env: &[(&str, &str)],
) -> std::process::Output {
    let mut full = vec!["--policy-fd", "3", "--"];
    full.extend_from_slice(args);
    let (mut cmd, keep) = launch_cmd(request, &full, env);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let out = cmd.output().expect("run fin-sandbox-exec");
    drop(keep);
    out
}

/// The launcher with exactly `args` and `env`, not yet started, and the request pipe's read
/// end, which must stay open until the command has started.
pub fn launch_cmd(
    request: &[u8],
    args: &[&str],
    env: &[(&str, &str)],
) -> (Command, std::os::fd::OwnedFd) {
    let mut cmd = Command::new(LAUNCHER);
    cmd.args(args).env_clear().envs(env.iter().copied());
    let keep = give_request(&mut cmd, request);
    (cmd, keep)
}

/// Makes `cmd`'s process inherit a pipe holding `request` as descriptor 3; the returned read
/// end must stay open until the spawn.
pub fn give_request(cmd: &mut Command, request: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;

    let mut fds = [0; 2];
    // SAFETY: a valid array for the two new descriptors.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: both descriptors are fresh and owned here.
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&r, &w] {
        // SAFETY: a flag on our own descriptor.
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    assert!(request.len() < 16_000, "the pipe holds the whole request");
    std::fs::File::from(w).write_all(request).unwrap();
    let raw = r.as_raw_fd();
    // SAFETY: only async-signal-safe calls in the child; `r` lives until the spawn.
    unsafe {
        cmd.pre_exec(move || {
            if raw == 3 {
                libc::fcntl(3, libc::F_SETFD, 0);
            } else if libc::dup2(raw, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    r
}

/// An agent request for a project and server data under `t` (canonical), with the base
/// toolchain.
pub fn agent(t: &Path) -> fin_sandbox::launch::LaunchRequest {
    use fin_core::events::payloads::agent::{AgentMode, Harness};
    for d in ["p/workspace/s1", "data/agents/s1/tmp", "home"] {
        std::fs::create_dir_all(t.join(d)).unwrap();
    }
    write(&t.join("data/auth/local.key"), "k");
    fin_sandbox::launch::LaunchRequest::Agent(fin_sandbox::AgentInput {
        session_id: "s1".into(),
        mode: AgentMode::Draft,
        harness: Harness::Codex,
        project_dir: t.join("p"),
        workspace_dir: t.join("p/workspace/s1"),
        shared_dir: None,
        library_roots: vec![],
        session_dir: t.join("data/agents/s1"),
        sign_in_home: None,
        sign_in_store: None,
        toolchains: fin_sandbox::data::toolchain_base_resolved(fin_sandbox::Os::Macos),
        server_data: t.join("data"),
        home: t.join("home"),
        fin: None,
    })
}
