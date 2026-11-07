//! The helper runtime (Decision 8): a helper's own channel to the host. It mirrors the worker
//! runtime: the self-sandbox entry first, crash UI off, `hello {role: helper, kind, id}`; it
//! waits for `attach`, runs the helper, and exits 0 at `shutdown` (which also sets the cancel
//! token of a `run` still going). stdin EOF exits 70 at once. A helper's `run` that fails is
//! answered `error` and the process exits 1; a panic exits 101.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc;

use serde_json::Value;

use crate::error::JobError;
use crate::msg_host::{Attach, HelperKind};
use crate::msg_worker::{Hello, Role, WorkerMsg};
use crate::worker::{
    CancelToken, Channel, EXIT_PANIC, Inbound, Shared, enter, panic_text, spawn_alive, spawn_reader,
};
use crate::{EXIT_HOST_GONE, EXIT_HOST_PROTOCOL, MINOR, PROTOCOL};

/// A helper program: a codec backend or a plugin process beside a job's worker.
pub trait Helper: Send {
    fn kind(&self) -> HelperKind;
    fn id(&self) -> &str;
    /// Serves the attached job. Returning `Ok` waits for `shutdown`.
    fn run(&mut self, attach: &Attach, ctx: &mut HelperCtx<'_>) -> Result<(), JobError>;
}

/// What a helper's `run` gets.
pub struct HelperCtx<'a> {
    shared: &'a Arc<Shared>,
    token: CancelToken,
}

impl HelperCtx<'_> {
    /// A host call; blocks this thread until the reply.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, JobError> {
        self.shared.calls.call(&self.shared.out, method, params)
    }

    /// Set by `shutdown` (or the host's loss).
    pub fn cancel_token(&self) -> CancelToken {
        self.token.clone()
    }
}

#[cfg(feature = "test-hooks")]
impl HelperCtx<'_> {
    /// Stops `alive` (the `hang` test helper).
    pub fn stop_alive_for_test(&self) {
        self.shared.pause_alive();
    }

    /// Stops reading stdin for the rest of the process: the host's writes fill the pipe and
    /// EOF no longer exits (the `hang` test helper).
    pub fn stop_reading_for_test(&self) {
        self.shared.stop_reading();
    }
}

/// Runs `helper` over stdin/stdout and exits the process.
pub fn run(helper: impl Helper) -> ! {
    std::process::exit(run_on(helper, Channel::stdio()))
}

/// Runs `helper` over `chan` and returns the exit code.
pub fn run_on(mut helper: impl Helper, chan: Channel) -> i32 {
    let profile = match enter(&chan) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let shared = Shared::new(chan.output, chan.hard_exit, true);
    let hello = Hello {
        v: PROTOCOL,
        minor: MINOR,
        role: Role::Helper,
        family: String::new(),
        kinds: Vec::new(),
        warm: false,
        envelope: Some(crate::envelope_id().to_owned()),
        self_sandbox: profile,
        pid: chan.pid,
        kind: Some(helper.kind()),
        id: Some(helper.id().to_owned()),
    };
    let _ = shared.out.send(&WorkerMsg::Hello(hello));
    let (tx, rx) = mpsc::channel();
    spawn_reader(chan.input, &shared, tx);
    spawn_alive(&shared);
    let mut attached = false;
    let code = loop {
        match rx.recv() {
            Ok(Inbound::Attach(a, token)) if !attached => {
                attached = true;
                if let Some(code) = run_helper(&mut helper, &a, token, &shared) {
                    break code;
                }
            }
            Ok(Inbound::Attach(..)) => eprintln!("fin-jobs: a second `attach`; ignored"),
            Ok(Inbound::Start(..)) => eprintln!("fin-jobs: a helper got `start`; ignored"),
            Ok(Inbound::Shutdown) => break 0,
            Ok(Inbound::Protocol) => break EXIT_HOST_PROTOCOL,
            Ok(Inbound::HostGone) | Err(_) => break EXIT_HOST_GONE,
        }
    };
    shared.stop_alive();
    code
}

fn run_helper(
    helper: &mut impl Helper,
    attach: &Attach,
    token: CancelToken,
    shared: &Arc<Shared>,
) -> Option<i32> {
    let outcome = {
        let mut ctx = HelperCtx { shared, token };
        catch_unwind(AssertUnwindSafe(|| helper.run(attach, &mut ctx)))
    };
    let (error, code) = match outcome {
        Ok(Ok(())) => return None,
        Ok(Err(e)) => (e, 1),
        Err(panic) => (JobError::crashed(panic_text(&*panic)), EXIT_PANIC),
    };
    let _ = shared.out.send(&WorkerMsg::Error {
        job: attach.job.clone(),
        error,
        item_errors: None,
    });
    Some(code)
}
