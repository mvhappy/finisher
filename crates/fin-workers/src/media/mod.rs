//! The `media` worker family (sub-project 6 spec §3; plan 6A Tasks 11 and 13): `media.look` v1
//! and `media.capabilities` v1; `media.probeDeep` v1 (plan 6B-1 Task 11, cold).
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
//! - `media.proxy` v1 (plan 6B-1 Task 14, cold): a proxy into `out/<key>/` (`proxy.rs`, the
//!   GPU scale in `scale.rs`, the encoder and muxer in `encode_pipeline.rs`).
//! - `media.thumbs` v1 (plan 6B-1 Task 15, cold): thumbnails and filmstrips into `out/<key>/`
//!   (`thumbs.rs`, the frames' source in `thumbs_source.rs`, upright pixels in `pixels.rs`).
//! - `media.pcm` v1 (plan 6C Task 18, cold): a `pcm48` or `pcm16m` feed into `out/<key>/`
//!   (`pcm.rs`; decoded audio meets `fin-audio` only in `audio_src.rs`).

use fin_codecs::{Capabilities, ProbeResult};
use fin_compositor::{Compositor, Gpu};
use fin_jobs::worker::{Family, JobCtx};
use fin_jobs::{JobError, KindVersion, Retry, StartJob};
use serde_json::Value;

mod audio_src;
mod caps;
mod diag;
mod encode_pipeline;
mod fonts;
mod inputs;
mod look;
mod open_cache;
mod pcm;
mod pixels;
mod probe_deep;
mod proxy;
mod proxy_source;
pub mod replay;
mod scale;
mod sources;
mod thumbs;
mod thumbs_source;

static KINDS: &[KindVersion] = &[
    KindVersion::new("media.look", 1),
    KindVersion::new("media.capabilities", 1),
    KindVersion::new("media.probeDeep", 1),
    KindVersion::new("media.proxy", 1),
    KindVersion::new("media.thumbs", 1),
    KindVersion::new("media.pcm", 1),
];

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
    /// The backends this machine has (6A's software pair until 6B-1 Task 8's probe).
    pub probe: ProbeResult,
    /// Their v1 table (`media.capabilities` v1).
    pub caps: Capabilities,
    /// Jobs started on this worker.
    pub jobs: u32,
    /// Stage timings since the last flush (`render.stats`; filled only with diagnostics on).
    pub render_stats: diag::RenderBook,
    /// `--replay`: outputs are never replaced (a leftover is refused).
    pub replay: bool,
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
                probe: ProbeResult::software_only(),
                caps: fin_codecs::capabilities(),
                jobs: 0,
                render_stats: diag::RenderBook::default(),
                replay: false,
            },
        }
    }

    /// The family as `--replay` runs it: outputs are never replaced, and originals open only
    /// below `confine` (`None`: anywhere, `--originals-anywhere`).
    pub fn replaying(confine: Option<std::path::PathBuf>) -> Self {
        let mut family = Self::new(false);
        family.state.replay = true;
        family.state.sources.confine(confine);
        family
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
        let out = match (job.kind.as_str(), job.kind_version) {
            ("media.look", 1) => look::run(job, ctx, &mut self.state),
            ("media.capabilities", 1) => caps::run(job, ctx, &mut self.state),
            ("media.probeDeep", 1) => probe_deep::run(job, ctx, &mut self.state),
            ("media.proxy", 1) => proxy::run(job, ctx, &mut self.state),
            ("media.thumbs", 1) => thumbs::run(job, ctx, &mut self.state),
            ("media.pcm", 1) => pcm::run(job, ctx, &mut self.state),
            (kind, version) => Err(JobError::new(
                "JOB_KIND_UNKNOWN",
                format!("family media has no kind {kind} v{version}"),
            )),
        };
        // The timings of a failed job count too (the stages that ran).
        diag::flush(&mut self.state.render_stats, ctx);
        out
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
