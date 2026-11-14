//! `media.capabilities` v1 (plan 6A Task 13; the journal's event is v2, 6B-1 Task 19): what this machine decodes and encodes, the GPU a
//! render would use and the worker's versions. A warm worker answers from what it already
//! holds: the codec table is fixed per build and the GPU is the one `media.look` opens (made
//! here when no look ran yet, and kept for the next job). A machine with no adapter, not even
//! a software one, answers `gpu: null`; any other GPU error is the job's.

use fin_codecs::{CodecSupport, Support};
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, StartJob};
use fin_media::RENDER_VERSION;
use fin_media::jobs::{
    CapSupport, CapabilitiesParams, CodecCapabilities, LegacyCodecCaps, LegacyMediaCapabilities,
    LegacyWorkerInfo, WorkerSandbox,
};
use serde_json::Value;

use super::look::{adapter, gpu};
use super::{Warm, job_error};
use super::{caps_report, diag};

fn support(s: Support) -> CapSupport {
    match s {
        Support::Hardware => CapSupport::Hardware,
        Support::Software => CapSupport::Software,
        Support::None => CapSupport::None,
    }
}

fn codec(c: &CodecSupport) -> LegacyCodecCaps {
    LegacyCodecCaps {
        codec: c.codec,
        decode: support(c.decode),
        encode: support(c.encode),
        backends: c.backends.clone(),
    }
}

/// The sandbox in force (sp6 §3.5.1, §15): `landlock` once the worker profile's Landlock phase
/// restricted this process (Linux, no seccomp yet; Windows after the AppContainer token check),
/// `worker` once the full profile did (macOS Seatbelt: files, no network, no process creation),
/// else `none` (Landlock disabled, or the profile not ready).
pub(crate) fn sandbox() -> WorkerSandbox {
    if !fin_sandbox::self_apply::applied() {
        WorkerSandbox::None
    } else if cfg!(target_os = "macos") {
        WorkerSandbox::Worker
    } else {
        WorkerSandbox::Landlock
    }
}

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: &mut Warm) -> Result<Value, JobError> {
    serde_json::from_value::<CapabilitiesParams>(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("params: {e}")))?;
    let gpu = match gpu(warm) {
        Ok(state) => Some(adapter(state.gpu.info())),
        Err(e) if e.code == "GPU_UNAVAILABLE" => None,
        Err(e) => return Err(e),
    };
    let result = LegacyMediaCapabilities {
        codecs: CodecCapabilities {
            video: warm.caps.video.iter().map(codec).collect(),
        },
        gpu,
        worker: LegacyWorkerInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            render_version: RENDER_VERSION,
            sandbox: sandbox(),
        },
    };
    // Once per server run in practice: the server caches the answer (Task 13). Built only
    // when diagnostics are on. The journal's kind is v2 (6B-1 Task 19), from the v2 report of the
    // backends this worker has; the answer stays v1 until Task 8's probe answers v2.
    if ctx.diagnostics() {
        let report = caps_report::report(&warm.probe, result.gpu.clone(), sandbox());
        let event = diag::capabilities_event(&report);
        ctx.diag(
            "media.capabilities",
            serde_json::to_value(event).expect("capabilities serialise"),
        );
    }
    serde_json::to_value(result)
        .map_err(|e| job_error(fin_core::CoreError::new("INTERNAL", e.to_string())))
}
