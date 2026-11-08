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
