//! `media.waveform` v1 (plan 6C Task 19, Decisions 7, 19, 20 and 23; sub-project 6 spec §7.2):
//! a source's waveform peaks as a `wave` cache entry (`peaks.bin`), cold, a batch of one item
//! (the fingerprint).
//!
//! 1. **The asset** is named by `in/manifest.json` (its fingerprint must be the one asked for:
//!    `ASSET_CHANGED`).
//! 2. **The signal** is the source's 48 kHz canonical feed, a mono source as dual mono (the
//!    peaks are per channel of the stereo signal): from the source's `pcm48` entry when the host
//!    found one (`params.pcmKey`; `in/pcm.json {key, dir}` names its directory), read through
//!    [`CachedSource`] (which checks the entry's key, sizes and digests), else, and whenever that
//!    entry is not usable, decoded from the original through [`DecodedSource`] and 6A's
//!    no-follow opener (`ORIGINALS_MISSING {reason}`). Both feeds are the same samples, so the
//!    bytes are the same. The decoder class of the key is the entry's, or the decoder's; another
//!    than the host planned is `PLAN_MISMATCH {reason: cacheKey}`, before anything is written.
//! 3. **The peaks** are built a block at a time (a multiple of the coarsest level's 65,536
//!    samples), so a long source never sits in memory whole: the levels of consecutive blocks
//!    concatenate to those of the whole signal exactly.
//!
//! Retries restart from sample 0 (Decision 23); the entry is one small file written once.

use std::path::Path;

use fin_audio::channels::Canonical;
use fin_audio::{PcmSource, Peaks, Stereo};
use fin_core::CoreError;
use fin_core::events::payloads::RenderPhase;
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, ProgressUnit, StartJob};
use fin_media::backend::VendorClass;
use fin_media::cache::{EntryMeta, META_FILE};
use fin_media::jobs::{
    PEAKS_FILE, PcmRecipe, WaveFrom, WaveParams, WaveResult, wave_key, wave_key_input,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::audio_src::{CachedSource, DecodedSource};
use super::diag::{AudioFacts, audio_codec_name, audio_media};
use super::proxy::{Fail, entry_file, item, manifest_asset, now_ms, open_asset, write_file};
use super::{Warm, inputs, job_error};

/// Frames per block: a multiple of the coarsest level's samples per peak.
const BLOCK: usize = 8 * 65_536;
/// The most `in/pcm.json` read.
const PCM_JSON_CAP: u64 = 64 << 10;

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: &mut Warm) -> Result<Value, JobError> {
    let p: WaveParams = serde_json::from_value(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("params: {e}")))?;
    p.check().map_err(job_error)?;
    let fp = p.fingerprint.clone();
    ctx.begin(&fp);
    let started = std::time::Instant::now();
    match make(&p, ctx, warm, job.attempt) {
        Ok((result, (codec, backend))) => {
            let value = serde_json::to_value(&result)
                .map_err(|e| job_error(CoreError::new("INTERNAL", e.to_string())))?;
            ctx.item(&fp, Some(value));
            if !ctx.diagnostics() {
                return Ok(json!({ "made": 1 }));
            }
            let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            warm.render_stats
                .record(RenderPhase::DecodeAudio, backend, micros);
            let media = audio_media(&AudioFacts {
                codec,
                backend,
                samples: result.frames,
                rate: 48_000,
                wall_micros: micros,
                cache_hit: result.from == WaveFrom::Pcm48,
            });
            Ok(json!({ "made": 1, "media": media }))
        }
        Err(Fail::Item(e)) => {
            ctx.item_failed(&fp, e);
            Ok(json!({ "made": 0 }))
        }
        Err(Fail::Job(e)) => Err(e),
    }
}

/// Peaks built block by block (module docs).
#[derive(Default)]
struct Builder {
    l: Vec<f32>,
    r: Vec<f32>,
    done: Option<Peaks>,
}

impl Builder {
    /// Appends frames (a mono block as dual mono).
    fn push(&mut self, c: &Canonical) {
        match c {
            Canonical::Mono(m) => {
                self.l.extend_from_slice(m);
                self.r.extend_from_slice(m);
            }
            Canonical::Stereo(s) => {
                self.l.extend_from_slice(&s.l);
                self.r.extend_from_slice(&s.r);
            }
        }
    }

    fn absorb(&mut self, block: Peaks) {
        match &mut self.done {
            None => self.done = Some(block),
            Some(all) => {
                all.frames += block.frames;
                for (a, b) in all.levels.iter_mut().zip(block.levels) {
                    for (ac, bc) in a.channels.iter_mut().zip(b.channels) {
                        ac.extend(bc);
                    }
                }
            }
        }
    }

    /// Peaks of every whole block held; `all` also takes the last, short one.
    fn drain(&mut self, all: bool) {
        while self.l.len() >= BLOCK || (all && !self.l.is_empty()) {
            let n = self.l.len().min(BLOCK);
            let block = Stereo {
                l: self.l.drain(..n).collect(),
                r: self.r.drain(..n).collect(),
            };
            self.absorb(fin_audio::peaks(&block));
        }
    }

    fn finish(mut self) -> Option<Peaks> {
        self.drain(true);
        self.done
    }
}

/// `in/pcm.json`: where the `pcm48` entry is.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PcmRef {
    key: String,
    dir: String,
}

/// The source's `pcm48` entry, when `params.pcmKey` names one this worker can use: it must be
/// this asset's fingerprint, a `pcm48`, and recompute this job's key with its decoder class.
fn open_entry(
    p: &WaveParams,
    ctx: &JobCtx<'_>,
    dir: &fin_shm::JobDir,
) -> Option<(CachedSource, VendorClass)> {
    let key = p.pcm_key.as_deref()?;
    let r: PcmRef = inputs::read_json(ctx, dir, "in/pcm.json", PCM_JSON_CAP).ok()?;
    let dir = Path::new(&r.dir);
    if r.key != key || !dir.is_absolute() {
        return None;
    }
    let src = CachedSource::open(dir, key).ok()?;
    let meta = src.meta();
    let class = meta.decoder_class;
    let usable = src.entry().recipe == PcmRecipe::Pcm48
        && meta.fingerprint == p.fingerprint
        && wave_key(&p.fingerprint, class).as_str() == p.key;
    usable.then_some((src, class))
}

/// Test builds only (hosts clear a worker's environment, so the fault is named in the
/// fingerprint): `test-crashonce-` aborts the first attempt once the peaks are built.
#[cfg(feature = "test-hooks")]
fn test_built(fingerprint: &str, attempt: u32) {
    if fingerprint.starts_with("test-crashonce-") && attempt == 1 {
        std::process::abort();
    }
}

#[cfg(not(feature = "test-hooks"))]
fn test_built(_: &str, _: u32) {}

fn make(
    p: &WaveParams,
    ctx: &mut JobCtx<'_>,
    warm: &mut Warm,
    attempt: u32,
) -> Result<(WaveResult, (&'static str, &'static str)), Fail> {
    let dir = inputs::job_dir(ctx).map_err(Fail::Job)?;
    inputs::out_dir(ctx, &dir).map_err(Fail::Job)?;
    manifest_asset(ctx, &dir, &p.asset, &p.fingerprint)?;
    let mut builder = Builder::default();
    let progress = |ctx: &mut JobCtx<'_>, done: u64, total: Option<u64>| -> Result<(), Fail> {
        if ctx.cancelled().is_some() {
            return Err(Fail::Job(JobError::cancelled()));
        }
        let mut pr = fin_jobs::Progress::new(done as f64, ProgressUnit::Frames);
        pr.total = total.map(|t| t as f64);
        pr.phase = Some("decodeAudio".into());
        ctx.progress(pr);
        Ok(())
    };
    // The entry first; a chunk that does not verify while it is read sends the job to the decode.
    let mut entry = None;
    if let Some((mut src, class)) = open_entry(p, ctx, &dir) {
        let total = src.entry().frames;
        let mut at = 0u64;
        while at < total {
            let n = (total - at).min(BLOCK as u64) as usize;
            let mut c = Canonical::Mono(Vec::new());
            if src.read(at as i64, n, &mut c).is_err() {
                break;
            }
            builder.push(&c);
            builder.drain(false);
            at += n as u64;
            progress(ctx, at, Some(total))?;
        }
        if at == total {
            entry = Some(class);
        } else {
            builder = Builder::default();
        }
    }
    // What served the peaks: a feed read back from the cache decoded nothing.
    let mut served = ("pcm", "none");
    let (class, from) = if let Some(class) = entry {
        (class, WaveFrom::Pcm48)
    } else {
        let file = open_asset(ctx, &dir, warm, &p.asset, &p.fingerprint)?;
        let mut src = DecodedSource::open(file, PcmRecipe::Pcm48)?;
        let class = src.decoder_class();
        check_key(p, class)?;
        served = (audio_codec_name(src.info().codec), src.backend());
        let total = src.expected_frames();
        let mut done = 0u64;
        let channels = src.feed_layout().channels();
        while let Some(block) = src.next_interleaved()? {
            let frames = block.len() / channels;
            let c = if channels == 1 {
                Canonical::Mono(block)
            } else {
                Canonical::Stereo(Stereo {
                    l: block.iter().step_by(2).copied().collect(),
                    r: block.iter().skip(1).step_by(2).copied().collect(),
                })
            };
            builder.push(&c);
            builder.drain(false);
            done += frames as u64;
            progress(ctx, done, total)?;
        }
        (class, WaveFrom::Decode)
    };
    check_key(p, class)?;
    let peaks = builder.finish().filter(|k| k.frames > 0).ok_or_else(|| {
        item(
            "MEDIA_CORRUPT",
            "the audio track presents no sample".into(),
            json!({ "reason": "noSamples" }),
        )
    })?;
    test_built(&p.fingerprint, attempt);
    let out = dir.output().join(&p.key);
    fin_shm::create_output_dir(&out)
        .map_err(|e| inputs::relative_refusal(&dir, e))
        .map_err(Fail::Job)?;
    let bytes = peaks.to_bytes();
    write_file(&dir, &out.join(PEAKS_FILE), &bytes)?;
    let file = entry_file(&dir, &out, PEAKS_FILE)?;
    let input = wave_key_input(&p.fingerprint, class);
    let meta = EntryMeta {
        key: p.key.clone(),
        fingerprint: input.fingerprint,
        recipe: input.recipe,
        recipe_version: input.recipe_version,
        params: input.params,
        render_version: input.render_version,
        decoder_class: class,
        encoder: None,
        files: vec![file],
        media: None,
        tone_mapped: None,
        created_at: now_ms(),
    };
    let text =
        serde_json::to_vec_pretty(&meta).map_err(|e| CoreError::new("INTERNAL", e.to_string()))?;
    write_file(&dir, &out.join(META_FILE), &text)?;
    let result = WaveResult {
        key: p.key.clone(),
        levels: peaks.levels.len() as u32,
        frames: peaks.frames,
        bytes: bytes.len() as u64,
        from,
    };
    Ok((result, served))
}

/// `PLAN_MISMATCH {reason: cacheKey}` when `class` is not the decoder the host planned.
fn check_key(p: &WaveParams, class: VendorClass) -> Result<(), Fail> {
    if wave_key(&p.fingerprint, class).as_str() == p.key && class == p.decoder_class {
        return Ok(());
    }
    Err(item(
        "PLAN_MISMATCH",
        "the waveform's cache key does not match the decoder this worker opened".into(),
        json!({ "reason": "cacheKey", "expected": p.key, "decoderClass": class }),
    ))
}
