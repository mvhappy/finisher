//! The worker runtime (Decision 6), in order: the self-sandbox entry (Decision 9a); on
//! Windows, crash UI off; `hello`. Then it reads `start`, runs the family's `run` with a
//! [`JobCtx`], answers `result` or `error`, and exits 0 (cold) or sends `ready` (warm).
//!
//! - stdin EOF at any time → exit [`crate::EXIT_HOST_GONE`] at once;
//! - `cancel` sets the job's [`CancelToken`]; the family returns [`JobError::cancelled`];
//! - `shutdown` (sent only to an idle worker) exits 0;
//! - a panic in `run` is answered `error {WORKER_CRASHED, retry: backoff}` and the process
//!   exits 101 (a warm worker is not reused after a panic);
//! - before `ready`, a warm family has closed every file and unmapped every mapping under the
//!   job directory (a family contract).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc;

use serde_json::Value;

use crate::error::JobError;
use crate::msg_host::Start;
use crate::msg_worker::{Hello, KindVersion, Role, WorkerMsg};
use crate::{EXIT_HOST_GONE, EXIT_HOST_PROTOCOL, EXIT_SELF_SANDBOX, MINOR, PROTOCOL, StartJob};

mod calls;
mod ctx;
mod io;

pub use ctx::{CancelToken, JobCtx, Reporter};
pub use io::Channel;
pub(crate) use io::{Inbound, Shared, spawn_alive, spawn_reader};

/// The exit code after a caught panic.
pub const EXIT_PANIC: i32 = 101;

/// A worker family: its kinds and the code that runs one job.
pub trait Family: Send {
    fn name(&self) -> &'static str;
    fn kinds(&self) -> &'static [KindVersion];
    /// A warm worker sends `ready` after each job and takes the next `start`.
    fn warm(&self) -> bool {
        false
    }
    fn run(&mut self, job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError>;

    /// Rewrites the `hello` before it is sent (test families' protocol violations).
    #[cfg(feature = "test-hooks")]
    fn hello_for_test(&self, _hello: &mut Hello) {}
}

/// Runs `family` over stdin/stdout and exits the process.
pub fn run(family: impl Family) -> ! {
    std::process::exit(run_on(family, Channel::stdio()))
}

/// The self-sandbox entry and crash-UI suppression every runtime starts with. `Err` is the
/// exit code.
pub(crate) fn enter(chan: &Channel) -> Result<Option<String>, i32> {
    let profile = crate::sandbox::enter(chan.self_sandbox.as_deref()).map_err(|e| {
        eprintln!("fin-jobs: {e}");
        EXIT_SELF_SANDBOX
    })?;
    crash_ui_off();
    Ok(profile)
}

/// Runs `family` over `chan` and returns the exit code (in-memory tests; [`run`] exits with
/// it). With `chan.hard_exit`, host loss exits the process from the reader thread.
pub fn run_on(mut family: impl Family, chan: Channel) -> i32 {
    let profile = match enter(&chan) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let shared = Shared::new(chan.output, chan.hard_exit, false);
    #[allow(unused_mut)]
    let mut hello = Hello {
        v: PROTOCOL,
        minor: MINOR,
        role: Role::Worker,
        family: family.name().to_owned(),
        kinds: family.kinds().to_vec(),
        warm: family.warm(),
        envelope: Some(crate::envelope_id().to_owned()),
        self_sandbox: profile,
        pid: chan.pid,
        kind: None,
        id: None,
    };
    #[cfg(feature = "test-hooks")]
    family.hello_for_test(&mut hello);
    let _ = shared.out.send(&WorkerMsg::Hello(hello));
    let (tx, rx) = mpsc::channel();
    spawn_reader(chan.input, &shared, tx);
    spawn_alive(&shared);
    let code = loop {
        match rx.recv() {
            Ok(Inbound::Start(start, token)) => {
                if let Some(code) = run_job(&mut family, &start, token, &shared) {
                    break code;
                }
            }
            Ok(Inbound::Shutdown) => break 0,
            Ok(Inbound::Attach(a, _)) => {
                shared.finish(&a.job);
                eprintln!("fin-jobs: a worker got `attach`; ignored");
            }
            Ok(Inbound::Protocol) => break EXIT_HOST_PROTOCOL,
            Ok(Inbound::HostGone) | Err(_) => break EXIT_HOST_GONE,
        }
    };
    shared.stop_alive();
    code
}

/// Runs one job; `Some(code)` when the process ends after it.
fn run_job(
    family: &mut impl Family,
    start: &Start,
    token: CancelToken,
    shared: &Arc<Shared>,
) -> Option<i32> {
    let job = start.job.id.clone();
    shared.set_alive_interval(start.limits.alive_ms);
    let outcome = {
        let mut ctx = JobCtx::new(start, shared, token);
        catch_unwind(AssertUnwindSafe(|| family.run(&start.job, &mut ctx)))
    };
    shared.finish(&job);
    let (msg, panicked) = match outcome {
        Ok(Ok(value)) => (
            WorkerMsg::Result {
                job: job.clone(),
                value,
            },
            false,
        ),
        Ok(Err(error)) => (error_msg(&job, error), false),
        Err(panic) => (
            error_msg(&job, JobError::crashed(panic_text(&*panic))),
            true,
        ),
    };
    if let Err(e) = shared.out.send(&msg) {
        let err = JobError::new("RESULT_TOO_LARGE", e.to_string());
        let _ = shared.out.send(&error_msg(&job, err));
    }
    if panicked {
        return Some(EXIT_PANIC);
    }
    if family.warm() {
        let _ = shared.out.send(&WorkerMsg::Ready {});
        None
    } else {
        Some(0)
    }
}

fn error_msg(job: &str, error: JobError) -> WorkerMsg {
    WorkerMsg::Error {
        job: job.to_owned(),
        error,
        item_errors: None,
    }
}

pub(crate) fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    let text = panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".to_owned());
    format!("the worker panicked: {text}")
}

/// `SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX)` and
/// `WerSetFlags(WER_FAULT_REPORTING_NO_UI)`: a crashing worker never shows a dialog.
#[cfg(windows)]
fn crash_ui_off() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };
    use windows_sys::Win32::System::ErrorReporting::{WER_FAULT_REPORTING_NO_UI, WerSetFlags};
    // SAFETY: both calls only set process-wide flags.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
        WerSetFlags(WER_FAULT_REPORTING_NO_UI);
    }
}

#[cfg(not(windows))]
fn crash_ui_off() {}
