//! `media.probeDeep` v1 (plan 6B-1 Task 11; sub-project 6 spec §6.3): the decode-level probe of
//! one asset. The manifest (`in/manifest.json`, `fin-node`'s `prepareAssetJob`) names the asset
//! and its original; the asset's fingerprint there must be the one the job was submitted for
//! (`ASSET_CHANGED`). The original opens through `fin-library`'s no-follow opener, checked
//! against the manifest's size, mtime and file id (`ORIGINALS_MISSING {reason}`), is demuxed
//! under the default limits, and:
//!
//! - **HDR:** the sample entry's `mdcv`/`clli`/`dvcC` (`hdrSource: container`), else the
//!   H.264/HEVC SEI of the first sample (`hdrSource: sei`, `fin_container::demux::in_band_hdr`);
//! - **decode:** the first presented frame through `fin_codecs::open_decoder` with the
//!   worker's `ProbeResult` (6A's software pair until Task 8's probe). Its backend, the
//!   backend's vendor class (`fin_media::backend`), whether it ran on hardware and the
//!   picture's size and format are reported. A codec no decoder here takes, or a bitstream the
//!   decoder refuses, is `decodable: false` with the error as `failure`: the probe itself
//!   succeeded. A container that does not parse fails the job.
//!
//! Nothing is written: the result is the job's answer.

use fin_codecs::{DecodeThreads, FrameReader};
use fin_container::{Demuxer, Limits};
use fin_core::CoreError;
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, StartJob};
use fin_media::backend::backend;
use fin_media::jobs::{
    FirstFrame, MANIFEST_VERSION, ProbeDeepParams, ProbeDeepResult, ProbeFailure, RenderManifest,
};
use serde_json::{Value, json};

use super::inputs::{self, MANIFEST_CAP};
use super::open_cache::{WORKER_MAX_PIXELS, open_original_in};
use super::{Warm, job_error};

fn input_invalid(message: String, details: Value) -> JobError {
    JobError::new("JOB_INPUT_INVALID", message).with_details(details)
}

fn failure(e: CoreError) -> ProbeFailure {
    ProbeFailure {
        code: e.code,
        message: e.message,
        details: e.details,
    }
}

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: &mut Warm) -> Result<Value, JobError> {
    let p: ProbeDeepParams = serde_json::from_value(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("params: {e}")))?;
    let dir = inputs::job_dir(ctx)?;
    let manifest: RenderManifest = inputs::read_json(ctx, &dir, "in/manifest.json", MANIFEST_CAP)?;
    if manifest.version != MANIFEST_VERSION {
        return Err(input_invalid(
            format!(
                "manifest version {} (this worker reads {MANIFEST_VERSION})",
                manifest.version
            ),
            json!({ "file": "in/manifest.json", "version": manifest.version }),
        ));
    }
    let asset = manifest.asset(&p.asset).ok_or_else(|| {
        input_invalid(
            format!("the manifest does not name asset {}", p.asset),
            json!({ "file": "in/manifest.json", "asset": p.asset }),
        )
    })?;
    if asset.fingerprint != p.fingerprint {
        return Err(JobError::new(
            "ASSET_CHANGED",
            format!(
                "asset {} is not the content the probe was asked for",
                p.asset
            ),
        )
        .with_details(json!({ "asset": p.asset, "reason": "fingerprint" })));
    }
    let missing = |reason: &str, message: String| {
        JobError::new("ORIGINALS_MISSING", message)
            .with_details(json!({ "asset": p.asset, "reason": reason }))
    };
    let original = asset.original.as_ref().ok_or_else(|| {
        missing(
            "notFound",
            format!("asset {}'s file is not on this machine", p.asset),
        )
    })?;
    let file = open_original_in(original, warm.sources.confined())
        .map_err(|r| missing(r.reason, r.message))?;
    let decode_handle = file
        .try_clone()
        .map_err(|e| job_error(CoreError::new("IO_ERROR", e.to_string())))?;

    let limits = Limits::default();
    let mut demux = Demuxer::open(file, &limits).map_err(job_error)?;
    let info = demux.info().video.clone().ok_or_else(|| {
        job_error(
            CoreError::new("NOT_FOUND", "the file has no video track")
                .with_details(json!({ "kind": "videoTrack", "asset": p.asset })),
        )
    })?;
    // In-band SEI fills what the boxes lack; a sample that cannot be read is no SEI (damaged
    // metadata is no reason to fail a probe; the decode below reports the sample itself).
    let (hdr, hdr_source) = fin_media::jobs::merge_hdr(info.hdr, || {
        fin_container::demux::in_band_hdr(&mut demux).ok().flatten()
    });
    drop(demux);
    if ctx.cancelled().is_some() {
        return Err(JobError::cancelled());
    }

    let mut result = ProbeDeepResult {
        asset: p.asset.clone(),
        fingerprint: p.fingerprint.clone(),
        codec: info.codec,
        decodable: false,
        backend: None,
        class: None,
        hw: None,
        hdr,
        hdr_source,
        first_frame: None,
        failure: None,
    };
    let decoded = FrameReader::open_with_limit(
        decode_handle,
        &limits,
        &warm.probe,
        DecodeThreads::default(),
        WORKER_MAX_PIXELS,
    )
    .and_then(|mut reader| {
        let name = reader.backend().0;
        let frame = reader.frame(0)?;
        Ok((name, frame.hw, frame.size, frame.format))
    });
    match decoded {
        Ok((name, hw, size, format)) => {
            result.decodable = true;
            result.backend = Some(name.to_string());
            result.class = backend(name).map(|b| b.class);
            result.hw = Some(hw);
            result.first_frame = Some(FirstFrame {
                width: size.width,
                height: size.height,
                format,
            });
        }
        Err(e) => result.failure = Some(failure(e)),
    }
    serde_json::to_value(result).map_err(|e| job_error(CoreError::new("INTERNAL", e.to_string())))
}
