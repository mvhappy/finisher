//! The inputs of a `media.listen` job (plan 6C Task 21, Decision 21): on the engine's worker
//! thread, snapshots a branch head, samples the audio plan of every 10 s window of the range
//! (`fin_media::jobs::listen_plan_samples`, the worker's own computation) and writes
//! `snapshot.json`, `manifest.json` (the assets the range's audio plan references, once each in id
//! order) and, when given the job's params, `params.json` with the samples filled. Resolves to
//! `{rev, planSamples, sources}` as `prepareMediaJob` does. The kind then lists the `pcm48`
//! entries the cache holds for those assets (`MediaCache.attachPcm`).

use std::collections::BTreeSet;
use std::path::PathBuf;

use fin_core::eval::eval_audio;
use fin_core::view::timeline_end;
use fin_core::{CoreError, Project, Rational};
use fin_media::jobs::{
    JobSnapshot, ListenParams, MANIFEST_VERSION, MAX_PCM_SECONDS, ManifestAsset, ManifestFonts,
    PlanSample, RenderManifest, listen_plan_samples,
};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Serialize;
use serde_json::json;

use super::inputs;
use crate::engine::{Engine, Shared, with_store};
use crate::{encode, reject};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Prepared {
    rev: u64,
    plan_samples: Vec<PlanSample>,
    /// The originals the manifest names, for the worker's grants.
    sources: Vec<String>,
}

struct Request {
    branch: String,
    timeline: String,
    from: String,
    to: String,
    input_dir: PathBuf,
    params_json: Option<String>,
    /// The rev the caller read the timeline at.
    rev: Option<u64>,
}

fn time(field: &str, s: &str) -> Result<Rational> {
    s.parse::<Rational>().map_err(|e: CoreError| {
        reject(
            CoreError::new(
                "INVALID_TIME",
                format!("{field} is not a time: {}", e.message),
            )
            .with_details(json!({ "field": field, "value": s }))
            .with_fix("send a rational number of seconds such as \"12\" or \"1001/30000\""),
        )
    })
}

/// The range against the snapshot (6C Task 21 review S1): within `[0, end of the timeline]` and at
/// most `MAX_PCM_SECONDS` long, before any window is sampled (`INVALID_RANGE`). A timeline gone
/// from a head that moved past the rev the caller read is `CONFLICT` (as `evalAudio`).
fn check_range(
    project: &Project,
    timeline: &str,
    from: Rational,
    to: Rational,
    head: u64,
    read: Option<u64>,
) -> Result<()> {
    let Some(tl) = project.timelines.iter().find(|t| t.id == timeline) else {
        if let Some(read) = read.filter(|r| *r != head) {
            return Err(reject(
                CoreError::new(
                    "CONFLICT",
                    format!("timeline {timeline} is not on the head (rev {head}) though it was read at rev {read}"),
                )
                .with_details(json!({ "reason": "TIMELINE_GONE", "timeline": timeline, "rev": read, "head": head })),
            ));
        }
        return Err(reject(
            CoreError::new("NOT_FOUND", format!("timeline {timeline} does not exist"))
                .with_details(json!({ "id": timeline, "kind": "timeline" })),
        ));
    };
    let end = timeline_end(tl).map_err(reject)?;
    let longest = Rational::from_int(MAX_PCM_SECONDS as i64);
    let too_long = !to.checked_sub(from).is_ok_and(|len| len <= longest);
    if from.is_negative() || to > end || to <= from || too_long {
        return Err(reject(
            CoreError::new(
                "INVALID_RANGE",
                format!("the range {from} to {to} is not within timeline {timeline} (0 to {end} seconds)"),
            )
            .with_details(json!({ "from": from, "to": to, "timeline": timeline, "duration": end }))
            .with_fix(format!("listen to a range within 0 to {end}, at most {MAX_PCM_SECONDS} s long")),
        ));
    }
    Ok(())
}

fn prepare(store: &Shared, req: Request) -> Result<String> {
    let (from, to) = (time("from", &req.from)?, time("to", &req.to)?);
    let params = req
        .params_json
        .as_deref()
        .map(|j| {
            let p: ListenParams = serde_json::from_str(j).map_err(|e| {
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
    check_range(&project, &req.timeline, from, to, rev, req.rev)?;
    let plan = eval_audio(&project, &req.timeline, from, to).map_err(reject)?;
    let samples = listen_plan_samples(&project, &req.timeline, from, to).map_err(reject)?;
    let ids: BTreeSet<&str> = plan.items.iter().map(|i| i.asset.as_str()).collect();
    let assets: Vec<ManifestAsset> = ids
        .into_iter()
        .filter_map(|id| {
            let a = project.assets.get(id)?;
            Some(ManifestAsset {
                asset: id.to_string(),
                kind: a.kind,
                fingerprint: a.fingerprint.clone(),
                original: inputs::original_file(&inputs::resolve(&project_dir, &a.path)),
                proxies: Vec::new(),
            })
        })
        .collect();
    let manifest = RenderManifest {
        version: MANIFEST_VERSION,
        assets,
        fonts: ManifestFonts::default(),
        pcm: Default::default(),
    };
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
        .collect();
    sources.sort();
    sources.dedup();
    encode(&Prepared {
        rev,
        plan_samples: samples,
        sources,
    })
}

#[napi]
impl Engine {
    /// Writes `snapshot.json`, `manifest.json` and (with `paramsJson`, the job's `ListenParams`)
    /// `params.json` into `inputDir` (the job's `in/`, which must exist) for a `media.listen` of
    /// `[from, to)` (rational seconds) of `timeline` on `branch`, and resolves to `{rev,
    /// planSamples, sources}` JSON. Rejects `INVALID_RANGE` for a range past the timeline's end
    /// or longer than a day, `NOT_FOUND` for an unknown timeline, `CONFLICT` for one gone from a
    /// head past `rev` (the rev the caller read).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_listen_job<'e>(
        &self,
        env: &'e Env,
        branch: String,
        timeline: String,
        from: String,
        to: String,
        input_dir: String,
        params_json: Option<String>,
        rev: Option<f64>,
    ) -> Result<Object<'e>> {
        let store = self.inner.clone();
        let req = Request {
            branch,
            timeline,
            from,
            to,
            input_dir: input_dir.into(),
            params_json,
            rev: rev.filter(|r| r.is_finite() && *r >= 0.0).map(|r| r as u64),
        };
        self.offload(env, move || prepare(&store, req))
    }
}
