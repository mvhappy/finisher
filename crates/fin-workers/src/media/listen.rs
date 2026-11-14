//! `media.listen` v1 (plan 6C Task 21, Decision 21; sub-project 6 spec §8.7): loudness, peaks,
//! clipping, silences and the loudest moments of a range of a timeline, per track and for the
//! mix, cold.
//!
//! 1. **The plan** comes from `in/snapshot.json` only: every 10 s window of the range is
//!    evaluated (`eval_audio`) and its BLAKE3 compared with the host's plan sample
//!    (`PLAN_MISMATCH` on any difference or a missing sample), then the whole range's plan is
//!    the one mixed.
//! 2. **Sources**, decided once per asset for the whole job: an asset with a `pcm48` entry in the
//!    manifest's `pcm` that verifies (every chunk's size and BLAKE3) is read from it
//!    ([`CachedSource`], one per mix: random access); otherwise its original is decoded
//!    (`fin-library`'s no-follow opener, then `AudioReader` → canonical → 48 kHz) through a
//!    **pool of decoders per asset** within a mix: a [`DecodedSource`] reads forward only, so an
//!    item takes, at its first read, an idle decoder not past it (opening one only when none is
//!    free), and gives it back when it ends. An entry that verified but no longer opens falls
//!    back to the decode. An asset with neither is silent and noted `missingSource {asset,
//!    reason}`.
//! 3. **The mixes:** the master bus and one per track (that track's items alone; a nested
//!    timeline's items count for the track their compound clip is on), each a `fin_audio::listen`
//!    pass, rendered in lockstep in blocks of one second: progress after each, cancel between
//!    them. Nothing is normalised: `fin listen` reports, it does not change.
//! 4. **Notes:** `STRETCH_EXTREME {item, speed}` for an item WSOLA cannot do well,
//!    `skippedEffect {item, effect, kind}` for an effect this engine does not know.
//!
//! Nothing is written to `out/`; the answer is the job's result ([`ListenResult`]).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use fin_audio::channels::Canonical;
use fin_audio::listen::{Pass, filter_plan, range, stretch_extreme};
use fin_audio::{PcmSource, SourceLayout};
use fin_core::eval::{AudioItem, AudioPlan, eval_audio};
use fin_core::events::payloads::RenderPhase;
use fin_core::{CoreError, Project, Rational};
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, Progress, ProgressUnit, StartJob};
use fin_media::jobs::{
    JobSnapshot, ListenJobStats, ListenNote, ListenParams, ListenResult, ListenTrack,
    MANIFEST_VERSION, OriginalFile, PcmLayout, PcmRecipe, RenderManifest, audio_plan_hash,
    listen_windows,
};
use serde_json::{Value, json};

use super::audio_src::{CachedSource, DecodedSource};
use super::diag::{AudioFacts, audio_codec_name, audio_media};
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

/// What an asset is read from, decided once for the whole job (every mix reads it the same way).
#[derive(Clone)]
enum Verdict {
    /// Its `pcm48` entry, verified.
    Cached,
    /// Its original, decoded; the feed's layout.
    Decoded(PcmLayout),
    /// Neither: the opener's code.
    Missing(String),
}

/// How much decoding the job did: decoders opened, and output frames decoded (each decoder
/// decodes from frame 0 to the furthest end it was read to).
#[derive(Default)]
struct Counters {
    opened: Cell<u32>,
    decoded: Cell<u64>,
}

/// One decoder of a pool: the frame its next read may start at, the furthest end read, and
/// whether a live item holds it.
struct Pooled {
    src: DecodedSource,
    pos: i64,
    furthest: i64,
    busy: bool,
}

/// One asset's decoders within a mix. An item takes, at its first read, the idle decoder that is
/// furthest along but not past that read (a `DecodedSource` reads forward only), and opens a new
/// one only when every decoder is past it or held; it gives it back when the mixer drops it (its
/// item has ended). So cuts of one original at increasing offsets walk one decoder forward: the
/// decode work is about the source length times the backward jumps, not the sum of the offsets.
struct Pool {
    original: OriginalFile,
    confine: Option<PathBuf>,
    layout: PcmLayout,
    decoders: Vec<Pooled>,
    counters: Rc<Counters>,
}

impl Pool {
    /// A decoder for a first read at `at`, marked held.
    fn take(&mut self, at: i64) -> Result<usize, CoreError> {
        let at = at.max(0);
        let best = (0..self.decoders.len())
            .filter(|&i| !self.decoders[i].busy && self.decoders[i].pos <= at)
            .max_by_key(|&i| self.decoders[i].pos);
        let i = match best {
            Some(i) => i,
            None => {
                let file =
                    open_original_in(&self.original, self.confine.as_deref()).map_err(|r| {
                        CoreError::new("ORIGINALS_MISSING", r.message)
                            .with_details(json!({ "reason": r.reason }))
                    })?;
                let src = DecodedSource::open(file, PcmRecipe::Pcm48)?;
                if src.feed_layout() != self.layout {
                    return Err(CoreError::new(
                        "MEDIA_CORRUPT",
                        "the original's channel layout changed during the listen",
                    )
                    .with_details(json!({ "reason": "channelsChange" })));
                }
                self.counters.opened.set(self.counters.opened.get() + 1);
                self.decoders.push(Pooled {
                    src,
                    pos: 0,
                    furthest: 0,
                    busy: false,
                });
                self.decoders.len() - 1
            }
        };
        self.decoders[i].busy = true;
        Ok(i)
    }
}

/// An item's decoded original: a decoder of its asset's pool, taken at the item's first read and
/// given back when the mixer drops the source (once the item has ended).
struct PooledSource {
    pool: Rc<RefCell<Pool>>,
    layout: PcmLayout,
    slot: Option<usize>,
}

impl PcmSource for PooledSource {
    fn layout(&self) -> SourceLayout {
        match self.layout {
            PcmLayout::Mono => SourceLayout::Mono,
            PcmLayout::Stereo => SourceLayout::Stereo,
        }
    }

    fn read(&mut self, at: i64, frames: usize, out: &mut Canonical) -> Result<(), CoreError> {
        let mut pool = self.pool.borrow_mut();
        let i = match self.slot {
            Some(i) => i,
            None => {
                let i = pool.take(at)?;
                self.slot = Some(i);
                i
            }
        };
        let counters = pool.counters.clone();
        let d = &mut pool.decoders[i];
        let end = at.saturating_add(frames as i64);
        if end > d.furthest {
            counters
                .decoded
                .set(counters.decoded.get() + (end - d.furthest) as u64);
            d.furthest = end;
        }
        d.src.read(at, frames, out)?;
        d.pos = d.pos.max(at);
        Ok(())
    }
}

impl Drop for PooledSource {
    fn drop(&mut self) {
        if let Some(i) = self.slot
            && let Ok(mut pool) = self.pool.try_borrow_mut()
        {
            pool.decoders[i].busy = false;
        }
    }
}

/// Opens the sources of the job's mixes (module docs, step 2), deciding each asset once.
struct Opener<'a> {
    manifest: &'a RenderManifest,
    confine: Option<&'a Path>,
    verdicts: BTreeMap<String, Verdict>,
    counters: Rc<Counters>,
    /// The first decoded original's codec and backend (`job.done.media`).
    decoded_via: Option<(&'static str, &'static str)>,
}

impl Opener<'_> {
    /// Whether the asset's `pcm48` entry is usable: every chunk verified (one read of each: a
    /// chunk that fails its size or BLAKE3 would otherwise surface mid-mix).
    fn entry_verifies(&self, asset: &str) -> bool {
        let Some(entry) = self.manifest.pcm.get(asset) else {
            return false;
        };
        let Ok(mut src) = CachedSource::open(Path::new(&entry.dir), &entry.key) else {
            return false;
        };
        let (per, frames) = (src.entry().layout.chunk_frames(), src.entry().frames);
        let mut one = Canonical::Mono(Vec::new());
        let mut at = 0u64;
        while at < frames {
            if src.read(at as i64, 1, &mut one).is_err() {
                return false;
            }
            at += per;
        }
        true
    }

    fn original(&self, asset: &str) -> Result<&OriginalFile, String> {
        self.manifest
            .asset(asset)
            .and_then(|a| a.original.as_ref())
            .ok_or_else(|| "ORIGINALS_MISSING".to_string())
    }

    /// The asset's verdict, decided on first ask.
    fn verdict(&mut self, asset: &str) -> Verdict {
        if let Some(v) = self.verdicts.get(asset) {
            return v.clone();
        }
        let v = if self.entry_verifies(asset) {
            Verdict::Cached
        } else {
            let opened = self.original(asset).and_then(|o| {
                let file = open_original_in(o, self.confine)
                    .map_err(|_| "ORIGINALS_MISSING".to_string())?;
                DecodedSource::open(file, PcmRecipe::Pcm48).map_err(|e| e.code)
            });
            match opened {
                Ok(src) => {
                    self.note_decoder(&src);
                    Verdict::Decoded(src.feed_layout())
                }
                Err(reason) => Verdict::Missing(reason),
            }
        };
        self.verdicts.insert(asset.to_string(), v.clone());
        v
    }

    /// The asset decided from its original (a cached entry that verified but no longer opens
    /// falls back here): `Decoded` or `Missing`, recorded for the job.
    fn decode_verdict(&mut self, asset: &str) -> Verdict {
        let opened = self.original(asset).and_then(|o| {
            let file =
                open_original_in(o, self.confine).map_err(|_| "ORIGINALS_MISSING".to_string())?;
            DecodedSource::open(file, PcmRecipe::Pcm48).map_err(|e| e.code)
        });
        let v = match opened {
            Ok(src) => {
                self.note_decoder(&src);
                Verdict::Decoded(src.feed_layout())
            }
            Err(reason) => Verdict::Missing(reason),
        };
        self.verdicts.insert(asset.to_string(), v.clone());
        v
    }

    fn note_decoder(&mut self, src: &DecodedSource) {
        self.decoded_via
            .get_or_insert((audio_codec_name(src.info().codec), src.backend()));
    }

    /// `plan` as one mix reads it, and its sources: an asset's `pcm48` entry once (random
    /// access); a decoded original through its asset's [`Pool`], one source per item (the
    /// item's `asset` is renamed `<asset>\0<k>` for it: a NUL is in no asset id); an asset that
    /// cannot be read is absent (silent, and noted).
    fn sources(&mut self, plan: &AudioPlan) -> (AudioPlan, BTreeMap<String, Box<dyn PcmSource>>) {
        let mut plan = plan.clone();
        let mut out: BTreeMap<String, Box<dyn PcmSource>> = BTreeMap::new();
        let mut pools: BTreeMap<String, Rc<RefCell<Pool>>> = BTreeMap::new();
        for (k, item) in plan.items.iter_mut().enumerate() {
            let mut verdict = self.verdict(&item.asset);
            if matches!(verdict, Verdict::Cached) {
                if out.contains_key(&item.asset) {
                    continue;
                }
                let entry = &self.manifest.pcm[&item.asset];
                match CachedSource::open(Path::new(&entry.dir), &entry.key) {
                    Ok(src) => {
                        out.insert(item.asset.clone(), Box::new(src));
                        continue;
                    }
                    Err(_) => verdict = self.decode_verdict(&item.asset),
                }
            }
            let Verdict::Decoded(layout) = verdict else {
                continue;
            };
            let Ok(original) = self.original(&item.asset).cloned() else {
                continue;
            };
            let pool = pools.entry(item.asset.clone()).or_insert_with(|| {
                Rc::new(RefCell::new(Pool {
                    original,
                    confine: self.confine.map(Path::to_path_buf),
                    layout,
                    decoders: Vec::new(),
                    counters: self.counters.clone(),
                }))
            });
            let key = format!("{}\0{k}", item.asset);
            out.insert(
                key.clone(),
                Box::new(PooledSource {
                    pool: pool.clone(),
                    layout,
                    slot: None,
                }),
            );
            item.asset = key;
        }
        (plan, out)
    }

    /// Decoders opened so far, over every mix.
    #[cfg(test)]
    fn decoders_opened(&self) -> u32 {
        self.counters.opened.get()
    }

    /// Output frames decoded so far, over every mix.
    #[cfg(test)]
    fn frames_decoded(&self) -> u64 {
        self.counters.decoded.get()
    }

    fn missing(&self) -> BTreeMap<String, String> {
        self.verdicts
            .iter()
            .filter_map(|(a, v)| match v {
                Verdict::Missing(r) => Some((a.clone(), r.clone())),
                _ => None,
            })
            .collect()
    }

    fn count(&self, cached: bool) -> u32 {
        self.verdicts
            .values()
            .filter(|v| {
                matches!(
                    (v, cached),
                    (Verdict::Cached, true) | (Verdict::Decoded(_), false)
                )
            })
            .count() as u32
    }
}

/// Test builds only, after each block: an asset whose fingerprint starts `test-slow-` makes the
/// worker wait 300 ms, `test-crash-` aborts it (a listen that runs long enough to cancel, and one
/// that fails: the pins' release on every end).
#[cfg(feature = "test-hooks")]
fn test_block(manifest: &RenderManifest) {
    for a in &manifest.assets {
        if a.fingerprint.starts_with("test-slow-") {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        if a.fingerprint.starts_with("test-crash-") {
            std::process::abort();
        }
    }
}

#[cfg(not(feature = "test-hooks"))]
fn test_block(_: &RenderManifest) {}

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
        verdicts: BTreeMap::new(),
        counters: Rc::default(),
        decoded_via: None,
    };
    let (mix_plan, mix_sources) = opener.sources(&plan);
    let mut mix = Pass::new(&mix_plan, mix_sources).map_err(job_error)?;
    let mut passes = Vec::with_capacity(tracks.len());
    for t in &tracks {
        let only = filter_plan(&plan, |i| &outer_track(project, &p.timeline, i) == t);
        let (only, sources) = opener.sources(&only);
        passes.push(Pass::new(&only, sources).map_err(job_error)?);
    }
    let (start, end) = range(&plan).map_err(job_error)?;
    let total = (end - start) as u64;
    let mut done = 0u64;
    let mixing = std::time::Instant::now();
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
        test_block(&manifest);
        let mut pr = Progress::new(done as f64, ProgressUnit::Frames);
        pr.total = Some(total as f64);
        pr.phase = Some("mix".into());
        ctx.progress(pr);
    }
    let notes = notes_of(&plan, &mix, &opener.missing());
    let mut stats = ListenJobStats {
        sources_from_cache: opener.count(true),
        sources_decoded: opener.count(false),
        media: None,
    };
    if ctx.diagnostics() {
        let micros = u64::try_from(mixing.elapsed().as_micros()).unwrap_or(u64::MAX);
        // The whole pass (lazy decodes included) is one `Mix` sample, filed under the first source
        // that was decoded rather than read from pcm48 (`none` when every source was cached): a
        // render.stats row has one backend, and a multi-codec mix has no better single one.
        let (codec, backend) = opener.decoded_via.unwrap_or(("pcm", "none"));
        warm.render_stats.record(RenderPhase::Mix, backend, micros);
        stats.media = Some(audio_media(&AudioFacts {
            codec,
            backend,
            samples: total,
            rate: 48_000,
            wall_micros: micros,
            cache_hit: stats.sources_decoded == 0 && stats.sources_from_cache > 0,
        }));
    }
    let result = ListenResult {
        timeline: p.timeline.clone(),
        rev: snapshot.rev,
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

#[cfg(test)]
#[path = "listen_tests.rs"]
mod tests;
