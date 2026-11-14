//! `media.thumbs` v1 (plan 6B-1 Task 15, Decision 17; sub-project 6 spec §3.2 item 6): PNG
//! thumbnails at chosen times, or a filmstrip, as one cache entry `out/<key>/`; cold, a batch
//! whose items are the thumbnails or the sprites.
//!
//! 1. **The source** ([`thumbs_source`](super::thumbs_source)): the `1080` proxy entry the
//!    manifest lists (verified as a look's, digests included), else the original through the
//!    manifest and 6A's no-follow opener (`ORIGINALS_MISSING {reason}`). A proxy that cannot be read is replaced by the original
//!    and the run says why (`proxyFailed`: the refusal's reason).
//! 2. **The key** (Decision 11) is recomputed from the original's decoder class (the decoder
//!    opened, or the one a proxy's `meta.json` records): another class than the host expected
//!    is `PLAN_MISMATCH {reason: cacheKey}`; a filmstrip's sprite count is checked against the
//!    source's duration (`reason: sprites`). Either fails every item before anything is written.
//! 3. **Each item** in order: `begin`, the frame (`frame_at` on the original's table), the GPU
//!    scale to the display size (tone-mapped like a proxy), stood upright, the PNG in
//!    `out/<key>/`, then `item` with a [`ThumbItem`]. A cancel is seen between items. Items a
//!    retry has no work for (`only`) keep the files an earlier attempt made.
//! 4. **The entry:** once every file is there (this run's or an earlier attempt's), `strip.json`
//!    (filmstrips) and `meta.json` are written; a run that leaves files to make writes neither.

use std::path::PathBuf;

use fin_core::CoreError;
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, StartJob};
use fin_media::cache::{EntryFile, EntryMeta, META_FILE};
use fin_media::jobs::{
    ProxyTier, STRIP_INDEX_FILE, STRIP_INDEX_VERSION, STRIP_SPRITE_SECONDS, STRIP_TILE_SECONDS,
    StripIndex, StripSprite, StripTile, ThumbItem, ThumbsMode, ThumbsParams, ThumbsRun,
    strip_sprites, strip_tiles, thumb_size, thumbs_key, tile_size, tile_time,
};
use fin_media::{Size, frame_at};
use fin_shm::JobDir;
use serde_json::{Value, json};

use super::pixels;
use super::proxy::{Fail, entry_file, item, manifest_asset, now_ms, open_asset, write_file};
use super::thumbs_source::{Origin, Source};
use super::{Warm, inputs, job_error, look, scale};

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: &mut Warm) -> Result<Value, JobError> {
    let p: ThumbsParams = serde_json::from_value(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("params: {e}")))?;
    p.check().map_err(job_error)?;
    let todo = p.todo();
    match make(&p, &todo, ctx, warm) {
        Ok(run) => serde_json::to_value(run)
            .map_err(|e| job_error(CoreError::new("INTERNAL", e.to_string()))),
        // The content is at fault: every item still to make fails with it.
        Err(Fail::Item(e)) => {
            for i in &todo {
                ctx.begin(i);
                ctx.item_failed(i, e.clone());
            }
            Ok(json!({ "made": 0, "source": "original", "complete": false }))
        }
        Err(Fail::Job(e)) => Err(e),
    }
}

/// The source of `p`: the `1080` proxy the manifest lists for the asset when it is readable,
/// else the original; and why a listed proxy could not be read.
fn open(
    p: &ThumbsParams,
    ctx: &JobCtx<'_>,
    dir: &JobDir,
    warm: &Warm,
) -> Result<(Source, Option<String>), Fail> {
    // The asset must still be this content, whichever file is read.
    let asset = manifest_asset(ctx, dir, &p.asset, &p.fingerprint)?;
    let mut proxy_failed = None;
    if let Some(px) = asset.proxy(ProxyTier::Preview) {
        match Source::proxy(px, &p.fingerprint, warm) {
            Ok(s) => return Ok((s, None)),
            Err(r) => proxy_failed = Some(r.reason().to_string()),
        }
    }
    let file = open_asset(ctx, dir, warm, &p.asset, &p.fingerprint)?;
    Ok((Source::original(file, warm)?, proxy_failed))
}

/// What this run needs to make one picture.
struct Maker<'a> {
    p: &'a ThumbsParams,
    src: Source,
    dir: &'a JobDir,
    out: PathBuf,
    /// `--replay` never replaces an output.
    replace: bool,
}

fn mismatch(reason: &str, details: Value) -> Fail {
    let mut d = json!({ "reason": reason });
    if let (Value::Object(m), Value::Object(extra)) = (&mut d, details) {
        m.extend(extra);
    }
    item(
        "PLAN_MISMATCH",
        format!("the request does not match the source this worker opened ({reason})"),
        d,
    )
}

fn make(
    p: &ThumbsParams,
    todo: &[String],
    ctx: &mut JobCtx<'_>,
    warm: &mut Warm,
) -> Result<ThumbsRun, Fail> {
    let dir = inputs::job_dir(ctx).map_err(Fail::Job)?;
    inputs::out_dir(ctx, &dir).map_err(Fail::Job)?;
    let (src, proxy_failed) = open(p, ctx, &dir, warm)?;
    test_crash(&p.fingerprint);
    let key = thumbs_key(&p.fingerprint, &p.mode, src.decoder_class);
    if key.as_str() != p.key {
        return Err(mismatch(
            "cacheKey",
            json!({ "expected": p.key, "decoderClass": src.decoder_class }),
        ));
    }
    if let ThumbsMode::Strip { sprites, .. } = p.mode {
        let want = strip_sprites(src.info.duration);
        if want != sprites {
            return Err(mismatch(
                "sprites",
                json!({ "expected": sprites, "found": want }),
            ));
        }
    }
    let out = dir.output().join(key.as_str());
    fin_shm::create_output_dir(&out)
        .map_err(|e| inputs::relative_refusal(&dir, e))
        .map_err(Fail::Job)?;
    let mut m = Maker {
        p,
        src,
        dir: &dir,
        out,
        replace: !warm.replay,
    };
    let origin = m.src.origin;
    let mut made = 0u32;
    for id in todo {
        if ctx.cancelled().is_some() {
            return Err(Fail::Job(JobError::cancelled()));
        }
        ctx.begin(id);
        let i: usize = id.parse().unwrap_or(usize::MAX);
        match m.item(i, warm) {
            Ok(v) => {
                ctx.item(
                    id,
                    Some(
                        serde_json::to_value(&v)
                            .map_err(|e| CoreError::new("INTERNAL", e.to_string()))?,
                    ),
                );
                made += 1;
                test_crash_after(&p.fingerprint, p.only.is_none(), made);
            }
            Err(Fail::Item(e)) => {
                ctx.item_failed(id, e);
                return Ok(run_of(made, origin, proxy_failed, false));
            }
            Err(f) => return Err(f),
        }
    }
    let complete = m.finish(warm)?;
    Ok(run_of(made, origin, proxy_failed, complete))
}

fn run_of(made: u32, origin: Origin, proxy_failed: Option<String>, complete: bool) -> ThumbsRun {
    ThumbsRun {
        made,
        source: origin.name().into(),
        proxy_failed,
        complete,
    }
}

impl Maker<'_> {
    /// The upright size of a thumbnail or tile.
    fn shown(&self) -> Size {
        let d = self.src.video.display;
        match &self.p.mode {
            ThumbsMode::Thumbs { max_side, .. } => thumb_size(d, *max_side),
            ThumbsMode::Strip { tile_height, .. } => tile_size(d, *tile_height),
        }
    }

    /// The picture of original frame `index` at `shown`, upright (RGBA).
    fn picture(
        &mut self,
        index: usize,
        shown: Size,
        warm: &mut Warm,
    ) -> Result<Vec<u8>, CoreError> {
        let rotation = self.src.video.rotation;
        // The scale pass works in the coded orientation; rotating after it keeps the bytes of
        // the pass a function of the coded picture alone.
        let target = if rotation.swaps() {
            Size::new(shown.height, shown.width)
        } else {
            shown
        };
        let f = self.src.reader.frame(index)?;
        let colour = scale::colour_of(f, &self.src.ingest);
        if let Some(t) = scale::tone_mapped(&colour, &self.src.ingest) {
            self.src.tone_mapped = Some(t);
        }
        let g = look::gpu(warm).map_err(|e| CoreError {
            code: e.code,
            message: e.message,
            details: e.details,
            fix: None,
        })?;
        let rgba = g
            .compositor
            .scale_rgba8(&g.gpu, &scale::source_of(f, &self.src.ingest), target);
        let rgba = match rgba {
            Ok(r) => r,
            Err(e) => {
                // A device that failed is not kept for the next job.
                if e.code.starts_with("GPU_") {
                    warm.gpu = None;
                }
                return Err(e);
            }
        };
        Ok(pixels::rotate(&rgba, target, rotation))
    }

    fn write(&self, file: &str, size: Size, rgba: &[u8]) -> Result<(), Fail> {
        let name = format!("{}/{file}", self.p.key);
        inputs::write_png(self.dir, &name, size.width, size.height, rgba, self.replace)
            .map_err(Fail::Job)
    }

    fn item(&mut self, i: usize, warm: &mut Warm) -> Result<ThumbItem, Fail> {
        let shown = self.shown();
        let file = self.p.mode.file_name(i);
        match self.p.mode.clone() {
            ThumbsMode::Thumbs { times, .. } => {
                let at = times[i];
                let index = frame_at(&self.src.table, at).index;
                let rgba = self.picture(index, shown, warm)?;
                self.write(&file, shown, &rgba)?;
                Ok(ThumbItem {
                    file,
                    width: shown.width,
                    height: shown.height,
                    at: Some(at),
                    frame: Some(index as u64),
                    tiles: None,
                })
            }
            ThumbsMode::Strip { .. } => {
                let tiles = self.sprite_tiles(i);
                let width = shown.width * tiles.len() as u32;
                let mut sprite = vec![0u8; width as usize * shown.height as usize * 4];
                // Neighbouring tiles of a slow source show one frame: made once.
                let mut last: Option<(usize, Vec<u8>)> = None;
                for (n, (_, frame)) in tiles.iter().enumerate() {
                    let rgba = match &last {
                        Some((f, px)) if f == frame => px.clone(),
                        _ => self.picture(*frame, shown, warm)?,
                    };
                    pixels::paste(&mut sprite, width, n as u32 * shown.width, &rgba, shown);
                    last = Some((*frame, rgba));
                }
                let size = Size::new(width, shown.height);
                self.write(&file, size, &sprite)?;
                Ok(ThumbItem {
                    file,
                    width,
                    height: shown.height,
                    at: None,
                    frame: None,
                    tiles: Some(tiles.len() as u32),
                })
            }
        }
    }

    /// The tiles of sprite `k`: each one's source time (as the tile number) and original frame.
    fn sprite_tiles(&self, k: usize) -> Vec<(u32, usize)> {
        let per = (STRIP_SPRITE_SECONDS / STRIP_TILE_SECONDS) as usize;
        let total = strip_tiles(self.src.info.duration) as usize;
        (k * per..((k + 1) * per).min(total))
            .map(|j| {
                (
                    j as u32,
                    frame_at(&self.src.table, tile_time(j as u32)).index,
                )
            })
            .collect()
    }

    /// Writes `strip.json` and `meta.json` when every file of the entry exists.
    fn finish(&mut self, _warm: &Warm) -> Result<bool, Fail> {
        let items = self.p.mode.items().len();
        let mut names: Vec<String> = (0..items).map(|i| self.p.mode.file_name(i)).collect();
        for name in &names {
            if !self.exists(name)? {
                return Ok(false);
            }
        }
        if let ThumbsMode::Strip { tile_height, .. } = self.p.mode {
            let shown = tile_size(self.src.video.display, tile_height);
            self.write_index(shown, items)?;
            names.push(STRIP_INDEX_FILE.into());
        }
        let files = names
            .iter()
            .map(|n| entry_file(self.dir, &self.out, n))
            .collect::<Result<Vec<EntryFile>, _>>()?;
        let input = self.p.key_input(self.src.decoder_class);
        let meta = EntryMeta {
            key: self.p.key.clone(),
            fingerprint: input.fingerprint,
            recipe: input.recipe,
            recipe_version: input.recipe_version,
            params: input.params,
            render_version: input.render_version,
            decoder_class: self.src.decoder_class,
            encoder: None,
            files,
            media: None,
            tone_mapped: self.src.tone_mapped,
            created_at: now_ms(),
        };
        let text = serde_json::to_vec_pretty(&meta)
            .map_err(|e| CoreError::new("INTERNAL", e.to_string()))?;
        write_file(self.dir, &self.out.join(META_FILE), &text)?;
        Ok(true)
    }

    /// Whether `out/<key>/<name>` was made (by this run or an earlier attempt).
    fn exists(&self, name: &str) -> Result<bool, Fail> {
        match fin_shm::checked_open(&self.out.join(name), u64::MAX) {
            Ok(_) => Ok(true),
            Err(e) if e.details.get("reason").and_then(Value::as_str) == Some("missing") => {
                Ok(false)
            }
            Err(e) => Err(Fail::Job(inputs::relative_refusal(self.dir, e))),
        }
    }

    fn write_index(&self, tile: Size, sprites: usize) -> Result<(), Fail> {
        let sprites: Vec<StripSprite> = (0..sprites)
            .map(|k| StripSprite {
                file: self.p.mode.file_name(k),
                tiles: self
                    .sprite_tiles(k)
                    .into_iter()
                    .map(|(j, frame)| StripTile {
                        at: tile_time(j),
                        frame: frame as u64,
                    })
                    .collect(),
            })
            .collect();
        let index = StripIndex {
            version: STRIP_INDEX_VERSION,
            tile_width: tile.width,
            tile_height: tile.height,
            tile_seconds: STRIP_TILE_SECONDS as u32,
            sprite_seconds: STRIP_SPRITE_SECONDS as u32,
            sprites,
        };
        let text =
            serde_json::to_vec(&index).map_err(|e| CoreError::new("INTERNAL", e.to_string()))?;
        write_file(self.dir, &self.out.join(STRIP_INDEX_FILE), &text)?;
        Ok(())
    }
}

/// Test builds only: a fingerprint starting `test-crash-` aborts the worker when it opens the
/// source (the engine's isolation then poisons the item).
#[cfg(feature = "test-hooks")]
fn test_crash(fp: &str) {
    if fp.starts_with("test-crash-") && !fp.starts_with("test-crash-after-") {
        std::process::abort();
    }
}

#[cfg(not(feature = "test-hooks"))]
fn test_crash(_: &str) {}

/// Test builds only: `test-crash-after-<n>-…` aborts the worker once `n` items of a run that
/// makes every item are acknowledged (what a retry must not repeat).
#[cfg(feature = "test-hooks")]
fn test_crash_after(fp: &str, first_run: bool, made: u32) {
    let n = fp
        .strip_prefix("test-crash-after-")
        .and_then(|r| r.split('-').next())
        .and_then(|n| n.parse::<u32>().ok());
    if first_run && n == Some(made) {
        std::process::abort();
    }
}

#[cfg(not(feature = "test-hooks"))]
fn test_crash_after(_: &str, _: bool, _: u32) {}
