//! The napi `JobHost` (5C-1 plan, Task 6; Decisions 2, 11, 11a, 27, 41): `fin-jobhost`'s
//! [`Host`] for `finisherd`.
//!
//! The JS thread never waits for a process: `start`, `spawn`, `killGroup` and `shutdown`
//! return promises settled from the host's own threads; `openGroup` only allocates an id;
//! `send` only enqueues. Events go through [`events`] (one bounded queue, progress coalesced).
//! Values cross as JSON strings; `packages/fin-node/index.js` parses them.
//!
//! Events of a process may reach JS before its `spawn` promise settles (they travel on
//! different queues): the caller keys them by process id.
//!
//! The host is never dropped on the JS thread: its drop joins the spawn thread, which may be
//! handing an event to a JS thread that is busy dropping it.

use std::sync::Arc;

use fin_core::CoreError;
use fin_jobhost::{EventSink, GroupId, Host, HostError, HostOptions, ProcId, SendError};
use fin_jobs::HostMsg;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeCallContext;
use napi_derive::napi;
use serde_json::json;

use crate::{guarded, reject};

mod events;
mod napi_types;
mod params;

use events::{EventFn, Events, Inflight};
use napi_types::{GroupIn, Options, SpawnIn, WatchIn, decode, invalid, kill_reason};

pub use napi_types::JobHostCaps;
pub use params::{JobKindVersion, job_kind_versions};

/// The default bound of the event queue (Decision 11a).
const EVENT_QUEUE: usize = 1024;

/// A rejection for a host error: its code (`WORKER_UNAVAILABLE`, `JOBHOST_UNSUPPORTED`) with
/// `details.reason` (the host's named reason, else the kind of failure) and `details.error`.
fn host_error(e: &HostError) -> Error {
    let reason = e.reason().unwrap_or(match e {
        HostError::Unsupported => "unsupported",
        HostError::UnknownGroup(_) => "unknownGroup",
        HostError::GroupClosed(_) => "groupClosed",
        HostError::ShutDown => "shutDown",
        HostError::Program(_) => "program",
        HostError::Spawner(_) => "spawner",
        HostError::Spawn(_) => "spawn",
        HostError::Unavailable { .. } => "unavailable",
    });
    reject(
        CoreError::new(e.code(), e.to_string())
            .with_details(json!({ "reason": reason, "error": e.to_string() })),
    )
}

/// A JS number as an id: a non-negative integer.
fn id(field: &str, n: f64) -> Result<u64> {
    if n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= 2f64.powi(53) {
        Ok(n as u64)
    } else {
        Err(reject(invalid(field, format!("{n} is not an id"))))
    }
}

/// The host and its event path, dropped together off the JS thread.
struct Running {
    host: Host,
    events: Arc<Events>,
}

/// `fin-jobhost`'s host. Construct with `JobHost.start`.
#[napi]
pub struct JobHost {
    running: Option<Running>,
}

impl Drop for JobHost {
    fn drop(&mut self) {
        if let Some(r) = self.running.take() {
            r.events.close();
            let _ = std::thread::Builder::new()
                .name("jobhost-drop".into())
                .spawn(move || drop(r));
        }
    }
}

impl JobHost {
    fn host(&self) -> &Host {
        &self.running.as_ref().expect("a started host").host
    }
}

#[napi]
impl JobHost {
    /// Starts a host (`optionsJson`: `JobHostOptions`); resolves once its watchdog (Unix)
    /// or spawn broker (Windows) runs. `callback` gets every event as JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<JobHost>")]
    pub fn start<'e>(
        env: &'e Env,
        options_json: String,
        #[napi(ts_arg_type = "(json: string) => void")] callback: Function<String, ()>,
    ) -> Result<Object<'e>> {
        let o: Options = decode("options", &options_json).map_err(reject)?;
        let inflight = Inflight::new();
        let taken = inflight.clone();
        let tsfn: EventFn = callback
            .build_threadsafe_function::<String>()
            .weak::<true>()
            .callee_handled::<false>()
            .build_callback(move |ctx: ThreadsafeCallContext<String>| {
                taken.take();
                Ok(ctx.value)
            })?;
        let events = Events::start(tsfn, inflight, o.event_queue.unwrap_or(EVENT_QUEUE))?;
        let mut opts = HostOptions::new(&o.binary_dir);
        opts.timing = o.timing.timing();
        if let Some(c) = o.cgroups {
            opts.cgroups = c.mode();
        }
        if let Some(w) = o.watchdog {
            opts.watchdog = w;
        }
        let (deferred, promise) = env.create_deferred::<JobHost, _>()?;
        let unstarted = events.clone();
        let sink = {
            let events = events.clone();
            EventSink::new(move |e| events.on_event(e))
        };
        let started = std::thread::Builder::new()
            .name("jobhost-start".into())
            .spawn(move || match Host::start(opts, sink) {
                Ok(host) => deferred.resolve(move |_env| {
                    Ok(JobHost {
                        running: Some(Running { host, events }),
                    })
                }),
                Err(e) => {
                    events.close();
                    deferred.reject(host_error(&e));
                }
            });
        if let Err(e) = started {
            unstarted.close();
            return Err(reject(CoreError::new(
                "INTERNAL",
                format!("cannot start the job host thread: {e}"),
            )));
        }
        Ok(promise)
    }

    /// What this host can enforce.
    #[napi(catch_unwind)]
    pub fn caps(&self) -> JobHostCaps {
        self.host().caps().into()
    }

    /// Allocates a group (`specJson`: `{label, memoryBytes?, qos}`); its OS objects are made
    /// at its first spawn.
    #[napi(catch_unwind)]
    pub fn open_group(&self, spec_json: String) -> Result<f64> {
        let spec: GroupIn = decode("spec", &spec_json).map_err(reject)?;
        Ok(self.host().open_group(spec.into()).0 as f64)
    }

    /// Spawns a process in `group` (`reqJson`: `JobSpawnRequest`); resolves to its id.
    #[napi(catch_unwind, ts_return_type = "Promise<number>")]
    pub fn spawn<'e>(&self, env: &'e Env, group: f64, req_json: String) -> Result<Object<'e>> {
        let group = GroupId(id("group", group)?);
        let req = decode::<SpawnIn>("req", &req_json)
            .and_then(SpawnIn::request)
            .map_err(reject)?;
        let (deferred, promise) = env.create_deferred::<f64, _>()?;
        self.host().spawn(
            group,
            req,
            Box::new(move |r| match r {
                Ok(p) => deferred.resolve(move |_env| Ok(p.0 as f64)),
                Err(e) => deferred.reject(host_error(&e)),
            }),
        );
        Ok(promise)
    }

    /// Enqueues a `HostMsg` (JSON). `false`: the queue is full (the process is being killed as
    /// hung) or the process is gone. A message that is not a `HostMsg` or too long to frame
    /// throws `INVALID_REQUEST`.
    #[napi(catch_unwind)]
    pub fn send(&self, proc: f64, msg_json: String) -> Result<bool> {
        let proc = ProcId(id("proc", proc)?);
        let msg: HostMsg = decode("msg", &msg_json).map_err(reject)?;
        match self.host().send(proc, &msg) {
            Ok(()) => Ok(true),
            Err(SendError::QueueFull | SendError::Gone) => Ok(false),
            Err(SendError::Encode(e)) => Err(reject(invalid("msg", e))),
        }
    }

    /// Sets (`{job, stallMs?, timeoutMs?}` JSON) or clears (`null`) a running job's timers.
    #[napi(catch_unwind)]
    pub fn watch(&self, proc: f64, watch_json: Option<String>) -> Result<()> {
        let proc = ProcId(id("proc", proc)?);
        let w = match watch_json {
            Some(j) => Some(decode::<WatchIn>("watch", &j).map_err(reject)?.into()),
            None => None,
        };
        self.host().watch(proc, w);
        Ok(())
    }

    /// Closes the process's stdin once its queue is written.
    #[napi(catch_unwind)]
    pub fn close_stdin(&self, proc: f64) -> Result<()> {
        self.host().close_stdin(ProcId(id("proc", proc)?));
        Ok(())
    }

    /// Kills every process of the group; resolves once they all exited or the kill wait
    /// passed (a no-op on a closed or empty group).
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn kill_group<'e>(&self, env: &'e Env, group: f64, reason: String) -> Result<Object<'e>> {
        let group = GroupId(id("group", group)?);
        let reason = kill_reason(&reason).map_err(reject)?;
        let (deferred, promise) = env.create_deferred::<(), _>()?;
        self.host().kill_group(
            group,
            reason,
            Box::new(move || deferred.resolve(|_env| Ok(()))),
        );
        Ok(promise)
    }

    /// Closes the group: later spawns fail, live processes are killed, and once empty it is
    /// released (queued on the spawn thread).
    #[napi(catch_unwind)]
    pub fn close_group(&self, group: f64) -> Result<()> {
        self.host().close_group(GroupId(id("group", group)?));
        Ok(())
    }

    /// Closes every stdin, kills what is left after `graceMs`, resolves once every process
    /// exited (or the kill wait passed). Later spawns fail.
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn shutdown<'e>(&self, env: &'e Env, grace_ms: u32) -> Result<Object<'e>> {
        let (deferred, promise) = env.create_deferred::<(), _>()?;
        self.host()
            .shutdown(grace_ms, Box::new(move || deferred.resolve(|_env| Ok(()))));
        Ok(promise)
    }
}

/// Test hooks (`build:test` only).
#[cfg(feature = "test-hooks")]
#[napi]
impl JobHost {
    /// Kills one process as a crash would (reported `exited`, signal `kill`).
    #[napi(catch_unwind)]
    pub fn kill_proc_for_test(&self, proc: f64) -> Result<()> {
        self.host().kill_proc_for_test(ProcId(id("proc", proc)?));
        Ok(())
    }

    /// The process's exit is held for `holdMs` once observed, so a kill of its group leaves it
    /// abandoned after the kill wait.
    #[napi(catch_unwind)]
    pub fn abandon_for_test(&self, proc: f64, hold_ms: u32) -> Result<()> {
        let hold = std::time::Duration::from_millis(u64::from(hold_ms));
        self.host()
            .abandon_for_test(ProcId(id("proc", proc)?), hold);
        Ok(())
    }

    /// Events wait `ms` before they are handed to JS, as with a busy JS thread.
    #[napi(catch_unwind)]
    pub fn pause_events_for_test(&self, ms: u32) {
        if let Some(r) = &self.running {
            r.events.pause_for_test(ms);
        }
    }
}

/// `validateJobParams`: resolves the kind's Rust params type; answers `{ok, params}` or
/// `{ok: false, errors}` as JSON.
#[napi(catch_unwind)]
pub fn validate_job_params(kind: String, version: u32, params_json: String) -> Result<String> {
    guarded(
        || {
            let params: serde_json::Value = decode("params", &params_json).map_err(reject)?;
            Ok(params::validate(&kind, version, &params).to_string())
        },
        || {},
    )
}

/// A group memory rule (`{fixedMiB?, ramFraction?, capMiB?}` JSON) in bytes for a machine with
/// `totalBytes` of RAM (Decision 20); `null` without any bound.
#[napi(catch_unwind)]
pub fn resolve_memory_rule(rule_json: String, total_bytes: f64) -> Result<Option<f64>> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RuleIn {
        #[serde(rename = "fixedMiB", default)]
        fixed_mib: Option<u64>,
        #[serde(rename = "ramFraction", default)]
        ram_fraction: Option<f64>,
        #[serde(rename = "capMiB", default)]
        cap_mib: Option<u64>,
    }
    let r: RuleIn = decode("rule", &rule_json).map_err(reject)?;
    let rule = fin_jobhost::MemoryRule {
        fixed_mib: r.fixed_mib,
        ram_fraction: r.ram_fraction,
        cap_mib: r.cap_mib,
    };
    Ok(rule
        .resolve(id("totalBytes", total_bytes)?)
        .map(|b| b as f64))
}
