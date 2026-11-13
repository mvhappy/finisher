//! Shared by the Linux escape suites: runs `fin-sandbox-probe` and reads its answer.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use fin_sandbox::linux::landlock::{Abi, Support, query};

pub const PROBE: &str = env!("CARGO_BIN_EXE_fin-sandbox-probe");

/// The errnos a filesystem or TCP denial may show: Landlock answers `EACCES` (and `EXDEV` for
/// a link or rename it refuses). Anything else (`ENOENT` above all, and `EPERM`, which sticky
/// directories and `protected_hardlinks` also give) is not a denial.
pub const DENIALS: [i32; 2] = [libc::EACCES, libc::EXDEV];
/// The ops a Landlock scope refuses, with `EPERM`.
pub const SCOPED: [&str; 1] = ["signal-parent"];

/// The kernel's ABI, or `None` with a printed reason (Landlock disabled: Task 3's stubbed run
/// covers that case). `FIN_REQUIRE_LANDLOCK=1` turns the skip into a failure.
pub fn landlock_or_skip(test: &str) -> Option<Abi> {
    match query() {
        Support::Abi(a) => Some(a),
        other => {
            assert_ne!(
                std::env::var("FIN_REQUIRE_LANDLOCK").as_deref(),
                Ok("1"),
                "{test}: FIN_REQUIRE_LANDLOCK=1 but Landlock answers {other:?}"
            );
            eprintln!("{test}: skipped, Landlock is not available ({other:?})");
            None
        }
    }
}

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
    /// Did `op` (the probe's op and its arguments) show a denial?
    pub fn denied<S: AsRef<str>>(&self, op: &[S]) -> bool {
        let scoped = op.first().is_some_and(|o| SCOPED.contains(&o.as_ref()));
        match self {
            Outcome::Ran(Some(e)) if scoped => *e == libc::EPERM,
            Outcome::Ran(Some(e)) => DENIALS.contains(e),
            _ => false,
        }
    }
}

/// Runs the probe with `flags` (sandbox selection) and `op`.
pub fn probe(flags: &[String], op: &[String]) -> Outcome {
    probe_env(flags, op, &[])
}

/// [`probe`] with `env` set (the status stub).
pub fn probe_env(flags: &[String], op: &[String], env: &[(&str, &str)]) -> Outcome {
    let out = Command::new(PROBE)
        .args(flags)
        .args(op)
        .env_clear()
        .envs(env.iter().copied())
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

/// Writes `value` as JSON to `dir/name` and returns the path as a string.
pub fn json_file(dir: &Path, name: &str, value: &impl serde::Serialize) -> String {
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_vec(value).unwrap()).unwrap();
    p.to_string_lossy().into_owned()
}

/// A canonical temporary directory (the policy refuses non-canonical paths).
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

pub const LAUNCHER: &str = env!("CARGO_BIN_EXE_fin-sandbox-exec");

/// Runs `fin-sandbox-exec` with `request` on descriptor 3 (`None`: no descriptor) and `args`
/// after its own flags, with exactly `env`.
pub fn launch_raw(
    request: Option<&[u8]>,
    args: &[&str],
    env: &[(&str, &str)],
) -> std::process::Output {
    let (mut cmd, _keep) = launch_cmd(request, args, env);
    cmd.output().expect("run fin-sandbox-exec")
}

/// [`launch_raw`]'s command, not yet started, and the request pipe's read end, which must stay
/// open until the command has started.
pub fn launch_cmd(
    request: Option<&[u8]>,
    args: &[&str],
    env: &[(&str, &str)],
) -> (Command, Option<std::os::fd::OwnedFd>) {
    let mut cmd = Command::new(LAUNCHER);
    cmd.args(args).env_clear().envs(env.iter().copied());
    let keep = request.map(|bytes| give_request(&mut cmd, bytes));
    (cmd, keep)
}

/// Makes `cmd`'s process inherit a pipe holding `bytes` as descriptor 3; the returned read
/// end must stay open until the spawn.
pub fn give_request(cmd: &mut Command, bytes: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;

    let mut fds = [0; 2];
    // SAFETY: a valid array for the two new descriptors.
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    // SAFETY: both descriptors are fresh and owned here.
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    assert!(bytes.len() < 60_000, "the pipe holds the whole request");
    std::fs::File::from(w).write_all(bytes).unwrap();
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

/// An agent request for a project and server data under `t`, with the base toolchain.
pub fn agent(t: &Path) -> fin_sandbox::launch::LaunchRequest {
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
    use fin_core::events::payloads::agent::{AgentMode, Harness};
    fin_sandbox::launch::LaunchRequest::Agent(fin_sandbox::AgentInput {
        session_id: "s1".into(),
        mode: AgentMode::Draft,
        harness: Harness::Codex,
        workspace_dir: project.join("workspace").join("s1"),
        shared_dir: None,
        project_dir: project,
        library_roots: vec![],
        session_dir: data.join("agents").join("s1"),
        sign_in_home: None,
        sign_in_store: None,
        toolchains: fin_sandbox::data::toolchain_base_resolved(fin_sandbox::Os::Linux),
        server_data: data,
        home: t.join("home"),
        fin: None,
    })
}

/// The launcher's stderr line, parsed.
pub fn failure(out: &std::process::Output) -> serde_json::Value {
    let text = String::from_utf8_lossy(&out.stderr);
    serde_json::from_str(text.lines().last().unwrap_or("")).unwrap_or(serde_json::Value::Null)
}

/// Makes `cmd`'s process inherit `file` as descriptor 4, without close-on-exec (as a careless
/// parent would leak one). The returned descriptor must stay open until the spawn.
pub fn inherit_fd4(cmd: &mut Command, file: std::fs::File) -> std::os::fd::OwnedFd {
    use std::os::fd::FromRawFd;
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    // Above 4, so neither the request's dup onto 3 nor this one onto 4 overwrites the other.
    // SAFETY: `F_DUPFD_CLOEXEC` makes a new descriptor owned below.
    let raw = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
    assert!(raw >= 10, "{}", std::io::Error::last_os_error());
    // SAFETY: a fresh descriptor nobody else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    drop(file);
    // SAFETY: only async-signal-safe calls in the child; `fd` lives until the spawn.
    unsafe {
        cmd.pre_exec(move || {
            if raw == 4 {
                libc::fcntl(4, libc::F_SETFD, 0);
            } else if libc::dup2(raw, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    fd
}

/// The launcher's `--probe` answer: can it hide Unix sockets in a namespace here (Decision 26)?
pub fn launcher_namespaces(env: &[(&str, &str)]) -> fin_sandbox::Namespaces {
    let out = Command::new(LAUNCHER)
        .arg("--probe")
        .env_clear()
        .envs(env.iter().copied())
        .output()
        .expect("run fin-sandbox-exec --probe");
    assert!(out.status.success(), "--probe: {out:?}");
    let p: fin_sandbox::LauncherProbe = serde_json::from_slice(&out.stdout).expect("probe JSON");
    p.namespaces
}

/// Whether the launcher makes its namespace here, with a printed reason when it cannot.
/// `FIN_REQUIRE_NAMESPACES=1` turns "cannot" into a failure (the privileged-container run).
pub fn namespaces_or_skip(test: &str) -> bool {
    match launcher_namespaces(&[]) {
        fin_sandbox::Namespaces::Available => true,
        other => {
            assert_ne!(
                std::env::var("FIN_REQUIRE_NAMESPACES").as_deref(),
                Ok("1"),
                "{test}: FIN_REQUIRE_NAMESPACES=1 but the launcher answers {other:?}"
            );
            eprintln!("{test}: no socket namespace on this host ({other:?})");
            false
        }
    }
}
