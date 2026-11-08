//! Probing one catalogued file (plan 5D Decisions 34, 35 and 20): the file is opened once,
//! through [`open_for_read`] with its catalogued raw path and `expect`, and `fp1` and the
//! shallow probe are both computed from that handle (`fin_probe::probe_reader`, the
//! fingerprint first), read through the library's [`Throttle`]. A file whose probe fails keeps
//! its fingerprint.
//!
//! The outcome is what `library.probed` carries to the catalog. Its `state` is a file state
//! of the catalog (`present`, `missing`, `cloudOnly`, `unreadable`, `probeFailed`) or
//! [`CHANGED`], which is **never stored**: the file no longer matches its row, nothing was
//! read, and the catalog rescans its directory instead. A cancelled run answers
//! [`CANCELLED`], also never stored: the worker drops it.

use std::fs::File;
use std::io;
use std::path::Path;

use fin_probe::{FileMeta, PROBE_VERSION, Probe};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::budget::Throttle;
use crate::open::open_for_read;
use crate::raw::RawRel;
use crate::types::{Expect, OpenError};

pub const PRESENT: &str = "present";
pub const MISSING: &str = "missing";
pub const CLOUD_ONLY: &str = "cloudOnly";
pub const UNREADABLE: &str = "unreadable";
pub const PROBE_FAILED: &str = "probeFailed";
/// The catalog's state of a file the worker did not read (Decision 37's extension filter).
pub const SKIPPED: &str = "skipped";
/// Not a stored file state: the file differs from its catalogued row (rescan its directory).
pub const CHANGED: &str = "changed";
/// Not a stored file state: the run was cancelled (drop the outcome).
pub const CANCELLED: &str = "cancelled";

/// One catalogued file to read: where it is (root and raw path), what the catalog expects of
/// it, the library's `hydrate`, and the path error messages name (never opened).
#[derive(Debug, Clone, Copy)]
pub struct FileRef<'a> {
    /// The catalog's file id, echoed in the outcome.
    pub file: i64,
    pub root: &'a Path,
    pub rel: &'a RawRel,
    pub expect: &'a Expect,
    pub hydrate: bool,
    pub display: &'a Path,
}

impl FileRef<'_> {
    /// [`open_for_read`] of this file: the only open of it.
    pub fn open(&self) -> Result<File, OpenError> {
        open_for_read(self.root, self.rel, self.expect, self.hydrate)
    }
}

/// What `library.probed` reports for one file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProbeOutcome {
    pub file: i64,
    /// `fp1`, whenever the file could be read that far, even if the probe failed.
    pub fingerprint: Option<String>,
    pub probe: Option<Probe>,
    pub probe_version: u32,
    pub state: String,
    /// The error code of a state other than `present`.
    pub code: Option<String>,
}

impl ProbeOutcome {
    /// A file the library's kinds table does not list: not opened, `skipped` with `code`.
    pub fn skipped(file: i64, code: &str) -> Self {
        Self::new(file, SKIPPED, Some(code))
    }

    fn new(file: i64, state: &str, code: Option<&str>) -> Self {
        ProbeOutcome {
            file,
            fingerprint: None,
            probe: None,
            probe_version: PROBE_VERSION,
            state: state.into(),
            code: code.map(str::to_owned),
        }
    }
}

/// The state and code a refused open (or a failed read) leaves a file in.
pub(crate) fn refusal(e: &OpenError) -> (&'static str, &'static str) {
    match e {
        OpenError::CloudOnly => (CLOUD_ONLY, "FILE_CLOUD_ONLY"),
        OpenError::Changed => (CHANGED, "FILE_CHANGED"),
        OpenError::NotFound => (MISSING, "FILE_NOT_FOUND"),
        OpenError::Escapes | OpenError::NameSurrogate => (UNREADABLE, "INVALID_PATH"),
        OpenError::Io(e) => io_refusal(e),
    }
}

pub(crate) fn io_refusal(e: &io::Error) -> (&'static str, &'static str) {
    if e.kind() == io::ErrorKind::NotFound {
        (MISSING, "FILE_NOT_FOUND")
    } else {
        (UNREADABLE, "ASSET_UNREADABLE")
    }
}

/// Opens `f` once and probes it: the fingerprint, then the kind and its fields, from that one
/// handle through `throttle`. Never panics on file content (`fin-probe` catches the
/// container reader's).
pub fn probe_file(f: &FileRef<'_>, throttle: &Throttle) -> ProbeOutcome {
    let file = match f.open() {
        Ok(file) => file,
        Err(e) => {
            let (state, code) = refusal(&e);
            return ProbeOutcome::new(f.file, state, Some(code));
        }
    };
    let meta = match file.metadata() {
        Ok(m) => FileMeta::of(&m),
        Err(e) => {
            let (state, code) = io_refusal(&e);
            return ProbeOutcome::new(f.file, state, Some(code));
        }
    };
    let out = fin_probe::probe_reader(throttle.reader(file), meta, f.display);
    let mut o = ProbeOutcome::new(f.file, PRESENT, None);
    o.fingerprint = out.fingerprint;
    match out.probe {
        _ if throttle.is_cancelled() => {
            o.state = CANCELLED.into();
            o.fingerprint = None;
        }
        Ok(probe) => o.probe = Some(probe),
        Err(e) => {
            // No fingerprint means the read itself failed under it: the file is unreadable.
            o.state = if o.fingerprint.is_some() {
                PROBE_FAILED
            } else {
                UNREADABLE
            }
            .into();
            o.code = Some(e.code.to_string());
        }
    }
    o
}
