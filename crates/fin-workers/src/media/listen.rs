//! `media.listen` v1 (plan 6C Task 21, Decision 21; sub-project 6 spec §8.7): loudness, peaks,
//! clipping, silences and the loudest moments of a range of a timeline, per track and for the
//! mix, cold.
//!
//! 1. **The plan** comes from `in/snapshot.json` only: every 10 s window of the range is
//!    evaluated (`eval_audio`) and its BLAKE3 compared with the host's plan sample
//!    (`PLAN_MISMATCH` on any difference or a missing sample), then the whole range's plan is
//!    the one mixed.
//! 2. **Sources**, one set per mix (a [`DecodedSource`] reads forward only): an asset with a
//!    `pcm48` entry in the manifest's `pcm` is read from it ([`CachedSource`], which recomputes
//!    its key and checks every chunk); otherwise, or when the entry does not verify, its original
//!    is decoded (`fin-library`'s no-follow opener, then `AudioReader` → canonical → 48 kHz). An
//!    asset with neither is silent and noted `missingSource {asset, reason}`.
//! 3. **The mixes:** the master bus and one per track (that track's items alone; a nested
//!    timeline's items count for the track their compound clip is on), each a `fin_audio::listen`
//!    pass, rendered in lockstep in blocks of one second: progress after each, cancel between
//!    them. Nothing is normalised: `fin listen` reports, it does not change.
//! 4. **Notes:** `STRETCH_EXTREME {item, speed}` for an item WSOLA cannot do well,
//!    `skippedEffect {item, effect, kind}` for an effect this engine does not know.
//!
//! Nothing is written to `out/`; the answer is the job's result ([`ListenResult`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use fin_audio::PcmSource;
use fin_audio::channels::Canonical;
use fin_audio::listen::{Pass, filter_plan, range, stretch_extreme};
use fin_core::eval::{AudioItem, AudioPlan, eval_audio};
use fin_core::{Project, Rational};
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, Progress, ProgressUnit, StartJob};
use fin_media::jobs::{
    JobSnapshot, ListenJobStats, ListenNote, ListenParams, ListenResult, ListenTrack,
    MANIFEST_VERSION, PcmRecipe, RenderManifest, audio_plan_hash, listen_windows,
};
use serde_json::{Value, json};

use super::audio_src::{CachedSource, DecodedSource};
use super::inputs::{self, MANIFEST_CAP, SNAPSHOT_CAP};
use super::open_cache::open_original_in;
use super::{Warm, job_error};

/// Frames mixed between two progress reports (and cancel checks): one second.
const BLOCK: usize = 48_000;

fn mismatch(t: Rational, want: Option<&str>, got: &str) -> JobError {
    let reason = if want.is_some() { "hash" } else { "noSample" };
    JobError::new(
        "PLAN_MISMATCH",
        format!("the worker's audio plan at {t} differs from the host's ({reason})"),
    )
    .with_details(json!({ "at": t, "reason": reason, "want": want, "got": got }))
}

/// Every window's plan against its sample (module docs, step 1).
fn check_samples(project: &Project, p: &ListenParams) -> Result<(), JobError> {
    for (a, b) in listen_windows(p.from, p.to).map_err(job_error)? {
        let hash = audio_plan_hash(&eval_audio(project, &p.timeline, a, b).map_err(job_error)?);
        let want = p.sample(a).map(|s| s.blake3.as_str());
        if want != Some(hash.as_str()) {
            return Err(mismatch(a, want, &hash));
        }
    }
    Ok(())
}

/// The track of the measured timeline an item sounds on: its own, or for a nested timeline's
/// item, the track of the compound clip it sounds through (`via[0]`).
fn outer_track(project: &Project, timeline: &str, item: &AudioItem) -> String {
    let Some(outer) = item.via.first() else {
        return item.track.clone();
    };
    project
        .timelines
        .iter()
        .find(|t| t.id == timeline)
        .and_then(|t| {
            t.tracks
                .iter()
                .find(|tr| tr.items.iter().any(|i| &i.id == outer))
        })
        .map_or_else(|| item.track.clone(), |tr| tr.id.clone())
}

/// The tracks with items in `plan`, in the timeline's order (any the timeline does not list
/// after them, in plan order).
fn tracks_of(project: &Project, timeline: &str, plan: &AudioPlan) -> Vec<String> {
    let used: Vec<String> = plan
        .items
        .iter()
        .map(|i| outer_track(project, timeline, i))
        .collect();
    let mut out: Vec<String> = project
        .timelines
        .iter()
        .find(|t| t.id == timeline)
        .map(|t| {
            t.tracks
                .iter()
                .filter(|tr| used.contains(&tr.id))
                .map(|tr| tr.id.clone())
                .collect()
        })
        .unwrap_or_default();
    for t in used {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Opens the sources of the job's mixes (module docs, step 2), counting each asset once.
struct Opener<'a> {
    manifest: &'a RenderManifest,
    confine: Option<&'a Path>,
    from_cache: BTreeSet<String>,
    decoded: BTreeSet<String>,
    missing: BTreeMap<String, String>,
}

impl Opener<'_> {
    /// The asset's `pcm48` entry, every chunk verified first (one read of each: a chunk that
    /// fails its size or BLAKE3 would otherwise surface mid-mix), or `None` when the manifest
    /// lists none or it does not verify.
    fn cached(&self, asset: &str) -> Option<Box<dyn PcmSource>> {
        let entry = self.manifest.pcm.get(asset)?;
        let mut src = CachedSource::open(Path::new(&entry.dir), &entry.key).ok()?;
        let (per, frames) = (src.entry().layout.chunk_frames(), src.entry().frames);
        let mut one = Canonical::Mono(Vec::new());
        let mut at = 0u64;
        while at < frames {
            src.read(at as i64, 1, &mut one).ok()?;
            at += per;
        }
        Some(Box::new(src))
    }

    fn decoded(&self, asset: &str) -> Result<Box<dyn PcmSource>, String> {
        let a = self
            .manifest
            .asset(asset)
            .ok_or_else(|| "ORIGINALS_MISSING".to_string())?;
        let original = a
            .original
            .as_ref()
            .ok_or_else(|| "ORIGINALS_MISSING".to_string())?;
        let file = open_original_in(original, self.confine)
            .map_err(|_| "ORIGINALS_MISSING".to_string())?;
        let src = DecodedSource::open(file, PcmRecipe::Pcm48).map_err(|e| e.code)?;
        Ok(Box::new(src))
    }

    /// The sources of `plan`'s assets, by asset id (an asset that cannot be read is absent).
    fn sources(&mut self, plan: &AudioPlan) -> BTreeMap<String, Box<dyn PcmSource>> {
        let assets: BTreeSet<&str> = plan.items.iter().map(|i| i.asset.as_str()).collect();
        let mut out = BTreeMap::new();
        for asset in assets {
            if self.missing.contains_key(asset) {
                continue;
            }
            if let Some(src) = self.cached(asset) {
                self.from_cache.insert(asset.to_string());
                out.insert(asset.to_string(), src);
                continue;
            }
            match self.decoded(asset) {
                Ok(src) => {
                    self.decoded.insert(asset.to_string());
                    out.insert(asset.to_string(), src);
                }
                Err(reason) => {
                    self.missing.insert(asset.to_string(), reason);
                }
            }
        }
        out
    }
}

fn notes_of(plan: &AudioPlan, mix: &Pass, missing: &BTreeMap<String, String>) -> Vec<ListenNote> {
    let mut notes: Vec<ListenNote> = missing
        .iter()
        .map(|(asset, reason)| ListenNote::MissingSource {
            asset: asset.clone(),
            reason: reason.clone(),
        })
        .collect();
    let mut seen = BTreeSet::new();
    for it in &plan.items {
        if stretch_extreme(it.speed) && seen.insert(it.item.clone()) {
            notes.push(ListenNote::StretchExtreme {
                item: it.item.clone(),
                speed: it.speed,
            });
        }
    }
    notes.extend(
        mix.mixer()
            .skipped_effects()
            .iter()
            .map(|s| ListenNote::SkippedEffect {
                item: s.item.clone(),
                effect: s.effect.clone(),
                kind: s.kind.clone(),
            }),
    );
    notes
}

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: &mut Warm) -> Result<Value, JobError> {
    let mut p: ListenParams = serde_json::from_value(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("params: {e}")))?;
    p.check().map_err(job_error)?;
    // As `media.look`: the samples `prepare` computed win over any in the params.
    if let Some(samples) = job.extra.as_ref().and_then(|e| e.get("planSamples")) {
        p.plan_samples = serde_json::from_value(samples.clone())
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
    let project = &snapshot.project;
    check_samples(project, &p)?;
    let plan = eval_audio(project, &p.timeline, p.from, p.to).map_err(job_error)?;
    let tracks = tracks_of(project, &p.timeline, &plan);
    let mut opener = Opener {
        manifest: &manifest,
        confine: warm.sources.confined(),
        from_cache: BTreeSet::new(),
        decoded: BTreeSet::new(),
        missing: BTreeMap::new(),
    };
    let mix_sources = opener.sources(&plan);
    let mut mix = Pass::new(&plan, mix_sources).map_err(job_error)?;
    let mut passes = Vec::with_capacity(tracks.len());
    for t in &tracks {
        let only = filter_plan(&plan, |i| &outer_track(project, &p.timeline, i) == t);
        let sources = opener.sources(&only);
        passes.push(Pass::new(&only, sources).map_err(job_error)?);
    }
    let (start, end) = range(&plan).map_err(job_error)?;
    let total = (end - start) as u64;
    let mut done = 0u64;
    while done < total {
        if ctx.cancelled().is_some() {
            return Err(JobError::cancelled());
        }
        let n = (total - done).min(BLOCK as u64) as usize;
        mix.render(n).map_err(job_error)?;
        for pass in &mut passes {
            pass.render(n).map_err(job_error)?;
        }
        done += n as u64;
        let mut pr = Progress::new(done as f64, ProgressUnit::Frames);
        pr.total = Some(total as f64);
        pr.phase = Some("mix".into());
        ctx.progress(pr);
    }
    let notes = notes_of(&plan, &mix, &opener.missing);
    let stats = ListenJobStats {
        sources_from_cache: opener.from_cache.len() as u32,
        sources_decoded: opener.decoded.len() as u32,
    };
    let result = ListenResult {
        timeline: p.timeline.clone(),
        from: p.from,
        to: p.to,
        mix: mix.finish(),
        tracks: tracks
            .into_iter()
            .zip(passes)
            .map(|(track, pass)| ListenTrack {
                track,
                stats: pass.finish(),
            })
            .collect(),
        notes,
        stats,
    };
    Ok(serde_json::to_value(result).expect("a result serialises"))
}
