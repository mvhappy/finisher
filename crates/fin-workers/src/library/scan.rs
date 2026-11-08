//! `library.scan` (Decisions 26, 27, 33): walks the root with `fin_library::walk` and hands
//! each batch to the indexer through `out/` and `call library.batch`.

use std::cell::Cell;
use std::fs::File;
use std::io::Write as _;
use std::path::Path;

use fin_jobs::worker::JobCtx;
use fin_jobs::{CancelReason, JobError, Progress, ProgressUnit, Retry, StartJob};
use fin_library::batchfile;
use fin_library::params::{
    BatchCall, CALL_BATCH, DIRS_FILE, LibraryScanParams, LibraryScanResult, OUT_DIR, ScanMode,
};
use fin_library::walk::THREADS_LOCAL;
use fin_library::{Excludes, ScanBatch, WalkError, WalkMode, WalkOpts, WalkStop, walk};
use serde_json::Value;

use super::common::{
    CancelBridge, decode, grant, offline, params_invalid, policy_name, root_path, source,
};
use super::jobfiles;

/// The most `progress` messages one batch's directories send.
const PROGRESS_PER_BATCH: usize = 64;

fn io_error(what: &str, e: &std::io::Error) -> JobError {
    JobError::new("IO_ERROR", format!("{what}: {e}")).with_retry(Retry::Backoff)
}

/// Writes `batch` to `out/<name>`, whole, and closes it before the call names it.
fn write_batch(out: &Path, name: &str, batch: &ScanBatch) -> Result<(), JobError> {
    let bytes = batchfile::encode(batch).map_err(|e| io_error("encoding a batch", &e))?;
    let mut f = File::create(out.join(name)).map_err(|e| io_error("writing a batch", &e))?;
    f.write_all(&bytes)
        .and_then(|()| f.flush())
        .map_err(|e| io_error("writing a batch", &e))
}

fn mode(
    p: &LibraryScanParams,
    known: Option<fin_library::KnownDirs>,
) -> Result<WalkMode, JobError> {
    match p.mode {
        ScanMode::Full => Ok(WalkMode::Full {
            generation: p.generation,
            known,
        }),
        ScanMode::Dirs => match &p.dirs {
            Some(dirs) if !dirs.is_empty() => {
                for d in dirs {
                    d.validate()
                        .map_err(|e| params_invalid(format!("params.dirs: {e}")))?;
                }
                Ok(WalkMode::Dirs {
                    dirs: dirs.clone(),
                    known,
                })
            }
            _ => Err(params_invalid("params.dirs: mode dirs needs directories")),
        },
        ScanMode::Poll => known
            .map(|known| WalkMode::Poll { known })
            .ok_or_else(|| params_invalid("mode poll needs the in/dirs.ndjson grant")),
    }
}

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    let p: LibraryScanParams = decode(job)?;
    let root = root_path(&p.root)?;
    if p.mode != ScanMode::Dirs && p.dirs.is_some() {
        return Err(params_invalid(
            "params.dirs: only mode dirs takes directories",
        ));
    }
    let out = grant(ctx, OUT_DIR)?;
    let known = match ctx.file(DIRS_FILE) {
        Some(g) => Some(jobfiles::known_dirs(&g.path)?),
        None => None,
    };
    let walk_mode = mode(&p, known)?;
    let excludes =
        Excludes::new(&p.excludes).map_err(|e| params_invalid(format!("params.excludes: {e}")))?;
    let mut opts = WalkOpts::new(excludes);
    opts.threads = p
        .threads
        .map_or(THREADS_LOCAL, |t| (t as usize).clamp(1, 256));
    opts.stored = p.volume.clone();
    if let Some(t) = p.timing {
        #[cfg(feature = "test-hooks")]
        {
            opts.timing = t;
        }
        #[cfg(not(feature = "test-hooks"))]
        {
            let _ = t;
            return Err(params_invalid("params.timing: only test builds take it"));
        }
    }
    let src = source(&root)?;
    let bridge = CancelBridge::new(ctx.cancel_token());

    let host_error: Cell<Option<JobError>> = Cell::new(None);
    let mut listed = 0u64;
    let mut sink = |batch: ScanBatch| -> Result<(), WalkStop> {
        let fail = |e: JobError| {
            let stop = WalkStop(e.message.clone());
            host_error.set(Some(e));
            stop
        };
        let name = batchfile::file_name(batch.seq);
        write_batch(&out, &name, &batch).map_err(fail)?;
        let call = BatchCall {
            file: name.clone(),
            seq: batch.seq,
        };
        let params = serde_json::to_value(&call)
            .map_err(|e| fail(JobError::new("INTERNAL", e.to_string())))?;
        ctx.call(CALL_BATCH, params).map_err(fail)?;
        // The reply means the indexer applied the file; it is not needed again.
        let _ = std::fs::remove_file(out.join(&name));
        // One progress per directory, thinned to at most PROGRESS_PER_BATCH a batch (a fast
        // scan lists thousands of directories per batch; a slow one, whose stall timer
        // matters, lists a few).
        let done = batch.dirs_done.iter().filter(|d| d.complete).count();
        let step = done.div_ceil(PROGRESS_PER_BATCH).max(1);
        for i in 1..=done {
            listed += 1;
            if i % step == 0 || i == done {
                let p = Progress::new(listed as f64, ProgressUnit::Items).phase("directories");
                ctx.progress(p);
            }
        }
        Ok(())
    };
    let outcome = walk(src, walk_mode, &opts, &mut sink, &bridge.token());
    match outcome {
        Ok(s) => serde_json::to_value(LibraryScanResult {
            identity: s.identity,
            listed: s.listed,
            incomplete: s.incomplete,
            timeouts: s.timeouts,
            unchanged: s.unchanged,
            skipped: s.skipped,
            entries: s.entries,
            batches: s.batches,
            stuck: s.stuck,
            dataless_policy: policy_name(),
        })
        .map_err(|e| JobError::new("INTERNAL", e.to_string())),
        Err(WalkError::Offline(state)) => Err(offline(&state)),
        Err(WalkError::Cancelled) => {
            // The catalog is the scan's state (generation and known directories), so a
            // pause only marks that it stopped.
            if ctx.cancelled() == Some(CancelReason::Pause) {
                ctx.checkpoint(serde_json::json!({ "paused": true }))?;
            }
            Err(JobError::cancelled())
        }
        Err(WalkError::Stopped(stop)) => Err(host_error
            .take()
            .unwrap_or_else(|| JobError::new("LIBRARY_BATCH_REFUSED", stop.to_string()))),
        Err(WalkError::Thread(e)) => Err(io_error("starting a read thread", &e)),
    }
}
