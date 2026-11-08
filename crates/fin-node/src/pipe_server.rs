//! `PipeServer`: finisherd's native Windows named-pipe listener (sub-project 5A Task 11,
//! Decision 38, spec §4.3). The listener itself is [`crate::pipe_win`] (Windows only); this
//! is its napi face. Elsewhere the constructor fails with `PIPE_UNSUPPORTED`: Unix keeps its
//! socket (`transport.ts`).
//!
//! - Events (a connection that passed the identity check, a refused client) reach `onEvent`
//!   as JSON strings through a weak threadsafe function; those before it was set are kept.
//! - `read` resolves to the next chunk of a connection (at most 64 KiB) or `null` once it has
//!   ended; one read at a time per connection. `write` resolves once the bytes are in the
//!   pipe; a client that went away rejects with `ERROR_BROKEN_PIPE`.
//! - `close` is graceful (the client gets what was written first); `shutdown` closes
//!   everything and returns once the pipe's name is free.

use fin_core::CoreError;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;
use napi_derive::napi;
use serde_json::json;

use crate::{guarded, reject};

/// `new PipeServer(name, options)`.
#[napi(object)]
pub struct PipeOptions {
    /// SIDs allowed beside this user (sub-project 7: AppContainer SIDs).
    pub allow: Option<Vec<String>>,
    /// Client instances at most (the connections and the one waiting), 2..=253; default 64.
    pub max_connections: Option<u32>,
}

type EventFn = ThreadsafeFunction<String, (), String, Status, false, true>;

#[napi]
pub struct PipeServer {
    #[cfg(windows)]
    core: std::sync::Arc<crate::pipe_win::PipeCore>,
}

fn io_reject(e: &std::io::Error) -> Error {
    let code = match e.raw_os_error() {
        // ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED: the client went away.
        Some(109 | 232 | 233) => "ERROR_BROKEN_PIPE",
        _ if e.kind() == std::io::ErrorKind::BrokenPipe => "ERROR_BROKEN_PIPE",
        _ => "IO_ERROR",
    };
    reject(CoreError::new(code, format!("pipe I/O failed: {e}")))
}

#[napi]
impl PipeServer {
    /// Creates the pipe `name` (`\\.\pipe\…`) with its own descriptor and starts accepting.
    /// Throws `PIPE_SQUATTED` when the name exists already, `PIPE_UNSUPPORTED` off Windows.
    #[napi(constructor, catch_unwind)]
    pub fn new(name: String, options: Option<PipeOptions>) -> Result<Self> {
        guarded(|| imp::new(name, options), || {})
    }

    /// This user's SID, which the pipe's descriptor grants.
    #[napi(getter)]
    pub fn user_sid(&self) -> String {
        imp::user_sid(self)
    }

    /// Delivers every event (JSON: `{type: "connection", id, pid, sid, appContainer?}` or
    /// `{type: "refused", pid, sid?, appContainer?, reason}`) to `callback`. It does not keep
    /// the process alive.
    #[napi(catch_unwind)]
    pub fn on_event(
        &self,
        #[napi(ts_arg_type = "(json: string) => void")] callback: Function<String, ()>,
    ) -> Result<()> {
        let tsfn: EventFn = callback
            .build_threadsafe_function::<String>()
            .weak::<true>()
            .build()?;
        imp::on_event(self, tsfn);
        Ok(())
    }

    /// The next bytes of connection `id`, or `null` once it has ended.
    #[napi(catch_unwind, ts_return_type = "Promise<Buffer | null>")]
    pub fn read<'e>(&self, env: &'e Env, id: u32) -> Result<Object<'e>> {
        let (deferred, promise) = env.create_deferred::<Option<Buffer>, _>()?;
        imp::read(
            self,
            id,
            Box::new(move |r: std::io::Result<Option<Vec<u8>>>| match r {
                Ok(data) => deferred.resolve(move |_| Ok(data.map(Buffer::from))),
                Err(e) => deferred.reject(io_reject(&e)),
            }),
        );
        Ok(promise)
    }

    /// Writes `data` to connection `id`.
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn write<'e>(&self, env: &'e Env, id: u32, data: Buffer) -> Result<Object<'e>> {
        let (deferred, promise) = env.create_deferred::<(), _>()?;
        imp::write(
            self,
            id,
            data.to_vec(),
            Box::new(move |r: std::io::Result<()>| match r {
                Ok(()) => deferred.resolve(|_| Ok(())),
                Err(e) => deferred.reject(io_reject(&e)),
            }),
        );
        Ok(promise)
    }

    /// Closes connection `id` once its client has read what was written (at most 2 s).
    #[napi(catch_unwind)]
    pub fn close(&self, id: u32) {
        imp::close(self, id);
    }

    /// Allows `sid` too (sub-project 7: an AppContainer SID). Throws `INVALID_SID`.
    #[napi(catch_unwind)]
    pub fn allow(&self, sid: String) -> Result<()> {
        imp::allow(self, &sid)
    }

    /// Closes every connection and the pipe; returns once the name is free. Idempotent.
    #[napi(catch_unwind)]
    pub fn shutdown(&self) {
        imp::shutdown(self);
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::Arc;

    use napi::threadsafe_function::ThreadsafeFunctionCallMode;

    use super::*;
    use crate::pipe_win::{
        DEFAULT_MAX_CONNECTIONS, Event, Options, PipeCore, ReadDone, StartError, WriteDone,
    };

    pub fn new(name: String, options: Option<PipeOptions>) -> Result<PipeServer> {
        let options = options.unwrap_or(PipeOptions {
            allow: None,
            max_connections: None,
        });
        let opts = Options {
            allow: options.allow.unwrap_or_default(),
            max_connections: options.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS),
        };
        match PipeCore::new(&name, opts) {
            Ok(core) => Ok(PipeServer {
                core: Arc::new(core),
            }),
            Err(StartError::Squatted(e)) => Err(reject(
                CoreError::new(
                    "PIPE_SQUATTED",
                    format!("another process already holds the pipe {name}"),
                )
                .with_details(json!({ "pipe": name, "cause": e.to_string() })),
            )),
            Err(StartError::Other(e)) => Err(reject(
                CoreError::new(
                    "SERVER_START_FAILED",
                    format!("cannot create the pipe {name}: {e}"),
                )
                .with_details(json!({ "pipe": name })),
            )),
        }
    }

    pub fn user_sid(s: &PipeServer) -> String {
        s.core.user_sid().to_string()
    }

    pub fn on_event(s: &PipeServer, tsfn: EventFn) {
        s.core.set_handler(Box::new(move |e| {
            let value = match e {
                Event::Connected(p) => json!({
                    "type": "connection", "id": p.id, "pid": p.pid, "sid": p.sid,
                    "appContainer": p.app_container,
                }),
                Event::Refused(r) => json!({
                    "type": "refused", "pid": r.pid, "sid": r.sid,
                    "appContainer": r.app_container, "reason": r.reason,
                }),
            };
            tsfn.call(value.to_string(), ThreadsafeFunctionCallMode::NonBlocking);
        }));
    }

    pub fn read(s: &PipeServer, id: u32, done: ReadDone) {
        s.core.read(id, done);
    }

    pub fn write(s: &PipeServer, id: u32, data: Vec<u8>, done: WriteDone) {
        s.core.write(id, data, done);
    }

    pub fn close(s: &PipeServer, id: u32) {
        s.core.close(id);
    }

    pub fn allow(s: &PipeServer, sid: &str) -> Result<()> {
        s.core.allow(sid).map_err(|e| {
            let code = if e.kind() == std::io::ErrorKind::InvalidInput {
                "INVALID_SID"
            } else {
                "IO_ERROR"
            };
            reject(CoreError::new(code, e.to_string()))
        })
    }

    pub fn shutdown(s: &PipeServer) {
        s.core.shutdown();
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    type ReadDone = Box<dyn FnOnce(std::io::Result<Option<Vec<u8>>>) + Send>;
    type WriteDone = Box<dyn FnOnce(std::io::Result<()>) + Send>;

    pub fn new(name: String, _options: Option<PipeOptions>) -> Result<PipeServer> {
        Err(reject(
            CoreError::new("PIPE_UNSUPPORTED", "named pipes exist only on Windows")
                .with_details(json!({ "pipe": name })),
        ))
    }

    // A `PipeServer` cannot be made here, so the rest is never reached.
    pub fn user_sid(_: &PipeServer) -> String {
        String::new()
    }
    pub fn on_event(_: &PipeServer, _: EventFn) {}
    pub fn read(_: &PipeServer, _: u32, done: ReadDone) {
        done(Ok(None));
    }
    pub fn write(_: &PipeServer, _: u32, _: Vec<u8>, done: WriteDone) {
        done(Err(std::io::ErrorKind::BrokenPipe.into()));
    }
    pub fn close(_: &PipeServer, _: u32) {}
    pub fn allow(_: &PipeServer, _: &str) -> Result<()> {
        Ok(())
    }
    pub fn shutdown(_: &PipeServer) {}
}
