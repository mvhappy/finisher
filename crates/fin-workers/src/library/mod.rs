//! The `library` worker family (plan 5D Task 6; spec §6.2, §7): `library.scan`,
//! `library.probe` and `library.hash`. Every byte it reads under a library root goes through
//! `fin-library` (the walker's [`Source`](fin_library::Source) for directories,
//! [`open_for_read`](fin_library::open_for_read) for files); the clippy `disallowed-methods`
//! gate (`clippy.toml`) keeps every other path-based read out of this crate, and the family
//! starts no process (Decision 1).
//!
//! - **Dataless policy:** [`LibraryFamily::init`] turns materialisation off before `hello`
//!   (Decision 6); only a cold `hydrate: true` job turns it back on, for its own process.
//! - **Offline** (Decision 33): a root that is unusable at the start, or found unusable when
//!   a file fails, ends the job with `LIBRARY_OFFLINE {state}` and `retry: never`; the
//!   outcome of the failing file is dropped, so nothing is ever marked missing under an
//!   offline root.
//! - **Batches** (Decision 26): the scan writes `out/scan-<seq>.ndjson` (one line, one
//!   generated `ScanBatch`; `fin_library::batchfile`) and `call library.batch {file, seq}`;
//!   the reply means the indexer applied it, and only then is the file removed.
//! - **Probe results** (Decision 34): up to 32 outcomes per `call library.probed`, then one
//!   `item` per file they cover. `changed` outcomes travel with them (the catalog rescans
//!   those files' directories); a `cancelled` outcome is dropped with its item unacknowledged.

use fin_jobs::worker::{Family, JobCtx};
use fin_jobs::{JobError, KindVersion, StartJob};
use serde_json::Value;

mod common;
mod hash;
mod jobfiles;
mod probe;
mod scan;
#[cfg(feature = "test-hooks")]
mod stage_test;

#[cfg(not(feature = "test-hooks"))]
static KINDS: &[KindVersion] = &[
    KindVersion::new("library.scan", 1),
    KindVersion::new("library.probe", 1),
    KindVersion::new("library.hash", 1),
];
/// Test builds add `test.stage`, the stage framework's test kind (plan 5D Task 11).
#[cfg(feature = "test-hooks")]
static KINDS: &[KindVersion] = &[
    KindVersion::new("library.scan", 1),
    KindVersion::new("library.probe", 1),
    KindVersion::new("library.hash", 1),
    KindVersion::new("test.stage", 1),
];

/// The `library` family.
pub struct LibraryFamily {
    warm: bool,
}

impl LibraryFamily {
    pub fn new(warm: bool) -> Self {
        Self { warm }
    }
}

impl Family for LibraryFamily {
    fn name(&self) -> &'static str {
        "library"
    }

    fn kinds(&self) -> &'static [KindVersion] {
        KINDS
    }

    fn warm(&self) -> bool {
        self.warm
    }

    fn init(&mut self) -> Result<(), String> {
        fin_library::process::deny_dataless_materialisation()
            .map_err(|e| format!("dataless materialisation stays on: {e}"))
    }

    fn run(&mut self, job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
        match (job.kind.as_str(), job.kind_version) {
            ("library.scan", 1) => scan::run(job, ctx),
            ("library.probe", 1) => probe::run(job, ctx, self.warm),
            ("library.hash", 1) => hash::run(job, ctx, self.warm),
            #[cfg(feature = "test-hooks")]
            ("test.stage", 1) => stage_test::run(job, ctx),
            (kind, version) => Err(JobError::new(
                "JOB_KIND_UNKNOWN",
                format!("family library has no kind {kind} v{version}"),
            )),
        }
    }
}
