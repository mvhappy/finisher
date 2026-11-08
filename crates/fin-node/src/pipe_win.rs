//! The Windows named-pipe listener behind `PipeServer` (sub-project 5A Task 11, Decision 38,
//! spec §4.3). napi-free, so `tests/pipe_server.rs` drives it directly.
//!
//! - **Instances** are `PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED`, byte mode,
//!   `PIPE_REJECT_REMOTE_CLIENTS`, with the descriptor of [`crate::pipe_sec`]. The first one,
//!   the *anchor*, carries `FILE_FLAG_FIRST_PIPE_INSTANCE` and is created before `new`
//!   returns: if anyone holds the name already, the start fails ([`StartError::Squatted`]).
//!   The anchor is disconnected at once and kept so until shutdown: it takes no client, holds
//!   the name (never free to take, even while every other instance closes) and holds
//!   `WRITE_DAC | WRITE_OWNER`, which `allow` needs for the descriptor and its label and which
//!   no later instance may ask for. The DACL refuses other users `FILE_CREATE_PIPE_INSTANCE`.
//! - **One thread** owns every handle and waits on one I/O completion port: the accept
//!   (`ConnectNamedPipe`), each connection's read and its writes are overlapped operations on
//!   it, and the callers' requests reach it as posted packets ([`Cmd`]). At most
//!   `max_connections` instances exist; at the cap, clients wait (`ERROR_PIPE_BUSY`).
//! - **A client** is checked before anything is read from it: its process and token
//!   ([`pipe_sec::process_identity`]) must be this user or an allowed SID; otherwise it is
//!   disconnected and reported as [`Event::Refused`].
//! - **Reads** are pulled: one outstanding read of at most [`READ_CHUNK`] bytes per connection,
//!   issued when the caller asks, so a connection buffers no more than that here.
//! - **Close** is graceful: the read is cancelled, the writes already issued complete, and the
//!   handle is closed once the client has read what was written (polled through
//!   `FilePipeLocalInformation`), the client has gone, or [`CLOSE_GRACE`] has passed. A server
//!   that closes a pipe handle with unread data loses it, so the "answer, then close" of a
//!   revocation (`Link.terminate`) needs this. `shutdown` closes everything at once.

use std::collections::VecDeque;
use std::io;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::IO::{CreateIoCompletionPort, PostQueuedCompletionStatus};

use crate::pipe_loop;
use crate::pipe_sec::{self, Descriptor};

/// The most one read hands over (and the pipe's buffer sizes).
pub const READ_CHUNK: usize = 64 * 1024;
/// How long a closing connection may take for its client to read what was written.
pub const CLOSE_GRACE: Duration = Duration::from_secs(2);
/// The default cap on instances (connections plus the waiting one).
pub const DEFAULT_MAX_CONNECTIONS: u32 = 64;

pub(crate) const KEY_PIPE: usize = 1;
pub(crate) const KEY_CMD: usize = 2;

pub struct Options {
    /// SIDs allowed beside this user (sub-project 7: AppContainer SIDs).
    pub allow: Vec<String>,
    /// Client instances at most: the connections and the one waiting, 2..=253 (clamped).
    pub max_connections: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            allow: Vec::new(),
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }
}

/// A connected client that passed the identity check.
#[derive(Clone, Debug)]
pub struct Peer {
    pub id: u32,
    pub pid: u32,
    pub sid: String,
    pub app_container: Option<String>,
}

/// A client that was disconnected before its first byte was read.
#[derive(Clone, Debug)]
pub struct Refused {
    pub pid: u32,
    pub sid: Option<String>,
    pub app_container: Option<String>,
    /// `peer` (neither this user nor an allowed SID) or `unidentified` (no pid or token).
    pub reason: &'static str,
}

#[derive(Clone, Debug)]
pub enum Event {
    Connected(Peer),
    Refused(Refused),
}

#[derive(Debug)]
pub enum StartError {
    /// The name was taken before this server created its first instance.
    Squatted(io::Error),
    Other(io::Error),
}

pub type ReadDone = Box<dyn FnOnce(io::Result<Option<Vec<u8>>>) + Send>;
pub type WriteDone = Box<dyn FnOnce(io::Result<()>) + Send>;
pub type Handler = Box<dyn FnMut(Event) + Send>;

pub(crate) enum Cmd {
    Read(u32, ReadDone),
    Write(u32, Vec<u8>, WriteDone),
    Close(u32),
    Allow(String, mpsc::Sender<io::Result<()>>),
    Shutdown,
}

impl Cmd {
    /// Answers a command that arrived after the loop stopped.
    pub(crate) fn refuse(self) {
        match self {
            Cmd::Read(_, done) => done(Ok(None)),
            Cmd::Write(_, _, done) => done(Err(io::ErrorKind::BrokenPipe.into())),
            Cmd::Allow(_, reply) => {
                let _ = reply.send(Err(io::Error::other("the pipe server has stopped")));
            }
            Cmd::Close(_) | Cmd::Shutdown => {}
        }
    }
}

#[derive(Default)]
pub(crate) struct Queue {
    pub cmds: VecDeque<Cmd>,
    pub stopped: bool,
}

pub(crate) struct Shared {
    pub port: usize,
    pub queue: Mutex<Queue>,
    /// The handler, or the events that arrived before one was set.
    pub handler: Mutex<(Option<Handler>, Vec<Event>)>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub(crate) fn post(&self, cmd: Cmd) {
        let mut q = lock(&self.queue);
        if q.stopped {
            drop(q);
            return cmd.refuse();
        }
        q.cmds.push_back(cmd);
        // Posted under the lock: the loop marks itself stopped under it before closing the port.
        // SAFETY: the port lives until the loop has stopped, which it has not (checked above).
        unsafe { PostQueuedCompletionStatus(self.port as HANDLE, 0, KEY_CMD, null_mut()) };
    }

    pub(crate) fn emit(&self, event: Event) {
        let mut h = lock(&self.handler);
        match &mut h.0 {
            Some(f) => f(event),
            None => h.1.push(event),
        }
    }
}

/// The listener (module docs). Dropping it shuts it down.
pub struct PipeCore {
    shared: Arc<Shared>,
    user: String,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl PipeCore {
    /// Creates the first instance of `name` (`\\.\pipe\…`) and starts accepting.
    pub fn new(name: &str, options: Options) -> Result<PipeCore, StartError> {
        let me = pipe_sec::current_identity().map_err(StartError::Other)?;
        let mut allow = Vec::new();
        for sid in &options.allow {
            allow.push(pipe_sec::canonical_sid(sid).map_err(StartError::Other)?);
        }
        let sd =
            Descriptor::from_sddl(&pipe_sec::sddl(&me.user, &allow)).map_err(StartError::Other)?;
        // SAFETY: a new port, closed by the loop when it ends.
        let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, null_mut(), 0, 1) };
        if port.is_null() {
            return Err(StartError::Other(io::Error::last_os_error()));
        }
        let shared = Arc::new(Shared {
            port: port as usize,
            queue: Mutex::new(Queue::default()),
            handler: Mutex::new((None, Vec::new())),
        });
        let thread = pipe_loop::start(
            shared.clone(),
            pipe_loop::Setup {
                name: pipe_sec::wide(name),
                max: options.max_connections.clamp(2, 253),
                user: me.user.clone(),
                allow,
                sd,
            },
        )
        .inspect_err(|_| {
            // SAFETY: the loop never started, so nothing else has the port.
            unsafe { CloseHandle(port) };
        })?;
        Ok(PipeCore {
            shared,
            user: me.user,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// This user's SID (the one the descriptor grants).
    pub fn user_sid(&self) -> &str {
        &self.user
    }

    /// Sets the event handler; events that came before it are delivered first, in order.
    pub fn set_handler(&self, mut handler: Handler) {
        let mut h = lock(&self.shared.handler);
        for e in std::mem::take(&mut h.1) {
            handler(e);
        }
        h.0 = Some(handler);
    }

    /// The next bytes of connection `id`, or `None` once it has ended (or is unknown).
    pub fn read(&self, id: u32, done: ReadDone) {
        self.shared.post(Cmd::Read(id, done));
    }

    pub fn write(&self, id: u32, data: Vec<u8>, done: WriteDone) {
        self.shared.post(Cmd::Write(id, data, done));
    }

    /// Closes connection `id` gracefully (module docs).
    pub fn close(&self, id: u32) {
        self.shared.post(Cmd::Close(id));
    }

    /// Allows `sid` too, for the instances to come and the pipe's descriptor now.
    pub fn allow(&self, sid: &str) -> io::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.shared.post(Cmd::Allow(sid.to_string(), tx));
        rx.recv()
            .unwrap_or_else(|_| Err(io::Error::other("the pipe server has stopped")))
    }

    /// Closes every connection and the waiting instance; returns once the name is gone.
    pub fn shutdown(&self) {
        self.shared.post(Cmd::Shutdown);
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
    }
}

impl Drop for PipeCore {
    fn drop(&mut self) {
        self.shutdown();
    }
}
