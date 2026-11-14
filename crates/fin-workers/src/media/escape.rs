//! `media.testEscape` (`test-hooks` builds only; plan 6B-1 Task 18): file-system attempts made
//! from inside a real `fin-workers media` process, after the `worker` profile it was started
//! under has been applied. The escape suite (`tests/media_sandbox.rs`) runs each attempt here
//! under the input a media job gets and again unconfined (the control), and asserts the OS's
//! answer: a denial must be an observed `EACCES`/`EPERM`/`EXDEV`, never an absence of output.
//! The kind does nothing else: no media is read, no output is written beyond what an attempt
//! names.
// The attempts are path-based by design (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;

use fin_jobs::{JobError, StartJob};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The kind's name (never registered by `finisherd`).
pub const KIND: &str = "media.testEscape";

/// One attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "op")]
pub enum Attempt {
    /// Opens `path` read-only and reads its first byte.
    Read { path: PathBuf },
    /// Creates `path` (`O_EXCL`) and writes a byte.
    Create { path: PathBuf },
    /// Opens the existing `path` for writing.
    Write { path: PathBuf },
    /// `link(path, to)`.
    Link { path: PathBuf, to: PathBuf },
    /// `symlink(target, to)`.
    Symlink { target: PathBuf, to: PathBuf },
    /// `rename(path, to)`.
    Rename { path: PathBuf, to: PathBuf },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Params {
    attempts: Vec<Attempt>,
}

fn attempt(a: &Attempt) -> std::io::Result<()> {
    match a {
        Attempt::Read { path } => {
            let mut b = Vec::new();
            OpenOptions::new()
                .read(true)
                .open(path)?
                .take(1)
                .read_to_end(&mut b)
                .map(|_| ())
        }
        Attempt::Create { path } => OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?
            .write_all(b"x"),
        Attempt::Write { path } => OpenOptions::new().write(true).open(path)?.write_all(b"x"),
        Attempt::Link { path, to } => std::fs::hard_link(path, to),
        #[cfg(unix)]
        Attempt::Symlink { target, to } => std::os::unix::fs::symlink(target, to),
        #[cfg(not(unix))]
        Attempt::Symlink { .. } => Err(std::io::ErrorKind::Unsupported.into()),
        Attempt::Rename { path, to } => std::fs::rename(path, to),
    }
}

/// `{errnos: [null | errno]}`, one per attempt in order (`null`: it succeeded; `-1`: it failed
/// without an OS error number).
pub fn run(job: &StartJob) -> Result<Value, JobError> {
    let p: Params = serde_json::from_value(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("params: {e}")))?;
    let errnos: Vec<Option<i32>> = p
        .attempts
        .iter()
        .map(|a| attempt(a).err().map(|e| e.raw_os_error().unwrap_or(-1)))
        .collect();
    Ok(serde_json::json!({ "errnos": errnos }))
}
