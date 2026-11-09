//! Media job inputs (plan 6A Task 12; sub-project 6 spec §3.3, §4.2): what `finisherd`'s
//! `media.look` kind needs from the project before a worker can run, and the one move of a
//! job's output.
//!
//! - [`Engine::prepare_media_job`]: on the engine's worker thread, snapshots a branch head,
//!   evaluates the frame plan at every requested time (with the server's shared measurer),
//!   and writes `snapshot.json` ([`fin_media::jobs::JobSnapshot`]), `manifest.json`
//!   ([`fin_media::jobs::RenderManifest`]) and, when given the job's params, `params.json`
//!   (with the plan samples filled) into the job's `in/` directory. Resolves to
//!   `{rev, planSamples, sources}` (the files the worker opens).
//! - `Engine::prepare_asset_job` (`asset_job.rs`, 6B-1 Task 11): the manifest of one asset,
//!   for `media.probeDeep`.
//! - [`commit_job_output`]: `fin_shm::commit_output` as an async call. The kind's
//!   `onSucceeded` moves each finished file out of the job directory with it, after the
//!   worker can no longer write (spec §3.3).
//!
//! `fin-node` links `fin-media` and `fin-shm` but no codec, compositor or GPU crate
//! (`scripts/check-crate-deps.mjs`).

mod asset_job;
mod inputs;

use std::path::{Path, PathBuf};

use fin_core::eval::eval_frame;
use fin_core::{CoreError, Rational};
use fin_media::Size;
use fin_media::jobs::{JobSnapshot, LookParams, PlanSample};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Serialize;
use serde_json::json;

use crate::engine::{Engine, Shared, with_store};
use crate::{encode, guarded, reject};

/// What `prepareMediaJob` resolves to.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Prepared {
    /// The rev of the snapshot written (the branch's head when it was read).
    rev: u64,
    plan_samples: Vec<PlanSample>,
    /// Every file the manifest makes the worker open (the assets' originals and the project's
    /// font files), as the manifest names them: the kind grants each to the worker (plan 7C
    /// Task 7).
    sources: Vec<String>,
}

/// The inputs of one `prepareMediaJob` call.
struct Request {
    branch: String,
    timeline: String,
    at: Vec<String>,
    input_dir: PathBuf,
    font_cache: Option<PathBuf>,
    params_json: Option<String>,
}

fn time(s: &str) -> Result<Rational> {
    s.parse::<Rational>().map_err(|e: CoreError| {
        reject(
            CoreError::new("INVALID_TIME", format!("at is not a time: {}", e.message))
                .with_details(json!({ "field": "at", "value": s }))
                .with_fix("send a rational number of seconds such as \"12\" or \"1001/30000\""),
        )
    })
}

fn prepare(store: &Shared, req: Request) -> Result<String> {
    let times: Vec<Rational> = req.at.iter().map(|s| time(s)).collect::<Result<_>>()?;
    // Params first: a bad request fails before anything is read or written.
    let params = req
        .params_json
        .as_deref()
        .map(|j| {
            let p: LookParams = serde_json::from_str(j).map_err(|e| {
                reject(
                    CoreError::new("JOB_PARAMS_INVALID", format!("params: {e}"))
                        .with_details(json!({ "reason": e.to_string() })),
                )
            })?;
            p.check().map_err(reject)?;
            Ok::<_, Error>(p)
        })
        .transpose()?;
    let (rev, project, project_dir) = with_store(store, |s| {
        let (rev, project) = s.snapshot(&req.branch).map_err(reject)?;
        Ok((rev, project, s.project_dir().to_path_buf()))
    })?;
    let measurer = store.fonts.manager().measurer();
    let mut plans = Vec::with_capacity(times.len());
    let mut samples = Vec::with_capacity(times.len());
    for &at in &times {
        let plan = eval_frame(&project, &req.timeline, at, Some(&*measurer)).map_err(reject)?;
        // The size drawn is limited like a named one (M3): no job for a frame no worker draws.
        if let Some(p) = &params {
            let res = plan.resolution;
            p.effective_size(Size::new(res.width, res.height))
                .map_err(reject)?;
        }
        samples.push(PlanSample {
            at,
            blake3: inputs::plan_hash(&plan),
        });
        plans.push(plan);
    }
    let manifest =
        inputs::build_manifest(&project, &project_dir, &plans, req.font_cache.as_deref());
    let dir = &req.input_dir;
    inputs::write_json(
        dir,
        "snapshot.json",
        &JobSnapshot {
            branch: req.branch.clone(),
            rev,
            project,
        },
    )
    .map_err(reject)?;
    inputs::write_json(dir, "manifest.json", &manifest).map_err(reject)?;
    if let Some(mut p) = params {
        p.plan_samples = samples.clone();
        inputs::write_json(dir, "params.json", &p).map_err(reject)?;
    }
    let mut sources: Vec<String> = manifest
        .assets
        .iter()
        .filter_map(|a| a.original.as_ref().map(|o| o.path.clone()))
        .chain(manifest.fonts.files.iter().map(|f| f.original.path.clone()))
        .collect();
    sources.sort();
    sources.dedup();
    encode(&Prepared {
        rev,
        plan_samples: samples,
        sources,
    })
}

/// The optional inputs of `prepareMediaJob`.
#[napi(object)]
pub struct PrepareMediaOptions {
    /// The server's font cache root, recorded in the manifest.
    pub font_cache: Option<String>,
    /// The job's `LookParams` as JSON: `params.json` is these with the plan samples filled in.
    pub params_json: Option<String>,
}

#[napi]
impl Engine {
    /// Writes `snapshot.json`, `manifest.json` and (with `options.paramsJson`, the job's
    /// `LookParams`) `params.json` into `inputDir` (the job's `in/` directory, which must
    /// exist) for a render of `timeline` on `branch` at each time of `at` (rational seconds),
    /// and resolves to `{rev, planSamples}` JSON: the rev snapshotted and the plan hash at
    /// every time.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn prepare_media_job<'e>(
        &self,
        env: &'e Env,
        branch: String,
        timeline: String,
        at: Vec<String>,
        input_dir: String,
        options: Option<PrepareMediaOptions>,
    ) -> Result<Object<'e>> {
        let store = self.inner.clone();
        let (font_cache, params_json) =
            options.map_or((None, None), |o| (o.font_cache, o.params_json));
        let req = Request {
            branch,
            timeline,
            at,
            input_dir: input_dir.into(),
            font_cache: font_cache.map(PathBuf::from),
            params_json,
        };
        self.offload(env, move || prepare(&store, req))
    }
}

/// [`fin_shm::commit_output`] on the libuv pool.
pub struct CommitTask {
    from: PathBuf,
    to: PathBuf,
    cap: u64,
    copy: bool,
}

impl Task for CommitTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<()> {
        let (from, to, cap): (&Path, &Path, u64) = (&self.from, &self.to, self.cap);
        let copy = self.copy;
        guarded(
            || {
                if copy {
                    fin_shm::commit_output_copy(from, to, cap)
                } else {
                    fin_shm::commit_output(from, to, cap)
                }
                .map_err(reject)
            },
            || {},
        )
    }

    fn resolve(&mut self, _env: Env, _output: ()) -> Result<()> {
        Ok(())
    }
}

/// Moves the finished file `from` (inside a job directory, absolute) to `to` (absolute; its
/// directory must exist), replacing it, with `maxBytes` as the cap. By default
/// `fin_shm::commit_output` (a rename: the destination is the writer's own inode, so call it
/// only once the worker has exited); with `copy`, `fin_shm::commit_output_copy`: the bytes of
/// the checked handle go into a temporary file the host creates beside `to` and that is renamed
/// over it, so a worker that is still alive (warm) never holds the destination.
/// Rejects with `SHM_REFUSED {file, reason}` when `from` is a link, not a regular file, hard
/// linked or over the cap, or when the arrival is not the file that was checked.
#[napi(catch_unwind)]
pub fn commit_job_output(
    from: String,
    to: String,
    max_bytes: f64,
    copy: Option<bool>,
) -> AsyncTask<CommitTask> {
    AsyncTask::new(CommitTask {
        from: from.into(),
        to: to.into(),
        cap: if max_bytes.is_finite() && max_bytes >= 0.0 {
            max_bytes as u64
        } else {
            0
        },
        copy: copy.unwrap_or(false),
    })
}
