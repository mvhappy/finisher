//! The process: a `fin_jobs::worker` family named `plugin` serving `plugin.command` v1, run in
//! the runtime's plugin mode (plan 8B Decision 21). `fin_jobs` owns the wire: `hello`, the
//! reader and its cancel routing, `alive`, `call`/`reply`, `result`/`error`, `ready` when warm,
//! exit 0 after a cold job, exit 70 when the host is gone, a caught panic answered
//! `WORKER_CRASHED` and exit 101.

use fin_jobs::worker::{Channel, Family, JobCtx};
use fin_jobs::{JobError, KindVersion, StartJob};
use fin_plugin_api::protocol::{COMMAND_KIND, COMMAND_KIND_VERSION, CommandRun, PluginContext};
use serde_json::Value;

use crate::command::{CommandCall, Plugin, cancelled, error_to_job, protocol};
use crate::host::Host;

/// The family every plugin process names in `hello` (Decision 3: one family for all plugins).
pub const FAMILY: &str = "plugin";

/// The argument that makes a process warm (Decision 21): the last one.
pub const WARM_ARG: &str = "--warm";

static KINDS: &[KindVersion] = &[KindVersion::new(COMMAND_KIND, COMMAND_KIND_VERSION)];

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
        KINDS
    }

    fn warm(&self) -> bool {
        self.warm
    }

    fn run(&mut self, job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
        let (call, context) = decode(job).map_err(error_to_job)?;
        #[allow(unused_mut)]
        let mut host = Host::new(ctx.reporter(), ctx.cancel_token(), context, call.rev);
        #[cfg(feature = "files")]
        host.files.extend_from_slice(ctx.files());
        if host.cancelled() {
            return Err(error_to_job(cancelled()));
        }
        let out = self.plugin.command(&host, call).map_err(error_to_job)?;
        serde_json::to_value(out).map_err(|e| {
            error_to_job(protocol(
                "result",
                format!("the result does not encode: {e}"),
            ))
        })
    }
}

/// The job's command and context; `PLUGIN_PROTOCOL {reason: kind | params | context}`.
fn decode(job: &StartJob) -> Result<(CommandCall, PluginContext), crate::PluginError> {
    if job.kind != COMMAND_KIND || job.kind_version != COMMAND_KIND_VERSION {
        return Err(protocol(
            "kind",
            format!(
                "a plugin serves {COMMAND_KIND} v{COMMAND_KIND_VERSION}, not {} v{}",
                job.kind, job.kind_version
            ),
        )
        .with("kind", job.kind.clone()));
    }
    let run: CommandRun = serde_json::from_value(job.params.clone())
        .map_err(|e| protocol("params", format!("the command run does not decode: {e}")))?;
    let extra = job.extra.clone().unwrap_or(Value::Null);
    let context: PluginContext = serde_json::from_value(extra)
        .map_err(|e| protocol("context", format!("start.extra is no plugin context: {e}")))?;
    Ok((run.into(), context))
}
