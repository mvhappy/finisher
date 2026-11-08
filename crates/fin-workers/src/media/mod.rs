//! The `media` worker family (sub-project 6 spec §3; plan 6A Task 11): `media.look` v1.
//!
//! - **Inputs** come from the job directory only (`start.files`, opened with `fin-shm`'s
//!   no-follow [`checked_open`](fin_shm::checked_open) below the [`JobDir`](fin_shm::JobDir)),
//!   and originals and font asset files through `fin-library`'s no-follow opener, checked
//!   against the size and mtime the manifest recorded. The clippy `disallowed-methods` gate
//!   (`clippy.toml`) keeps every other path-based read out of this crate.
//! - **Warm reuse** (spec §3.2 item 1, cp §6.3): the GPU device and compiled pipelines, the
//!   open sources (keyed by fingerprint, path, size and mtime: plan 6A Decision 6) and the
//!   measurer's font systems (keyed by the faces' SHA-256) live across the jobs of one warm
//!   worker. They are caches keyed by content: a reused worker writes the same bytes as a
//!   fresh one. Between jobs nothing under the job directory stays open.
//! - **Workers never start processes** (spec §2.9); `tests/media_look.rs` checks it.
//! - `fin-workers media --replay <jobdir>` runs a job directory without a host (`replay.rs`).

use fin_codecs::Capabilities;
use fin_compositor::{Compositor, Gpu};
use fin_jobs::worker::{Family, JobCtx};
use fin_jobs::{JobError, KindVersion, Retry, StartJob};
use serde_json::Value;

mod fonts;
mod inputs;
mod look;
mod open_cache;
pub mod replay;
mod sources;

static KINDS: &[KindVersion] = &[KindVersion::new("media.look", 1)];

/// The GPU state a warm worker keeps.
pub(crate) struct GpuState {
    pub gpu: Gpu,
    pub compositor: Compositor,
}

/// Everything a worker keeps between jobs.
pub(crate) struct Warm {
    pub gpu: Option<GpuState>,
    pub sources: open_cache::OpenCache,
    pub fonts: fonts::Fonts,
    pub caps: Capabilities,
    /// Jobs started on this worker.
    pub jobs: u32,
}

/// The `media` family.
pub struct MediaFamily {
    warm: bool,
    state: Warm,
}

impl MediaFamily {
    pub fn new(warm: bool) -> Self {
        Self {
            warm,
            state: Warm {
                gpu: None,
                sources: open_cache::OpenCache::default(),
                fonts: fonts::Fonts::new(),
                caps: fin_codecs::capabilities(),
                jobs: 0,
            },
        }
    }
}

impl Family for MediaFamily {
    fn name(&self) -> &'static str {
        "media"
    }

    fn kinds(&self) -> &'static [KindVersion] {
        KINDS
    }

    fn warm(&self) -> bool {
        self.warm
    }

    fn run(&mut self, job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
        self.state.jobs += 1;
        match (job.kind.as_str(), job.kind_version) {
            ("media.look", 1) => look::run(job, ctx, &mut self.state),
            (kind, version) => Err(JobError::new(
                "JOB_KIND_UNKNOWN",
                format!("family media has no kind {kind} v{version}"),
            )),
        }
    }
}

/// A `CoreError` as the job's error: `GPU_DEVICE_LOST` is retried once on a fresh worker
/// (Decision 31), `GPU_OUT_OF_MEMORY` with backoff, everything else never.
pub(crate) fn job_error(e: fin_core::CoreError) -> JobError {
    let retry = match e.code.as_str() {
        "GPU_DEVICE_LOST" => Retry::Fresh,
        "GPU_OUT_OF_MEMORY" => Retry::Backoff,
        _ => Retry::Never,
    };
    let mut details = e.details;
    if let Some(fix) = e.fix
        && let Value::Object(m) = &mut details
    {
        m.insert("fix".into(), Value::String(fix));
    }
    JobError::new(e.code, e.message)
        .with_details(details)
        .with_retry(retry)
}
