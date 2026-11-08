//! The job's own inputs (`in/snapshot.json`, `in/manifest.json`) and outputs (`out/*.png`).
//! Every path is built from the [`JobDir`] `fin-shm` resolves (only the host's `jobs/` parent
//! is canonicalised; a link at the job directory or below it is refused), inputs are opened
//! with [`fin_shm::checked_open`] (no-follow, a regular file, one link, under a cap) and only
//! the grants `start.files` lists are read.

use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};

use fin_core::CoreError;
use fin_jobs::JobError;
use fin_jobs::worker::JobCtx;
use fin_shm::JobDir;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::job_error;

/// The largest `in/snapshot.json` read.
pub const SNAPSHOT_CAP: u64 = 256 << 20;
/// The largest `in/manifest.json` read.
pub const MANIFEST_CAP: u64 = 64 << 20;

fn input_invalid(name: &str, e: impl std::fmt::Display) -> JobError {
    JobError::new("JOB_INPUT_INVALID", format!("{name}: {e}")).with_details(json!({ "file": name }))
}

/// `SHM_REFUSED` with `details.file` made relative to the job directory (no absolute path of
/// the host reaches a client).
pub fn relative_refusal(dir: &JobDir, mut e: CoreError) -> JobError {
    if let Some(file) = e.details.get("file").and_then(|f| f.as_str()) {
        let rel = Path::new(file)
            .strip_prefix(&dir.root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| "<outside the job directory>".to_owned());
        e.details["file"] = json!(rel);
    }
    job_error(e)
}

/// The job directory of `ctx`, resolved.
pub fn job_dir(ctx: &JobCtx<'_>) -> Result<JobDir, JobError> {
    JobDir::resolve(&ctx.dirs().job).map_err(job_error)
}

/// Where grant `name` is, built from `dir` (the grant's own path must name the same file).
fn granted(ctx: &JobCtx<'_>, dir: &JobDir, name: &str) -> Result<PathBuf, JobError> {
    let grant = ctx.file(name).ok_or_else(|| {
        JobError::new("JOB_INPUT_INVALID", format!("the job has no {name} grant"))
            .with_details(json!({ "file": name }))
    })?;
    let path = dir.root.join(name.trim_end_matches('/'));
    if grant.path.file_name() != path.file_name() {
        return Err(input_invalid(name, "the grant names another file"));
    }
    Ok(path)
}

/// Grant `name` (a JSON file under `in/`) decoded as `T`, at most `cap` bytes.
pub fn read_json<T: DeserializeOwned>(
    ctx: &JobCtx<'_>,
    dir: &JobDir,
    name: &str,
    cap: u64,
) -> Result<T, JobError> {
    let path = granted(ctx, dir, name)?;
    let file = fin_shm::checked_open(&path, cap).map_err(|e| relative_refusal(dir, e))?;
    let mut text = Vec::new();
    file.take(cap)
        .read_to_end(&mut text)
        .map_err(|e| input_invalid(name, e))?;
    serde_json::from_slice(&text).map_err(|e| input_invalid(name, e))
}

/// `out/` (the `out/` grant must exist).
pub fn out_dir(ctx: &JobCtx<'_>, dir: &JobDir) -> Result<PathBuf, JobError> {
    granted(ctx, dir, "out/")?;
    Ok(dir.output())
}

/// Writes an 8-bit RGBA frame as a PNG at `path` with fixed settings, so the same pixels are
/// always the same bytes. The file is closed when this returns.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), JobError> {
    let io = |e: &dyn std::fmt::Display| {
        JobError::new("IO_ERROR", format!("cannot write {}: {e}", file_name(path)))
            .with_details(json!({ "file": format!("out/{}", file_name(path)) }))
            .with_retry(fin_jobs::Retry::Backoff)
    };
    // `create` (not `create_new`): a retried attempt rewrites its own outputs.
    let f = std::fs::File::create(path).map_err(|e| io(&e))?;
    let mut w = BufWriter::new(f);
    {
        let mut enc = png::Encoder::new(&mut w, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.set_filter(png::Filter::Adaptive);
        let mut png = enc.write_header().map_err(|e| io(&e))?;
        png.write_image_data(rgba).map_err(|e| io(&e))?;
        png.finish().map_err(|e| io(&e))?;
    }
    let f = w.into_inner().map_err(|e| io(&e.error()))?;
    drop(f);
    Ok(())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
