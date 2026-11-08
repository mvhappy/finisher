//! `media.capabilities` v1 (plan 6A Task 13): what this machine decodes and encodes, the GPU a
//! render would use and the worker's versions. A warm worker answers from what it already
//! holds: the codec table is fixed per build and the GPU is the one `media.look` opens (made
//! here when no look ran yet, and kept for the next job). A machine with no adapter, not even
//! a software one, answers `gpu: null`; any other GPU error is the job's.

use fin_codecs::{CodecSupport, Support};
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, StartJob};
use fin_media::RENDER_VERSION;
use fin_media::jobs::{
    CapSupport, CapabilitiesParams, CodecCapabilities, CodecCaps, MediaCapabilities, WorkerInfo,
};
use serde_json::Value;

use super::diag;
use super::look::{adapter, gpu};
use super::{Warm, job_error};

fn support(s: Support) -> CapSupport {
    match s {
        Support::Hardware => CapSupport::Hardware,
        Support::Software => CapSupport::Software,
        Support::None => CapSupport::None,
    }
}

fn codec(c: &CodecSupport) -> CodecCaps {
    CodecCaps {
        codec: c.codec,
        decode: support(c.decode),
        encode: support(c.encode),
        backends: c.backends.clone(),
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
    let result = MediaCapabilities {
        codecs: CodecCapabilities {
            video: warm.caps.video.iter().map(codec).collect(),
        },
        gpu,
        worker: WorkerInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            render_version: RENDER_VERSION,
        },
    };
    // Once per server run in practice: the server caches the answer (Task 13). Built only
    // when diagnostics are on.
    if ctx.diagnostics() {
        let event = diag::capabilities_event(&result);
        ctx.diag(
            "media.capabilities",
            serde_json::to_value(event).expect("capabilities serialise"),
        );
    }
    serde_json::to_value(result)
        .map_err(|e| job_error(fin_core::CoreError::new("INTERNAL", e.to_string())))
}
