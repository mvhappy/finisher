//! One segment's folder I/O (module docs, step 4), without the store: the fence, the
//! directory, adopting or encoding, the write and its read-back. [`super::plan_step`] plans it
//! under the store lock and [`super::apply_step`] applies the [`Outcome`] under it again.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fin_core::CoreError;
use serde_json::json;

use super::policy::Fence;
use super::prepare::{Prep, adoptable};
use super::{Attempt, MAX_FAILED_READS, READBACK_DELAY, READBACK_RETRIES, Row};
use crate::reserved::{is_linked, no_links};
use crate::{StoreError, fsio, segment};

/// A planned step: the next unexported rows of `branch` and what writing them needs.
pub(super) struct StepIo {
    pub(super) branch: String,
    pub(super) rows: Vec<Row>,
    pub(super) prev: Option<String>,
    pub(super) dir: PathBuf,
    /// The attempt of a failed write to retry with the same bytes.
    pub(super) retry: Option<Attempt>,
    pub(super) prep: Prep,
    pub(super) fence: Fence,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fail_at: Option<super::FailPoint>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) hooks: crate::tick::hooks::TickHooks,
}

/// What a step's folder I/O did.
pub(super) enum Outcome {
    /// The lease does not allow writing; `keep` is retried next time.
    Stopped { keep: Option<Attempt> },
    /// A folder failure; `keep` is retried next time with the same bytes.
    Failed {
        error: CoreError,
        keep: Option<Attempt>,
    },
    /// Our name holds something else.
    Collision {
        target: PathBuf,
        attempt: Attempt,
        why: String,
    },
    /// Written and verified by read-back: to mark.
    Wrote(Attempt),
    /// Found in the folder (a crash left it): to mark.
    Adopted(Attempt),
    /// Encoding found the database broken (`DB_ERROR`).
    Broken(StoreError),
    /// An injected `FailPoint` stopped it (`TEST_FAULT`); `keep` stays for the retry.
    #[cfg(any(test, feature = "test-hooks"))]
    Fault { keep: Attempt },
}

impl StepIo {
    /// Runs the step's folder I/O; `true` when it got as far as the write (which uses up an
    /// injected `FailPoint`).
    pub(super) fn run(mut self) -> (Outcome, bool) {
        let Ok(lease_epoch) = self.fence.check() else {
            return (Outcome::Stopped { keep: self.retry }, false);
        };
        // Never through a link (`crate::reserved`): checked before `create_dir_all`, which
        // would follow one, and after it.
        let made = no_links(&self.dir)
            .and_then(|()| fs::create_dir_all(&self.dir))
            .and_then(|()| no_links(&self.dir));
        if let Err(e) = made {
            let error = folder_error(&self.dir, &self.branch, &e);
            return (
                Outcome::Failed {
                    error,
                    keep: self.retry,
                },
                false,
            );
        }
        let attempt = match self.retry.take() {
            Some(a) => a,
            None => {
                let prev = self.prev.as_deref();
                let id = &self.prep.project_id;
                if let Some(found) = adoptable(id, &self.branch, &self.dir, &self.rows, prev) {
                    return (Outcome::Adopted(found), false);
                }
                let prev = self.prev.clone();
                match self
                    .prep
                    .encode(&self.branch, &self.rows, lease_epoch, prev)
                {
                    Ok(a) => a,
                    Err(e) => return (Outcome::Broken(e), false),
                }
            }
        };
        (self.write(attempt), true)
    }

    /// Writes `a` and verifies it by read-back.
    fn write(&self, a: Attempt) -> Outcome {
        let dir = &self.dir;
        #[cfg(any(test, feature = "test-hooks"))]
        if self.fail_at == Some(super::FailPoint::AfterTempWrite) {
            let _ = fs::write(dir.join(crate::names::temp_name()), &a.bytes);
            return Outcome::Fault { keep: a };
        }
        #[cfg(any(test, feature = "test-hooks"))]
        self.hooks.at(crate::tick::hooks::TickPoint::ExportWrite);
        // The fence, right before the write.
        if !self.fence.check().is_ok_and(|e| a.epoch <= e) {
            return Outcome::Stopped { keep: Some(a) };
        }
        if let Err(e) = no_links(dir) {
            return self.failed(dir, &e, a);
        }
        let written = fsio::write_once(dir, &a.name, &a.bytes);
        #[cfg(any(test, feature = "test-hooks"))]
        if self.fail_at == Some(super::FailPoint::AfterRename) {
            return Outcome::Fault { keep: a };
        }
        let target = dir.join(&a.name);
        // Absent after a write that did not plainly fail: SMB may not show a fresh rename yet.
        let plain_failure = written.as_ref().is_err_and(|e| {
            !matches!(
                e.kind(),
                io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
            )
        });
        // Only a write that put a file in place (or found one there) makes an unverifiable
        // read-back evidence of a foreign file; otherwise it is the folder failing.
        let file_in_place = written
            .as_ref()
            .map_or_else(|e| e.kind() == io::ErrorKind::AlreadyExists, |()| true);
        let retries = if plain_failure { 0 } else { READBACK_RETRIES };
        let mut a = a;
        match read_back(&target, &a.hash, retries) {
            ReadBack::Ours => {}
            ReadBack::Other(why) => {
                return Outcome::Collision {
                    target,
                    attempt: a,
                    why,
                };
            }
            ReadBack::Missing(read_err) if read_err.kind() == io::ErrorKind::NotFound => {
                let e = written.err().unwrap_or(read_err);
                return self.failed(&target, &e, a);
            }
            ReadBack::Missing(read_err) if !file_in_place => {
                let e = written.err().unwrap_or(read_err);
                return self.failed(&target, &e, a);
            }
            ReadBack::Missing(read_err) => {
                a.failed_reads += 1;
                if a.failed_reads >= MAX_FAILED_READS {
                    let why = format!("{} read-backs failed: {read_err}", a.failed_reads);
                    return Outcome::Collision {
                        target,
                        attempt: a,
                        why,
                    };
                }
                return self.failed(&target, &read_err, a);
            }
        }
        // A link swapped in during the write: what was read back may lie outside the project.
        if let Err(e) = no_links(dir) {
            return self.failed(dir, &e, a);
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if self.fail_at == Some(super::FailPoint::BeforeMarkExported) {
            return Outcome::Fault { keep: a };
        }
        Outcome::Wrote(a)
    }

    fn failed(&self, path: &Path, e: &io::Error, a: Attempt) -> Outcome {
        Outcome::Failed {
            error: folder_error(path, &self.branch, e),
            keep: Some(a),
        }
    }
}

enum ReadBack {
    Ours,
    /// A complete file with other content (or damaged) under our name.
    Other(String),
    /// Absent, unreadable or still incomplete.
    Missing(io::Error),
}

/// Reads `path` back uncached; `NotFound` is retried `retries` times.
fn read_back(path: &Path, hash: &str, retries: u32) -> ReadBack {
    let mut left = retries;
    let bytes = loop {
        match fsio::read_uncached(path) {
            Ok(b) => break b,
            Err(e) if e.kind() == io::ErrorKind::NotFound && left > 0 => {
                left -= 1;
                std::thread::sleep(READBACK_DELAY);
            }
            Err(e) => return ReadBack::Missing(e),
        }
    };
    match segment::decode(&bytes) {
        Ok(seg) if seg.hash == hash => ReadBack::Ours,
        Ok(seg) => ReadBack::Other(format!("hash {} is not ours", seg.hash)),
        Err(segment::SegError::Incomplete) => ReadBack::Missing(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the written segment reads back incomplete",
        )),
        Err(e) => ReadBack::Other(e.to_string()),
    }
}

/// A folder failure as reported in status: the records stay unexported.
fn folder_error(path: &Path, branch: &str, e: &io::Error) -> CoreError {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    CoreError::new(
        "PROJECT_DIR_UNWRITABLE",
        format!("cannot export to {}: {e}", path.display()),
    )
    .with_details(json!({ "path": path.to_string_lossy(), "branch": branch, "reason": e.to_string() }))
    .with_fix(if is_linked(e) {
        "replace the link with the real directory (links in .finisher are never followed); export retries automatically and nothing is lost"
    } else {
        "make the project folder writable or free space; export retries automatically and nothing is lost"
    })
}
