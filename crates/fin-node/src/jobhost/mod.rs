//! The napi `JobHost` (5C-1 plan, Task 6; Decisions 2, 11, 11a, 27, 41): `fin-jobhost`'s
//! [`Host`] for `finisherd`.
//!
//! The JS thread never waits for a process: `start`, `spawn`, `killGroup` and `shutdown`
//! return promises settled from the host's own threads; `openGroup` only allocates an id;
//! `send` only enqueues. Events go through [`events`] (one bounded queue, progress coalesced).
//! Values cross as JSON strings in `fin_jobhost::wire`'s shapes; `packages/fin-node/jobhost.js`
//! parses them. Every promise is a [`Settle`]: settled even when its answer is lost.
//!
//! A spawn resolves **through the event queue**: its answer (`{type: "_spawn", spawn, proc}`,
//! consumed by `jobhost.js`) is handed over by the spawn thread before the process's
//! `spawned` and before its readers start, so no event of a process reaches JS before its
//! `spawn` resolves. The native promise only rejects (a refused spawn); on success it resolves
//! with nothing.
//!
//! The host is never dropped on the JS thread: its drop joins the spawn thread, which may be
//! handing an event to a JS thread that is busy dropping it.

use std::sync::Arc;

use fin_core::CoreError;
use fin_jobhost::wire::{
    JobGroupSpec, JobHostCaps, JobHostOptions, JobMemoryRule, JobSpawnRequest, JobWatch,
};
use fin_jobhost::{EventSink, GroupId, Host, HostError, HostOptions, ProcId, SendError};
use fin_jobs::HostMsg;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeCallContext;
use napi_derive::napi;
use serde_json::{Value, json};

use crate::{guarded, reject};

mod events;
mod napi_types;
mod params;
mod settle;

use events::{EventFn, Events, Handed, Inflight};
use napi_types::{decode, decode_value, invalid, kill_reason, wire};
use settle::Settle;

pub use params::{JobKindVersion, job_kind_versions};

/// The default bounds of the event queue (Decision 11a): events, and their JSON bytes.
const EVENT_QUEUE: usize = 1024;
const EVENT_QUEUE_BYTES: usize = 16 << 20;

/// A rejection for a host error: its code (`JOBHOST_STATE` for a group or host the caller
/// should know is gone, else `WORKER_UNAVAILABLE`) with `details.reason` (the host's named
/// reason, else the kind of failure) and `details.error`.
fn host_error(e: &HostError) -> Error {
    let reason = e.reason().unwrap_or(match e {
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

/// A JS number as a `u32` (a version, a duration in ms).
fn u32_of(field: &str, n: f64) -> Result<u32> {
    let v = id(field, n)?;
    u32::try_from(v).map_err(|_| reject(invalid(field, format!("{n} is over {}", u32::MAX))))
}

/// The ways `start` can be made to fail (`failForTest`, test builds).
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
enum FailForTest {
    None,
    /// The `jobhost-start` thread does not start.
    StartThread,
    /// `Host::start` panics.
    HostStart,
    /// Every spawn command panics before it answers.
    Spawn,
}

/// The options without `failForTest`, and what it asks.
fn options(json_text: &str) -> std::result::Result<(JobHostOptions, FailForTest), CoreError> {
    #[allow(unused_mut)]
    let mut v: Value = decode("options", json_text)?;
    #[allow(unused_mut)]
    let mut fail = FailForTest::None;
    #[cfg(feature = "test-hooks")]
    if let Some(f) = v.as_object_mut().and_then(|o| o.remove("failForTest")) {
        fail = match f.as_str() {
            Some("startThread") => FailForTest::StartThread,
            Some("hostStart") => FailForTest::HostStart,
            Some("spawn") => FailForTest::Spawn,
            _ => {
                return Err(invalid(
                    "options.failForTest",
                    format!("{f} is not a failure"),
                ));
            }
        };
    }
    Ok((decode_value("options", v)?, fail))
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
    fn running(&self) -> &Running {
        self.running.as_ref().expect("a started host")
    }

    fn host(&self) -> &Host {
        &self.running().host
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
        let (o, fail) = options(&options_json).map_err(reject)?;
        let inflight = Inflight::new();
        let taken = inflight.clone();
        let tsfn: EventFn = callback
            .build_threadsafe_function::<Handed>()
            .weak::<true>()
            .callee_handled::<false>()
            .build_callback(move |ctx: ThreadsafeCallContext<Handed>| {
                taken.take(&ctx.value);
                Ok(ctx.value.json)
            })?;
        let bound = |n: Option<u32>, d: usize| n.map_or(d, |n| n as usize);
        let events = Events::start(
            tsfn,
            inflight,
            bound(o.event_queue, EVENT_QUEUE),
            bound(o.event_queue_bytes, EVENT_QUEUE_BYTES),
        )?;
        let mut opts = HostOptions::new(&o.binary_dir);
        // Plan 7C Task 7: the worker and plugin profiles, where they are ready and enforced.
        opts.spawner = std::sync::Arc::new(fin_sandbox::SandboxSpawner::new(o.server_data.clone()));
        opts.timing = o.timing.unwrap_or_default().timing();
        if let Some(c) = o.cgroups {
            opts.cgroups = c.into();
        }
        if let Some(w) = o.watchdog {
            opts.watchdog = w;
        }
        // F2: only a host whose own mechanism is disabled accepts a worker's `selfSandbox: none`.
        opts.unconfined_ok = matches!(
            fin_sandbox::status::mechanism(),
            fin_sandbox::Mechanism::Disabled {
                reason: fin_sandbox::DisabledReason::LandlockDisabled
            }
        );
        #[cfg(feature = "test-hooks")]
        {
            opts.panic_on_start_for_test = fail == FailForTest::HostStart;
            opts.panic_on_spawn_for_test = fail == FailForTest::Spawn;
        }
        let (settle, promise) = Settle::<JobHost>::new(env, "JobHost.start")?;
        let unstarted = events.clone();
        let sink = {
            let events = events.clone();
            EventSink::new(move |e| events.on_event(e))
        };
        let run = move || match Host::start(opts, sink) {
            Ok(host) => settle.resolve(move |_env| {
                Ok(JobHost {
                    running: Some(Running { host, events }),
                })
            }),
            Err(e) => {
                events.close();
                settle.reject(host_error(&e));
            }
        };
        let builder = std::thread::Builder::new().name("jobhost-start".into());
        let started = if fail == FailForTest::StartThread {
            drop(run);
            Err(std::io::Error::other("test: no start thread"))
        } else {
            let closer = unstarted.clone();
            builder.spawn(move || {
                // A panic drops `settle` (rejecting the promise) and must not leave the events
                // open.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).is_err() {
                    closer.close();
                }
            })
        };
        if started.is_err() {
            // `run` is dropped with its `settle`: the promise rejects `INTERNAL`.
            unstarted.close();
        }
        Ok(promise)
    }

    /// What this host can enforce (`JobHostCaps` JSON).
    #[napi(catch_unwind)]
    pub fn caps(&self) -> Result<String> {
        crate::encode(&JobHostCaps::from(self.host().caps()))
    }

    /// Allocates a group (`specJson`: `JobGroupSpec`); its OS objects are made at its first
    /// spawn.
    #[napi(catch_unwind)]
    pub fn open_group(&self, spec_json: String) -> Result<f64> {
        let spec: JobGroupSpec = decode("spec", &spec_json).map_err(reject)?;
        Ok(self.host().open_group(spec.into()).0 as f64)
    }

    /// Spawns a process in `group` (`reqJson`: `JobSpawnRequest`). Success is answered on the
    /// event queue as `{type: "_spawn", spawn, proc}` (before any event of the process); the
    /// promise rejects for a refused spawn and otherwise resolves with nothing.
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn spawn<'e>(
        &self,
        env: &'e Env,
        group: f64,
        req_json: String,
        spawn: f64,
    ) -> Result<Object<'e>> {
        let group = GroupId(id("group", group)?);
        let spawn = id("spawn", spawn)?;
        let req = decode::<JobSpawnRequest>("req", &req_json)
            .and_then(|r| r.request().map_err(wire))
            .map_err(reject)?;
        let (settle, promise) = Settle::<()>::new(env, "JobHost.spawn")?;
        let events = self.running().events.clone();
        self.host().spawn(
            group,
            req,
            Box::new(move |r| match r {
                Ok(p) => {
                    events.deliver_now(
                        json!({ "type": "_spawn", "spawn": spawn, "proc": p.0 }).to_string(),
                    );
                    settle.resolve(|_env| Ok(()));
                }
                Err(e) => settle.reject(host_error(&e)),
            }),
        );
        Ok(promise)
    }

    /// Enqueues a `HostMsg` (JSON): `ok`; `full` (the queue is full: the process is being
    /// killed as hung); `gone` (it exited, its stdin is closed, or the id is unknown). How the
    /// process ended comes with its `exit`. A message that is not a `HostMsg` or too long to
    /// frame throws `INVALID_REQUEST`.
    #[napi(catch_unwind, ts_return_type = "\"ok\" | \"full\" | \"gone\"")]
    pub fn send(&self, proc: f64, msg_json: String) -> Result<String> {
        let proc = ProcId(id("proc", proc)?);
        let msg: HostMsg = decode("msg", &msg_json).map_err(reject)?;
        match self.host().send(proc, &msg) {
            Ok(()) => Ok("ok".into()),
            Err(SendError::QueueFull) => Ok("full".into()),
            Err(SendError::Gone) => Ok("gone".into()),
            Err(SendError::Encode(e)) => Err(reject(invalid("msg", e))),
        }
    }

    /// Sets (`JobWatch` JSON) or clears (`null`) a running job's timers.
    #[napi(catch_unwind)]
    pub fn watch(&self, proc: f64, watch_json: Option<String>) -> Result<()> {
        let proc = ProcId(id("proc", proc)?);
        let w = match watch_json {
            Some(j) => Some(decode::<JobWatch>("watch", &j).map_err(reject)?.into()),
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
        let (settle, promise) = Settle::<()>::new(env, "JobHost.killGroup")?;
        self.host().kill_group(
            group,
            reason,
            Box::new(move || settle.resolve(|_env| Ok(()))),
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
    pub fn shutdown<'e>(&self, env: &'e Env, grace_ms: f64) -> Result<Object<'e>> {
        let grace_ms = u32_of("graceMs", grace_ms)?;
        let (settle, promise) = Settle::<()>::new(env, "JobHost.shutdown")?;
        self.host()
            .shutdown(grace_ms, Box::new(move || settle.resolve(|_env| Ok(()))));
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
/// `{ok: false, errors}` as JSON. A `version` that is not a `u32` throws `INVALID_REQUEST`.
#[napi(catch_unwind)]
pub fn validate_job_params(kind: String, version: f64, params_json: String) -> Result<String> {
    guarded(
        || {
            let version = u32_of("version", version)?;
            let params: Value = decode("params", &params_json).map_err(reject)?;
            Ok(params::validate(&kind, version, &params, params_json.len()).to_string())
        },
        || {},
    )
}

/// A group memory rule (`JobMemoryRule` JSON) in bytes for a machine with `totalBytes` of RAM
/// (Decision 20); `null` without any bound.
#[napi(catch_unwind)]
pub fn resolve_memory_rule(rule_json: String, total_bytes: f64) -> Result<Option<f64>> {
    let rule: fin_jobhost::MemoryRule = decode::<JobMemoryRule>("rule", &rule_json)
        .map_err(reject)?
        .into();
    Ok(rule
        .resolve(id("totalBytes", total_bytes)?)
        .map(|b| b as f64))
}
