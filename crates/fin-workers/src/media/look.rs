//! `media.look` v1 (spec §3.3; plan 6A Task 11): for each time, `begin {item: i}`, the plan
//! (`eval_frame` over the snapshot, with the worker's measurer), its BLAKE3 against the plan
//! sample (`PLAN_MISMATCH` on any difference or a missing sample), a GPU render over the
//! worker's sources, the PNG in `out/`, then `item {item: i, value: LookFrame}`. The result is
//! a [`LookResult`]. A cancel is seen between frames.

use fin_compositor::{AdapterChoice, AdapterSummary, Compositor, Gpu};
use fin_core::eval::{FramePlan, eval_frame};
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, StartJob};
use fin_media::Size;
use fin_media::jobs::{
    GpuAdapter, JobSnapshot, LookFrame, LookParams, LookResult, LookSkipped, LookStats,
    MANIFEST_VERSION, RenderManifest,
};
use serde_json::{Value, json};

use super::inputs::{self, MANIFEST_CAP, SNAPSHOT_CAP};
use super::sources::Sources;
use super::{GpuState, Warm, job_error};

/// BLAKE3 of the plan's compact JSON (plan 6A Decision 4), lowercase hex.
pub fn plan_hash(plan: &FramePlan) -> String {
    let json = serde_json::to_string(plan).expect("a plan serialises");
    blake3::hash(json.as_bytes()).to_hex().to_string()
}

pub(super) fn adapter(a: AdapterSummary) -> GpuAdapter {
    GpuAdapter {
        backend: a.backend,
        device_type: a.device_type,
        name: a.name,
        vendor: a.vendor,
        device: a.device,
        software: a.software,
    }
}

/// The worker's GPU, made on first use (`FIN_GPU` picks the adapter).
pub(super) fn gpu(warm: &mut Warm) -> Result<&mut GpuState, JobError> {
    if warm.gpu.is_none() {
        let choice = AdapterChoice::from_env().map_err(job_error)?;
        let gpu = Gpu::new(choice).map_err(job_error)?;
        let compositor = Compositor::new(&gpu);
        warm.gpu = Some(GpuState { gpu, compositor });
    }
    Ok(warm.gpu.as_mut().expect("just made"))
}

fn mismatch(t: fin_core::Rational, want: Option<&str>, got: &str) -> JobError {
    let reason = if want.is_some() { "hash" } else { "noSample" };
    JobError::new(
        "PLAN_MISMATCH",
        format!("the worker's plan at {t} differs from the host's ({reason})"),
    )
    .with_details(json!({ "at": t, "reason": reason, "want": want, "got": got }))
}

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: &mut Warm) -> Result<Value, JobError> {
    let mut params: LookParams = serde_json::from_value(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("params: {e}")))?;
    params.check().map_err(job_error)?;
    // The kind's `prepare` computes the samples from the snapshot it writes and returns them as
    // `extra.planSamples`; params that carry their own (replay, tests) win.
    if params.plan_samples.is_empty()
        && let Some(samples) = job.extra.as_ref().and_then(|e| e.get("planSamples"))
    {
        params.plan_samples = serde_json::from_value(samples.clone())
            .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("extra.planSamples: {e}")))?;
    }
    let dir = inputs::job_dir(ctx)?;
    let snapshot: JobSnapshot = inputs::read_json(ctx, &dir, "in/snapshot.json", SNAPSHOT_CAP)?;
    let manifest: RenderManifest = inputs::read_json(ctx, &dir, "in/manifest.json", MANIFEST_CAP)?;
    if manifest.version != MANIFEST_VERSION {
        return Err(JobError::new(
            "JOB_INPUT_INVALID",
            format!(
                "manifest version {} (this worker reads {MANIFEST_VERSION})",
                manifest.version
            ),
        )
        .with_details(json!({ "file": "in/manifest.json", "version": manifest.version })));
    }
    let out = inputs::out_dir(ctx, &dir)?;
    warm.fonts.set(&manifest.fonts);
    warm.sources.forget_failures();
    let device_reused = warm.gpu.is_some();
    gpu(warm)?;
    let mut stats = LookStats {
        device_reused,
        worker_jobs: warm.jobs,
        ..LookStats::default()
    };
    let mut frames = Vec::with_capacity(params.at.len());
    let mut skipped = Vec::new();
    for (i, &t) in params.at.iter().enumerate() {
        if ctx.cancelled().is_some() {
            return Err(JobError::cancelled());
        }
        let item = i.to_string();
        ctx.begin(&item);
        let plan = eval_frame(
            &snapshot.project,
            &params.timeline,
            t,
            Some(warm.fonts.measurer()),
        )
        .map_err(job_error)?;
        let hash = plan_hash(&plan);
        let want = params.sample(t).map(|s| s.blake3.as_str());
        if want != Some(hash.as_str()) {
            return Err(mismatch(t, want, &hash));
        }
        let size = params
            .size
            .unwrap_or(Size::new(plan.resolution.width, plan.resolution.height));
        let mut src = Sources::new(&manifest, &mut warm.sources, &warm.caps);
        let state = warm.gpu.as_mut().expect("made above");
        let rendered = state.compositor.render(&state.gpu, &plan, &mut src, size);
        stats.sources_opened += src.opened;
        stats.sources_reused += src.reused;
        let rendered = match rendered {
            Ok(r) => r,
            Err(e) => {
                // A device that failed is not kept for the next job.
                if e.code.starts_with("GPU_") {
                    warm.gpu = None;
                }
                return Err(job_error(e));
            }
        };
        let file = params.file_name(i);
        inputs::write_png(&out.join(&file), size.width, size.height, &rendered.rgba8)?;
        skipped.extend(rendered.skipped.into_iter().map(|s| LookSkipped {
            at: t,
            item: s.item,
            kind: s.kind,
            code: s.code,
            details: s.details,
        }));
        let frame = LookFrame {
            at: t,
            file,
            width: size.width,
            height: size.height,
            plan: hash,
        };
        ctx.item(
            &item,
            Some(serde_json::to_value(&frame).expect("a frame serialises")),
        );
        frames.push(frame);
    }
    let adapter = adapter(warm.gpu.as_ref().expect("made above").gpu.info());
    let result = LookResult {
        frames,
        skipped,
        adapter,
        stats,
    };
    Ok(serde_json::to_value(result).expect("a result serialises"))
}
