//! `media.pcm` v1 (plan 6C Task 18, Decisions 7, 8, 20 and 23; sub-project 6 spec §7.2): a
//! source's audio as a `pcm48` or `pcm16m` cache entry, cold, a batch of one item (the
//! fingerprint), so the `pcm16m` library stage and a project's request share the kind.
//!
//! 1. **The source:** a project asset through `in/manifest.json` (its fingerprint must be the one
//!    asked for: `ASSET_CHANGED`) and 6A's no-follow opener (`ORIGINALS_MISSING {reason}`), or a
//!    library file through `fin-library`'s opener with the catalogue's raw chain and `Expect`
//!    (`FILE_CHANGED`, `FILE_NOT_FOUND`, `FILE_CLOUD_ONLY`), as `media.proxy` opens them.
//! 2. **The key** is recomputed from the decoder `AudioReader` opened and the source's channel
//!    count: another class or layout than the host planned is `PLAN_MISMATCH {reason: cacheKey}`,
//!    before anything is written.
//! 3. **The feed** (`audio_src::DecodedSource`): decoded, canonical, resampled, written as
//!    `out/<key>/chunk-NNNNNN.f32` (8 MiB each, the last short), each chunk complete and synced
//!    before the next is started, then `out/<key>/meta.json` (the compact `EntryMeta` the host's
//!    commit checks: every listed size and BLAKE3). Progress after each chunk, in output frames of the
//!    container's stated length when it states one; cancel between chunks.
//!
//! **Retries restart from sample 0** (Decision 23): nothing is checkpointed, and the item is
//! answered only once the entry is complete, so an attempt that dies leaves at most a partial
//! last chunk in `out/`, which 5C empties before the next attempt. The decode is deterministic
//! for a given decoder (Decision 8), so a retried entry is byte-identical to an uninterrupted one
//! but for `meta.json`'s `createdAt`.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use fin_core::CoreError;
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, ProgressUnit, StartJob};
use fin_library::{Expect, OpenError, RawRel, open_for_read};
use fin_media::cache::{EntryFile, EntryMeta, META_FILE};
use fin_media::jobs::{
    PcmDecoder, PcmLayout, PcmParams, PcmResult, ProxyLibraryFile, chunk_name, pcm_key,
};
use fin_shm::JobDir;
use serde_json::{Value, json};

use super::audio_src::DecodedSource;
use super::proxy::{Fail, io_error, item, now_ms, open_asset};
use super::{Warm, inputs, job_error};

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: &mut Warm) -> Result<Value, JobError> {
    let p: PcmParams = serde_json::from_value(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("params: {e}")))?;
    p.check().map_err(job_error)?;
    let fp = p.fingerprint.clone();
    ctx.begin(&fp);
    match make(&p, ctx, warm, job.attempt) {
        Ok(result) => {
            let value = serde_json::to_value(&result)
                .map_err(|e| job_error(CoreError::new("INTERNAL", e.to_string())))?;
            ctx.item(&fp, Some(value));
            test_item_answered(&fp, job.attempt);
            Ok(json!({ "made": 1 }))
        }
        Err(Fail::Item(e)) => {
            ctx.item_failed(&fp, e);
            Ok(json!({ "made": 0 }))
        }
        Err(Fail::Job(e)) => Err(e),
    }
}

/// A library file through `fin-library`'s opener (the `pcm16m` stage), as `media.proxy`.
fn open_library(f: &ProxyLibraryFile) -> Result<File, Fail> {
    let invalid = |what: &str| {
        item(
            "JOB_PARAMS_INVALID",
            format!("params.file.{what}: not fin-library's wire form"),
            json!({ "field": format!("file.{what}") }),
        )
    };
    let rel: RawRel = serde_json::from_value(f.rel.clone()).map_err(|_| invalid("rel"))?;
    let expect: Expect = serde_json::from_value(f.expect.clone()).map_err(|_| invalid("expect"))?;
    let root = Path::new(&f.root);
    if !root.is_absolute() {
        return Err(invalid("root"));
    }
    open_for_read(root, &rel, &expect, f.hydrate).map_err(|e| {
        let code = match &e {
            OpenError::CloudOnly => "FILE_CLOUD_ONLY",
            OpenError::Changed => "FILE_CHANGED",
            OpenError::NotFound => "FILE_NOT_FOUND",
            OpenError::Escapes | OpenError::NameSurrogate => "INVALID_PATH",
            OpenError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => "FILE_NOT_FOUND",
            OpenError::Io(_) => "ASSET_UNREADABLE",
        };
        item(code, e.to_string(), json!({ "library": f.library }))
    })
}

/// Test builds only, after each chunk: `FIN_TEST_PCM_ABORT_AFTER=<n>` aborts the worker once `n`
/// chunks are written (a worker death mid-entry, Decision 23's test). Hosts clear a worker's
/// environment, so their tests name the fault in the fingerprint: `test-slow-` waits 300 ms (a
/// host that dies mid-job), `test-crashonce-` aborts after chunk 2 of the first attempt,
/// `test-crashalways-` after chunk 1 of every attempt.
#[cfg(feature = "test-hooks")]
fn test_chunk_written(written: usize, fingerprint: &str, attempt: u32) {
    if fingerprint.starts_with("test-slow-") {
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    if (fingerprint.starts_with("test-crashonce-") && attempt == 1 && written == 2)
        || (fingerprint.starts_with("test-crashalways-") && written == 1)
    {
        std::process::abort();
    }
    if std::env::var("FIN_TEST_PCM_ABORT_AFTER")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        == Some(written)
    {
        std::process::abort();
    }
}

#[cfg(not(feature = "test-hooks"))]
fn test_chunk_written(_: usize, _: &str, _: u32) {}

/// Test builds only: `test-crashack-` aborts right after the first attempt answered its item.
#[cfg(feature = "test-hooks")]
fn test_item_answered(fingerprint: &str, attempt: u32) {
    if fingerprint.starts_with("test-crashack-") && attempt == 1 {
        // Let the item's line reach the host first.
        std::thread::sleep(std::time::Duration::from_millis(200));
        std::process::abort();
    }
}

#[cfg(not(feature = "test-hooks"))]
fn test_item_answered(_: &str, _: u32) {}

/// Writes the chunks of the feed into `out`, each complete and synced before the next.
struct Chunks<'a> {
    dir: &'a JobDir,
    out: &'a Path,
    files: Vec<EntryFile>,
    buf: Vec<u8>,
    cap: usize,
    fingerprint: &'a str,
    attempt: u32,
}

impl Chunks<'_> {
    fn flush(&mut self) -> Result<(), CoreError> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let name = chunk_name(self.files.len());
        let path = self.out.join(&name);
        let mut f = fin_shm::create_output(&path).map_err(|e| inputs::relative(self.dir, e))?;
        f.write_all(&self.buf).map_err(|e| io_error(&path, e))?;
        f.sync_all().map_err(|e| io_error(&path, e))?;
        self.files.push(EntryFile {
            name,
            bytes: self.buf.len() as u64,
            blake3: blake3::hash(&self.buf).to_hex().to_string(),
        });
        self.buf.clear();
        test_chunk_written(self.files.len(), self.fingerprint, self.attempt);
        Ok(())
    }

    /// Appends samples; `true` when a chunk was written.
    fn push(&mut self, samples: &[f32]) -> Result<bool, CoreError> {
        let mut wrote = false;
        for s in samples {
            self.buf.extend_from_slice(&s.to_le_bytes());
            if self.buf.len() == self.cap {
                self.flush()?;
                wrote = true;
            }
        }
        Ok(wrote)
    }
}

fn make(
    p: &PcmParams,
    ctx: &mut JobCtx<'_>,
    warm: &mut Warm,
    attempt: u32,
) -> Result<PcmResult, Fail> {
    let dir = inputs::job_dir(ctx).map_err(Fail::Job)?;
    inputs::out_dir(ctx, &dir).map_err(Fail::Job)?;
    let file = match &p.file {
        Some(f) => open_library(f)?,
        None => open_asset(
            ctx,
            &dir,
            warm,
            p.asset.as_deref().unwrap_or_default(),
            &p.fingerprint,
        )?,
    };
    let mut src = DecodedSource::open(file, p.recipe)?;
    let layout: PcmLayout = src.feed_layout();
    let class = src.decoder_class();
    let key = pcm_key(&p.fingerprint, p.recipe, layout, class);
    if key.as_str() != p.key {
        return Err(item(
            "PLAN_MISMATCH",
            "the PCM feed's cache key does not match the decoder or layout this worker opened"
                .into(),
            json!({ "reason": "cacheKey", "expected": p.key, "decoderClass": class, "layout": layout }),
        ));
    }
    let (source_rate, source_channels) = (src.info().sample_rate, src.info().channels);
    let total = src.expected_frames();
    let out = dir.output().join(key.as_str());
    fin_shm::create_output_dir(&out)
        .map_err(|e| inputs::relative_refusal(&dir, e))
        .map_err(Fail::Job)?;
    let mut chunks = Chunks {
        dir: &dir,
        out: &out,
        files: Vec::new(),
        buf: Vec::with_capacity(fin_media::jobs::CHUNK_BYTES as usize),
        cap: fin_media::jobs::CHUNK_BYTES as usize,
        fingerprint: &p.fingerprint,
        attempt,
    };
    let channels = layout.channels() as u64;
    let mut samples = 0u64;
    while let Some(block) = src.next_interleaved()? {
        samples += block.len() as u64;
        if chunks.push(&block)? {
            if ctx.cancelled().is_some() {
                return Err(Fail::Job(JobError::cancelled()));
            }
            let mut pr = fin_jobs::Progress::new((samples / channels) as f64, ProgressUnit::Frames);
            pr.total = total.map(|t| t as f64);
            pr.phase = Some("decodeAudio".into());
            ctx.progress(pr);
        }
    }
    chunks.flush()?;
    let frames = samples / channels;
    if frames == 0 {
        return Err(item(
            "MEDIA_CORRUPT",
            "the audio track presents no sample".into(),
            json!({ "reason": "noSamples" }),
        ));
    }
    let input = p.key_input(layout, class);
    let meta = EntryMeta {
        key: key.to_string(),
        fingerprint: input.fingerprint,
        recipe: input.recipe,
        recipe_version: input.recipe_version,
        params: input.params,
        render_version: input.render_version,
        decoder_class: class,
        encoder: None,
        files: chunks.files,
        media: None,
        tone_mapped: None,
        created_at: now_ms(),
    };
    let bytes = meta.bytes()?;
    let text = serde_json::to_vec(&meta).map_err(|e| CoreError::new("INTERNAL", e.to_string()))?;
    super::proxy::write_file(&dir, &out.join(META_FILE), &text)?;
    Ok(PcmResult {
        key: key.to_string(),
        recipe: p.recipe,
        layout,
        frames,
        chunks: meta.files.len() as u32,
        decoder: PcmDecoder {
            backend: src.backend().to_string(),
            class,
        },
        source_rate,
        source_channels,
        bytes,
    })
}
