//! The process: a `fin_jobs::worker` family named `plugin` serving `plugin.command` v1 (and
//! `plugin.hook` v1 for a plugin with [`Plugin::HOOKS`], 7F Decision 20), run in the runtime's
//! plugin mode (plan 8B Decision 21). `fin_jobs` owns the wire: `hello`, the
//! reader and its cancel routing, `alive`, `call`/`reply`, `result`/`error`, `ready` when warm,
//! exit 0 after a cold job, exit 70 when the host is gone, a caught panic answered
//! `WORKER_CRASHED` and exit 101. This module adds the plugin's own rules: a cancelled job
//! ends `JOB_CANCELLED` whatever the command returns, and a result or error is checked
//! against the 1 MiB line before `fin_jobs` sends it (`PLUGIN_PROTOCOL {reason: tooLarge}`).

use fin_jobs::codec::{self, CodecError};
use fin_jobs::worker::{Channel, Family, JobCtx};
use fin_jobs::{JobError, KindVersion, StartJob, WorkerMsg};
use fin_plugin_api::protocol::{
    COMMAND_KIND, COMMAND_KIND_VERSION, CommandRun, HOOK_KIND, HOOK_KIND_VERSION, HookRun,
    PluginContext,
};
use serde::Serialize;
use serde_json::Value;

use crate::command::{CommandCall, Plugin, cancelled, error_to_job, protocol};
use crate::hook::HookCall;
use crate::host::Host;

/// The family every plugin process names in `hello` (Decision 3: one family for all plugins).
pub const FAMILY: &str = "plugin";

/// The argument that makes a process warm (Decision 21): the last one.
pub const WARM_ARG: &str = "--warm";

static KINDS: &[KindVersion] = &[KindVersion::new(COMMAND_KIND, COMMAND_KIND_VERSION)];
static KINDS_WITH_HOOKS: &[KindVersion] = &[
    KindVersion::new(COMMAND_KIND, COMMAND_KIND_VERSION),
    KindVersion::new(HOOK_KIND, HOOK_KIND_VERSION),
];

/// Whether the process was started warm: its last argument is [`WARM_ARG`].
pub fn warm_from_args(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter().last().as_deref() == Some(WARM_ARG)
}

/// Runs `P` over stdin and stdout and exits ([`crate::main!`]).
pub fn run<P: Plugin + Send>() -> ! {
    let warm = warm_from_args(std::env::args().skip(1));
    fin_jobs::worker::run_plugin(PluginFamily::<P>::new(warm), crate::MINOR)
}

/// Runs `P` over `chan` and returns the exit code (in-memory tests).
pub fn run_on<P: Plugin + Send>(chan: Channel, warm: bool) -> i32 {
    fin_jobs::worker::run_plugin_on(PluginFamily::<P>::new(warm), chan, crate::MINOR)
}

struct PluginFamily<P> {
    plugin: P,
    warm: bool,
}

impl<P: Plugin> PluginFamily<P> {
    fn new(warm: bool) -> PluginFamily<P> {
        PluginFamily {
            plugin: P::default(),
            warm,
        }
    }
}

impl<P: Plugin + Send> Family for PluginFamily<P> {
    fn name(&self) -> &'static str {
        FAMILY
    }

    fn kinds(&self) -> &'static [KindVersion] {
        if P::HOOKS { KINDS_WITH_HOOKS } else { KINDS }
    }

    fn warm(&self) -> bool {
        self.warm
    }

    fn run(&mut self, job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
        let outcome = self.serve(job, ctx);
        if ctx.cancel_token().is_cancelled() {
            return Err(error_to_job(cancelled()));
        }
        fit(&job.id, outcome)
    }
}

impl<P: Plugin> PluginFamily<P> {
    fn serve(&mut self, job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
        let (call, context) = decode::<P>(job).map_err(error_to_job)?;
        let rev = match &call {
            Call::Command(c) => Some(c.rev),
            Call::Hook(_) => None,
        };
        #[allow(unused_mut)]
        let mut host = Host::new(ctx.reporter(), ctx.cancel_token(), context, rev);
        #[cfg(feature = "files")]
        host.files.extend_from_slice(ctx.files());
        if host.cancelled() {
            return Err(error_to_job(cancelled()));
        }
        match call {
            Call::Command(c) => encode(self.plugin.command(&host, c)),
            Call::Hook(h) => encode(self.plugin.hook(&host, h)),
        }
    }
}

/// A handler's answer as the job's result value.
fn encode<T: Serialize>(out: Result<T, crate::PluginError>) -> Result<Value, JobError> {
    let out = out.map_err(error_to_job)?;
    serde_json::to_value(out).map_err(|e| {
        error_to_job(protocol(
            "badResult",
            format!("the result does not encode: {e}"),
        ))
    })
}

/// The longest message an oversized error keeps, in bytes.
const SHORT_MESSAGE: usize = 1024;

/// `outcome` as it fits one envelope line: a result over the cap becomes `PLUGIN_PROTOCOL
/// {reason: tooLarge, bytes}`; an error over it is retried with its message cut to 1 KiB and
/// no details, then becomes that fixed error.
fn fit(job: &str, outcome: Result<Value, JobError>) -> Result<Value, JobError> {
    let error = match outcome {
        Ok(value) => {
            let msg = WorkerMsg::Result {
                job: job.to_owned(),
                value,
            };
            return match (oversize(&msg), msg) {
                (None, WorkerMsg::Result { value, .. }) => Ok(value),
                (Some(bytes), _) => Err(too_large(bytes)),
                (None, _) => unreachable!("a result message"),
            };
        }
        Err(error) => error,
    };
    let as_msg = |error| WorkerMsg::Error {
        job: job.to_owned(),
        error,
        item_errors: None,
    };
    let msg = as_msg(error);
    let Some(bytes) = oversize(&msg) else {
        return Err(error_of(msg));
    };
    let mut short = error_of(msg);
    short.message = cut(short.message, SHORT_MESSAGE);
    short.details = Value::Object(Default::default());
    let msg = as_msg(short);
    match oversize(&msg) {
        None => Err(error_of(msg)),
        Some(_) => Err(too_large(bytes)),
    }
}

/// The length of `msg`'s line when it passes the envelope's cap (`seq` counted at its widest).
fn oversize(msg: &WorkerMsg) -> Option<usize> {
    match codec::encode(u64::MAX, msg) {
        Err(CodecError::TooLong { len }) => Some(len),
        _ => None,
    }
}

fn error_of(msg: WorkerMsg) -> JobError {
    match msg {
        WorkerMsg::Error { error, .. } => error,
        _ => unreachable!("an error message"),
    }
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn cut(mut text: String, max: usize) -> String {
    if text.len() > max {
        let end = (0..=max)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        text.truncate(end);
    }
    text
}

fn too_large(bytes: usize) -> JobError {
    error_to_job(
        protocol(
            "tooLarge",
            format!("the answer is a line of {bytes} bytes, over 1 MiB"),
        )
        .with("bytes", bytes),
    )
}

/// What a job asks for.
enum Call {
    Command(CommandCall),
    Hook(HookCall),
}

/// The job's call and context; `PLUGIN_PROTOCOL {reason: kind | params | context}`. A
/// `plugin.hook` job is a `kind` error for a plugin without hooks.
fn decode<P: Plugin>(job: &StartJob) -> Result<(Call, PluginContext), crate::PluginError> {
    let v = job.kind_version;
    let call = if job.kind == COMMAND_KIND && v == COMMAND_KIND_VERSION {
        let run: CommandRun = serde_json::from_value(job.params.clone())
            .map_err(|e| protocol("params", format!("the command run does not decode: {e}")))?;
        Call::Command(run.into())
    } else if P::HOOKS && job.kind == HOOK_KIND && v == HOOK_KIND_VERSION {
        let run: HookRun = serde_json::from_value(job.params.clone())
            .map_err(|e| protocol("params", format!("the hook run does not decode: {e}")))?;
        Call::Hook(run.into())
    } else {
        let serves = if P::HOOKS {
            format!("{COMMAND_KIND} v{COMMAND_KIND_VERSION} and {HOOK_KIND} v{HOOK_KIND_VERSION}")
        } else {
            format!("{COMMAND_KIND} v{COMMAND_KIND_VERSION}")
        };
        return Err(protocol(
            "kind",
            format!("a plugin serves {serves}, not {} v{v}", job.kind),
        )
        .with("kind", job.kind.clone()));
    };
    let extra = job.extra.clone().unwrap_or(Value::Null);
    let context: PluginContext = serde_json::from_value(extra)
        .map_err(|e| protocol("context", format!("start.extra is no plugin context: {e}")))?;
    Ok((call, context))
}
