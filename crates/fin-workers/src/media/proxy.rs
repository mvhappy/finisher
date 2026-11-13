//! `media.proxy` v1 (plan 6B-1 Task 14; sub-project 6 spec §7.1): a proxy of one source, cold,
//! a batch of one item (the fingerprint), so the `proxy540` library stage and a project's
//! request share the kind.
//!
//! 1. **The source:** a project asset through `in/manifest.json` (its fingerprint must be the one
//!    asked for: `ASSET_CHANGED`) and 6A's no-follow opener (`ORIGINALS_MISSING {reason}`), or a
//!    library file through `fin-library`'s opener with the catalogue's raw chain and `Expect`
//!    (`FILE_CHANGED`, `FILE_NOT_FOUND`, `FILE_CLOUD_ONLY`: the stage rescans it).
//! 2. **The key** (Decision 11) is recomputed from the decoder `open_decoder` picked: another
//!    class than the host expected is `PLAN_MISMATCH {reason: cacheKey}`, before anything is
//!    written.
//! 3. **Every presented frame** in presentation order (`FrameReader`, so edit lists and VFR are
//!    the original's table), scaled and tone-mapped on the GPU into NV12 (`scale.rs`), encoded
//!    under `EncodePurpose::Proxy` with Decision 8's GOP (`encode_pipeline.rs`), muxed CFR at
//!    the original's nominal rate (Decision 7) into `out/<key>/proxy.mp4`; then
//!    `out/<key>/table.bin` (the original's sample table) and `out/<key>/meta.json` (the
//!    `EntryMeta` the host's cache commit checks). Progress every 1 % of frames; cancel
//!    between frames.
//!
//! The item fails with the source's or the codec's own error (`CODEC_UNAVAILABLE` for a codec
//! nothing here encodes, with `open_encoder`'s guidance); GPU loss and I/O fail the job, so the
//! engine retries it.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fin_codecs::{DecodeThreads, EncoderConfig, FrameReader, Gop, RateControl, RawFrame};
use fin_container::Limits;
use fin_core::CoreError;
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, ProgressUnit, StartJob};
use fin_library::{Expect, OpenError, RawRel, open_for_read};
use fin_media::backend::{EncodePurpose, backend};
use fin_media::cache::{EntryEncoder, EntryFile, EntryMeta, META_FILE};
use fin_media::jobs::{
    MANIFEST_VERSION, ProxyCodec, ProxyParams, ProxyResult, RenderManifest, gop, proxy_key,
};
use fin_media::table_bin::{self, TABLE_FILE};
use fin_media::{ColourTags, MediaInfo, VideoTrackInfo};
use fin_shm::JobDir;
use serde_json::{Value, json};

use super::encode_pipeline::{self, Frames, Track};
use super::inputs::{self, MANIFEST_CAP};
use super::open_cache::{WORKER_MAX_PIXELS, open_original_in};
use super::{Warm, job_error, look, scale};

/// The proxy's video file in its entry.
pub const PROXY_FILE: &str = "proxy.mp4";
/// SVT-AV1's quality for each tier (spec §7.1: CRF); hardware encoders map it to their scale.
const CRF_540: u8 = 38;
const CRF_1080: u8 = 32;

/// A failure of the item (`Item`) or of the job (`Job`: retried by the engine's policy).
enum Fail {
    Item(JobError),
    Job(JobError),
}

impl From<CoreError> for Fail {
    /// GPU loss and memory, I/O and refused outputs fail the job (retryable or not the
    /// item's fault); everything else is the item's.
    fn from(e: CoreError) -> Fail {
        match e.code.as_str() {
            "GPU_DEVICE_LOST" | "GPU_OUT_OF_MEMORY" | "GPU_UNAVAILABLE" | "IO_ERROR"
            | "SHM_REFUSED" | "INTERNAL" => Fail::Job(job_error(e)),
            "JOB_CANCELLED" => Fail::Job(JobError::cancelled()),
            _ => Fail::Item(job_error(e)),
        }
    }
}

fn item(code: &str, message: String, details: Value) -> Fail {
    Fail::Item(JobError::new(code, message).with_details(details))
}

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: &mut Warm) -> Result<Value, JobError> {
    let p: ProxyParams = serde_json::from_value(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("params: {e}")))?;
    p.check().map_err(job_error)?;
    let fp = p.fingerprint.clone();
    ctx.begin(&fp);
    match make(&p, ctx, warm) {
        Ok(result) => {
            let value = serde_json::to_value(&result)
                .map_err(|e| job_error(CoreError::new("INTERNAL", e.to_string())))?;
            ctx.item(&fp, Some(value));
            Ok(json!({ "made": 1 }))
        }
        Err(Fail::Item(e)) => {
            ctx.item_failed(&fp, e);
            Ok(json!({ "made": 0 }))
        }
        Err(Fail::Job(e)) => Err(e),
    }
}

/// The original, opened: the file and how to name it in errors.
fn open_source(p: &ProxyParams, ctx: &JobCtx<'_>, dir: &JobDir, warm: &Warm) -> Result<File, Fail> {
    if let Some(f) = &p.file {
        let invalid = |what: &str| {
            item(
                "JOB_PARAMS_INVALID",
                format!("params.file.{what}: not fin-library's wire form"),
                json!({ "field": format!("file.{what}") }),
            )
        };
        let rel: RawRel = serde_json::from_value(f.rel.clone()).map_err(|_| invalid("rel"))?;
        let expect: Expect =
            serde_json::from_value(f.expect.clone()).map_err(|_| invalid("expect"))?;
        let root = Path::new(&f.root);
        if !root.is_absolute() {
            return Err(item(
                "JOB_PARAMS_INVALID",
                "params.file.root: not absolute".into(),
                json!({ "field": "file.root" }),
            ));
        }
        return open_for_read(root, &rel, &expect, f.hydrate).map_err(|e| {
            let code = match &e {
                OpenError::CloudOnly => "FILE_CLOUD_ONLY",
                OpenError::Changed => "FILE_CHANGED",
                OpenError::NotFound => "FILE_NOT_FOUND",
                OpenError::Escapes | OpenError::NameSurrogate => "INVALID_PATH",
                OpenError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => "FILE_NOT_FOUND",
                OpenError::Io(_) => "ASSET_UNREADABLE",
            };
            item(code, e.to_string(), json!({ "library": f.library }))
        });
    }
    let asset = p.asset.as_deref().unwrap_or_default();
    let manifest: RenderManifest =
        inputs::read_json(ctx, dir, "in/manifest.json", MANIFEST_CAP).map_err(Fail::Job)?;
    if manifest.version != MANIFEST_VERSION {
        return Err(Fail::Job(JobError::new(
            "JOB_INPUT_INVALID",
            format!(
                "manifest version {} (this worker reads {MANIFEST_VERSION})",
                manifest.version
            ),
        )));
    }
    let a = manifest.asset(asset).ok_or_else(|| {
        Fail::Job(
            JobError::new(
                "JOB_INPUT_INVALID",
                format!("the manifest does not name asset {asset}"),
            )
            .with_details(json!({ "file": "in/manifest.json", "asset": asset })),
        )
    })?;
    if a.fingerprint != p.fingerprint {
        return Err(item(
            "ASSET_CHANGED",
            format!("asset {asset} is not the content the proxy was asked for"),
            json!({ "asset": asset, "reason": "fingerprint" }),
        ));
    }
    let missing = |reason: &str, message: String| {
        item(
            "ORIGINALS_MISSING",
            message,
            json!({ "asset": asset, "reason": reason }),
        )
    };
    let original = a.original.as_ref().ok_or_else(|| {
        missing(
            "notFound",
            format!("asset {asset}'s file is not on this machine"),
        )
    })?;
    open_original_in(original, warm.sources.confined()).map_err(|r| missing(r.reason, r.message))
}

/// Test builds only: a fingerprint starting `test-crash-` aborts the worker (the engine's
/// isolation then poisons the item: `rememberPoison`).
#[cfg(feature = "test-hooks")]
fn test_crash(fp: &str) {
    if fp.starts_with("test-crash-") {
        std::process::abort();
    }
}

#[cfg(not(feature = "test-hooks"))]
fn test_crash(_: &str) {}

fn make(p: &ProxyParams, ctx: &mut JobCtx<'_>, warm: &mut Warm) -> Result<ProxyResult, Fail> {
    let dir = inputs::job_dir(ctx).map_err(Fail::Job)?;
    inputs::out_dir(ctx, &dir).map_err(Fail::Job)?;
    let file = open_source(p, ctx, &dir, warm)?;
    test_crash(&p.fingerprint);
    let mut reader = FrameReader::open_with_limit(
        file,
        &Limits::default(),
        &warm.probe,
        DecodeThreads::default(),
        WORKER_MAX_PIXELS,
    )?;
    let decoder = reader.backend().0;
    let decoder_class = backend(decoder)
        .map(|b| b.class)
        .ok_or_else(|| CoreError::new("INTERNAL", format!("{decoder} is not a known backend")))?;
    let key = proxy_key(&p.fingerprint, p.tier, p.codec, decoder_class);
    if key.as_str() != p.key {
        return Err(item(
            "PLAN_MISMATCH",
            "the proxy's cache key does not match the decoder this worker opened".into(),
            json!({ "reason": "cacheKey", "expected": p.key, "decoderClass": decoder_class }),
        ));
    }
    let info: MediaInfo = reader.info().clone();
    let video: VideoTrackInfo = info.video.clone().ok_or_else(|| {
        item(
            "NOT_FOUND",
            "the file has no video track".into(),
            json!({ "kind": "videoTrack" }),
        )
    })?;
    let table = reader.table().clone();
    let frames = table.len() as u64;
    let size = p.tier.fit(video.coded);
    let (timescale, tick) = encode_pipeline::timing(video.nominal_rate)?;
    let (every, intra_only) = gop(
        p.tier,
        video.nominal_rate,
        p.intra_ok,
        p.codec == ProxyCodec::Av1,
    );
    let cfg = EncoderConfig {
        codec: p.codec.video(),
        profile: None,
        size,
        rate: video.nominal_rate,
        gop: Gop {
            key_every_frames: every,
            closed: true,
            intra_only,
        },
        rc: RateControl::Crf(match p.tier {
            fin_media::jobs::ProxyTier::Scrub => CRF_540,
            fin_media::jobs::ProxyTier::Preview => CRF_1080,
        }),
        purpose: EncodePurpose::Proxy,
        adapter: None,
        preset: None,
        // Proxies need not be bit-reproducible: SVT-AV1 may use several threads.
        threads: std::thread::available_parallelism().map_or(1, |n| n.get().min(8)) as u32,
        colour: ColourTags::BT709,
    };
    let out = dir.output().join(key.as_str());
    fin_shm::create_output_dir(&out)
        .map_err(|e| inputs::relative_refusal(&dir, e))
        .map_err(Fail::Job)?;
    let caps = warm.probe.clone();
    let mut tone_mapped = None;
    let mut src = Source {
        reader: &mut reader,
        video: &video,
        warm,
        size,
        ctx,
        frames,
        step: (frames / 100).max(1),
        path: out.join(PROXY_FILE),
        tone_mapped: &mut tone_mapped,
    };
    let encoded = encode_pipeline::encode(
        &caps,
        &cfg,
        &Track {
            timescale,
            tick,
            colour: ColourTags::BT709,
        },
        frames,
        &mut src,
    )?;
    write_file(&out.join(TABLE_FILE), &table_bin::encode(&table))?;
    let files = vec![entry_file(&out, PROXY_FILE)?, entry_file(&out, TABLE_FILE)?];
    let input = p.key_input(decoder_class);
    let meta = EntryMeta {
        key: key.to_string(),
        fingerprint: input.fingerprint,
        recipe: input.recipe,
        recipe_version: input.recipe_version,
        params: input.params,
        render_version: input.render_version,
        decoder_class,
        encoder: Some(EntryEncoder {
            backend: encoded.entry.name.to_string(),
            class: encoded.entry.class,
        }),
        files,
        media: Some(info),
        tone_mapped,
        created_at: now_ms(),
    };
    let bytes = meta.bytes()?;
    let text =
        serde_json::to_vec_pretty(&meta).map_err(|e| CoreError::new("INTERNAL", e.to_string()))?;
    write_file(&out.join(META_FILE), &text)?;
    Ok(ProxyResult {
        key: key.to_string(),
        tier: p.tier,
        codec: p.codec,
        backend: encoded.entry.name.to_string(),
        class: encoded.entry.class,
        decoder: decoder.to_string(),
        frames: encoded.frames,
        bytes,
        size,
        tone_mapped,
        fell_back: encoded.fell_back,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn io_error(path: &Path, e: impl std::fmt::Display) -> CoreError {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    CoreError::new("IO_ERROR", format!("{name}: {e}")).with_details(json!({ "file": name }))
}

/// Writes `bytes` as a new output file (no link at any segment).
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let mut f = fin_shm::create_output(path)?;
    f.write_all(bytes).map_err(|e| io_error(path, e))?;
    f.sync_all().map_err(|e| io_error(path, e))
}

/// The size and BLAKE3 of `out/<key>/<name>`, read back through the checked open.
fn entry_file(dir: &Path, name: &str) -> Result<EntryFile, CoreError> {
    let path = dir.join(name);
    let mut f = fin_shm::checked_open(&path, u64::MAX)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut bytes = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| io_error(&path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes += n as u64;
    }
    Ok(EntryFile {
        name: name.to_string(),
        bytes,
        blake3: hasher.finalize().to_hex().to_string(),
    })
}

/// The frames of the original, decoded, scaled and converted, for the encode pipeline.
struct Source<'a, 'c> {
    reader: &'a mut FrameReader<File>,
    video: &'a VideoTrackInfo,
    /// The GPU is opened on the first frame, after the encoder: a codec nothing here encodes
    /// is refused without one.
    warm: &'a mut Warm,
    size: fin_media::Size,
    ctx: &'a mut JobCtx<'c>,
    frames: u64,
    /// Progress is sent every `step` frames (1 %).
    step: u64,
    path: PathBuf,
    tone_mapped: &'a mut Option<fin_media::jobs::ToneMapped>,
}

impl Frames for Source<'_, '_> {
    fn frame(&mut self, i: u64) -> Result<RawFrame, CoreError> {
        let f = self.reader.frame(i as usize)?;
        let colour = scale::colour_of(f, self.video);
        if let Some(t) = scale::tone_mapped(&colour, self.video) {
            *self.tone_mapped = Some(t);
        }
        let g = look::gpu(self.warm).map_err(|e| CoreError {
            code: e.code,
            message: e.message,
            details: e.details,
            fix: None,
        })?;
        scale::scale(&mut g.compositor, &g.gpu, f, self.video, self.size)
    }

    fn between(&mut self, done: u64) -> Result<(), CoreError> {
        if self.ctx.cancelled().is_some() {
            return Err(CoreError::new("JOB_CANCELLED", "the proxy was cancelled"));
        }
        if done.is_multiple_of(self.step) || done == self.frames {
            let mut pr = fin_jobs::Progress::new(done as f64, ProgressUnit::Frames);
            pr.total = Some(self.frames as f64);
            pr.phase = Some("encode".into());
            self.ctx.progress(pr);
        }
        Ok(())
    }

    fn output(&mut self) -> Result<File, CoreError> {
        fin_shm::create_output(&self.path)
    }
}
