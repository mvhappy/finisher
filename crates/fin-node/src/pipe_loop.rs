//! The loop of the Windows pipe listener ([`crate::pipe_win`], whose docs describe it): one
//! thread that owns every instance and waits on the I/O completion port.

use std::collections::HashMap;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{SE_KERNEL_OBJECT, SetSecurityInfo};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, LABEL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    SECURITY_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::IO::{
    CancelIoEx, CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

use crate::pipe_sec::{self, Descriptor, Identity};
use crate::pipe_win::{
    CLOSE_GRACE, Cmd, Event, KEY_CMD, KEY_PIPE, Peer, READ_CHUNK, ReadDone, Refused, Shared,
    StartError, WriteDone, lock,
};

#[path = "pipe_io.rs"]
mod conn_io;

use conn_io::{drained, os_error};

/// How often closing connections are checked.
const POLL: Duration = Duration::from_millis(5);
/// After an instance could not be created (not at the cap), the next try.
const RETRY: Duration = Duration::from_millis(100);

/// What the loop starts from.
pub(crate) struct Setup {
    pub name: Vec<u16>,
    /// Client instances at most: the connections and the one waiting (2..=253).
    pub max: u32,
    pub user: String,
    pub allow: Vec<String>,
    pub sd: Descriptor,
}

/// Creates the anchor (`FILE_FLAG_FIRST_PIPE_INSTANCE`, so a squatter fails the start) and the
/// first listening instance here, then runs the loop on its own thread.
pub(crate) fn start(shared: Arc<Shared>, setup: Setup) -> Result<JoinHandle<()>, StartError> {
    let mut lp = Loop {
        anchor: null_mut(),
        shared,
        name: setup.name,
        max: setup.max,
        user: setup.user,
        allow: setup.allow,
        sd: setup.sd,
        listener: None,
        instances: 0,
        conns: HashMap::new(),
        next_id: 1,
        outstanding: 0,
        stopping: false,
        want_listen: false,
        retry_at: None,
    };
    lp.anchor = lp.create(true).map_err(|e| {
        let code = e.raw_os_error().and_then(|c| u32::try_from(c).ok());
        if code == Some(ERROR_ACCESS_DENIED) || code == Some(ERROR_PIPE_BUSY) {
            StartError::Squatted(e)
        } else {
            StartError::Other(e)
        }
    })?;
    // Disconnected, the anchor takes no client (they find it busy) until it is closed.
    // SAFETY: a new instance of ours; a client that raced in is disconnected and retries.
    unsafe { DisconnectNamedPipe(lp.anchor) };
    let anchor = lp.anchor;
    let first = match lp.create(false) {
        Ok(h) => h,
        Err(e) => {
            // SAFETY: the anchor is ours and unused.
            unsafe { CloseHandle(anchor) };
            return Err(StartError::Other(e));
        }
    };
    let send = SendLoop(lp, first as usize);
    std::thread::Builder::new()
        .name("fin-pipe".into())
        .spawn(move || {
            // A method call, so the closure captures `send` whole (it is `Send`; its parts are not).
            let (mut lp, first) = send.into_parts();
            // The first accept is issued on the loop's thread, which outlives it.
            lp.accept(first as HANDLE);
            lp.run();
        })
        .map_err(|e| {
            // `spawn` dropped the closure, and with it the loop: close both instances.
            // SAFETY: the handles were never used.
            unsafe {
                CloseHandle(first);
                CloseHandle(anchor);
            }
            StartError::Other(e)
        })
}

/// The loop and its first instance, moved to the loop's thread.
struct SendLoop(Loop, usize);
// SAFETY: the handles inside are only used by the thread the loop moves to.
unsafe impl Send for SendLoop {}

impl SendLoop {
    fn into_parts(self) -> (Loop, usize) {
        (self.0, self.1)
    }
}

struct Conn {
    handle: HANDLE,
    /// The outstanding read, for `CancelIoEx`.
    read: Option<*mut OVERLAPPED>,
    writes: usize,
    /// Set by `close`: the latest moment to close the handle.
    closing: Option<Instant>,
}

/// One overlapped operation; `ov` first, so the completion's `OVERLAPPED` pointer is the `Op`.
#[repr(C)]
struct Op {
    ov: OVERLAPPED,
    conn: u32,
    kind: Kind,
    buf: Vec<u8>,
}

enum Kind {
    Accept(HANDLE),
    Read(Option<ReadDone>),
    Write(Option<WriteDone>),
}

impl Op {
    fn boxed(conn: u32, kind: Kind, buf: Vec<u8>) -> Box<Op> {
        Box::new(Op {
            // SAFETY: an all-zero OVERLAPPED is the documented initial state.
            ov: unsafe { zeroed() },
            conn,
            kind,
            buf,
        })
    }
}

struct Loop {
    /// The first instance, kept disconnected for the loop's life: it holds the name (never free
    /// to take, even while every other instance closes) and `WRITE_OWNER`, which only the first
    /// instance may ask for and `allow` needs to set the label.
    anchor: HANDLE,
    shared: Arc<Shared>,
    name: Vec<u16>,
    max: u32,
    user: String,
    allow: Vec<String>,
    sd: Descriptor,
    /// The instance waiting for a client.
    listener: Option<HANDLE>,
    /// The instances that take clients: the listener and the connections (not the anchor).
    instances: u32,
    conns: HashMap<u32, Conn>,
    next_id: u32,
    /// Operations whose completion packets have not arrived.
    outstanding: usize,
    stopping: bool,
    want_listen: bool,
    retry_at: Option<Instant>,
}

impl Loop {
    /// The anchor (`anchor`), or a new instance associated with the port.
    fn create(&mut self, anchor: bool) -> io::Result<HANDLE> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.sd.as_ptr(),
            bInheritHandle: 0,
        };
        let open = PIPE_ACCESS_DUPLEX
            | FILE_FLAG_OVERLAPPED
            | if anchor {
                FILE_FLAG_FIRST_PIPE_INSTANCE | WRITE_DAC | WRITE_OWNER
            } else {
                0
            };
        let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
        let chunk = READ_CHUNK as u32;
        // SAFETY: `name` is NUL-terminated; `sa` and its descriptor outlive the call.
        let h = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                open,
                mode,
                // The anchor is one more instance than the clients may take.
                self.max + 1,
                chunk,
                chunk,
                0,
                &sa,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        if anchor {
            return Ok(h);
        }
        // SAFETY: `h` is a new overlapped handle; the port is ours.
        if unsafe { CreateIoCompletionPort(h, self.shared.port as HANDLE, KEY_PIPE, 0) }.is_null() {
            let e = io::Error::last_os_error();
            // SAFETY: `h` is ours and unused.
            unsafe { CloseHandle(h) };
            return Err(e);
        }
        self.instances += 1;
        Ok(h)
    }

    /// Starts `f` on `op`: `Ok` when a completion packet will come, else the op and the error.
    fn issue(
        &mut self,
        op: Box<Op>,
        f: impl FnOnce(*mut Op) -> i32,
    ) -> Result<*mut Op, (Box<Op>, u32)> {
        let raw = Box::into_raw(op);
        let ok = f(raw);
        // SAFETY: read right after the call that set it.
        let err = if ok != 0 {
            0
        } else {
            unsafe { GetLastError() }
        };
        if ok != 0 || err == ERROR_IO_PENDING {
            self.outstanding += 1;
            Ok(raw)
        } else {
            // SAFETY: no packet will come, so the op is still only ours.
            Err((unsafe { Box::from_raw(raw) }, err))
        }
    }

    /// Waits for a client on instance `h`.
    fn accept(&mut self, h: HANDLE) {
        let op = Op::boxed(0, Kind::Accept(h), Vec::new());
        // SAFETY: `h` is a live instance; the op stays allocated until its packet.
        match self.issue(op, |op| unsafe { ConnectNamedPipe(h, &mut (*op).ov) }) {
            Ok(_) => self.listener = Some(h),
            Err((_, ERROR_PIPE_CONNECTED)) => self.accepted(h),
            Err((_, e)) => self.drop_instance(h, e != ERROR_NO_DATA),
        }
    }

    /// Closes an instance that holds no connection; `retry`: wait before the next one.
    fn drop_instance(&mut self, h: HANDLE, retry: bool) {
        // SAFETY: `h` is ours and holds no operation.
        unsafe { CloseHandle(h) };
        self.instances -= 1;
        self.want_listen = true;
        if retry {
            self.retry_at = Some(Instant::now() + RETRY);
        }
    }

    /// A client connected to `h`: checked, then handed over or disconnected.
    fn accepted(&mut self, h: HANDLE) {
        self.want_listen = true;
        let mut pid = 0u32;
        // SAFETY: `h` is a connected server end.
        let identity = if unsafe { GetNamedPipeClientProcessId(h, &mut pid) } == 0 {
            None
        } else {
            pipe_sec::process_identity(pid).ok()
        };
        let (user, app_container) = match identity {
            Some(i) if pipe_sec::peer_allowed(&self.user, &self.allow, &i) => {
                (i.user, i.app_container)
            }
            other => {
                // SAFETY: `h` is ours; nothing was read from it.
                unsafe { DisconnectNamedPipe(h) };
                self.drop_instance(h, false);
                let (sid, app_container) = match other {
                    Some(Identity {
                        user,
                        app_container,
                        ..
                    }) => (Some(user), app_container),
                    None => (None, None),
                };
                let reason = if sid.is_some() {
                    "peer"
                } else {
                    "unidentified"
                };
                self.shared.emit(Event::Refused(Refused {
                    pid,
                    sid,
                    app_container,
                    reason,
                }));
                return;
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        self.conns.insert(
            id,
            Conn {
                handle: h,
                read: None,
                writes: 0,
                closing: None,
            },
        );
        self.shared.emit(Event::Connected(Peer {
            id,
            pid,
            sid: user,
            app_container,
        }));
    }

    fn run(&mut self) {
        loop {
            self.tick();
            if self.stopping && self.outstanding == 0 {
                break;
            }
            let (mut bytes, mut key, mut ov) = (0u32, 0usize, null_mut::<OVERLAPPED>());
            let timeout = self.timeout_ms();
            // SAFETY: the port is ours; the out-pointers are locals.
            let ok = unsafe {
                GetQueuedCompletionStatus(
                    self.shared.port as HANDLE,
                    &mut bytes,
                    &mut key,
                    &mut ov,
                    timeout,
                )
            };
            // SAFETY: read right after the call.
            let err = if ok != 0 {
                0
            } else {
                unsafe { GetLastError() }
            };
            if !ov.is_null() {
                self.outstanding -= 1;
                // SAFETY: every OVERLAPPED queued on this port is the head of a leaked `Op`.
                let op = unsafe { Box::from_raw(ov.cast::<Op>()) };
                self.complete(*op, bytes, err);
            } else if ok != 0 && key == KEY_CMD {
                self.commands();
            } else if ok == 0 && err != WAIT_TIMEOUT {
                // The port itself failed: nothing more can complete.
                break;
            }
        }
        let rest = {
            let mut q = lock(&self.shared.queue);
            q.stopped = true;
            std::mem::take(&mut q.cmds)
        };
        rest.into_iter().for_each(Cmd::refuse);
        // SAFETY: every handle on the port is closed and no packet is outstanding.
        unsafe { CloseHandle(self.shared.port as HANDLE) };
    }

    fn timeout_ms(&self) -> u32 {
        if self.stopping {
            return 1000;
        }
        let mut next: Option<Instant> = self.retry_at;
        if self.conns.values().any(|c| c.closing.is_some()) {
            let poll = Instant::now() + POLL;
            next = Some(next.map_or(poll, |n| n.min(poll)));
        }
        match next {
            None => u32::MAX, // INFINITE
            Some(at) => {
                let ms = at.saturating_duration_since(Instant::now()).as_millis();
                u32::try_from(ms).unwrap_or(u32::MAX - 1)
            }
        }
    }

    /// Closes the closing connections that are done and keeps an instance waiting.
    fn tick(&mut self) {
        let now = Instant::now();
        let done: Vec<u32> = self
            .conns
            .iter()
            .filter(|(_, c)| {
                c.closing.is_some_and(|at| now >= at)
                    || (c.closing.is_some() && c.writes == 0 && drained(c.handle))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in done {
            if let Some(c) = self.conns.remove(&id) {
                // SAFETY: ours; its operations (cancelled) still deliver their packets.
                unsafe { CloseHandle(c.handle) };
                self.instances -= 1;
                self.want_listen = true;
            }
        }
        if self.retry_at.is_some_and(|at| now < at) {
            return;
        }
        self.retry_at = None;
        while self.want_listen
            && !self.stopping
            && self.listener.is_none()
            && self.instances < self.max
        {
            self.want_listen = false;
            match self.create(false) {
                Ok(h) => self.accept(h),
                Err(_) => {
                    self.want_listen = true;
                    self.retry_at = Some(now + RETRY);
                    break;
                }
            }
        }
    }

    fn commands(&mut self) {
        let cmds = std::mem::take(&mut lock(&self.shared.queue).cmds);
        for cmd in cmds {
            match cmd {
                Cmd::Read(id, done) => self.start_read(id, done),
                Cmd::Write(id, data, done) => self.start_write(id, data, done),
                Cmd::Close(id) => {
                    if let Some(c) = self.conns.get_mut(&id).filter(|c| c.closing.is_none()) {
                        c.closing = Some(Instant::now() + CLOSE_GRACE);
                        if let Some(ov) = c.read {
                            // SAFETY: the read's OVERLAPPED is live until its packet.
                            unsafe { CancelIoEx(c.handle, ov) };
                        }
                    }
                }
                Cmd::Allow(sid, reply) => {
                    let _ = reply.send(self.allow(&sid));
                }
                Cmd::Shutdown => self.shutdown(),
            }
        }
    }

    fn allow(&mut self, sid: &str) -> io::Result<()> {
        let sid = pipe_sec::canonical_sid(sid)?;
        if self.allow.contains(&sid) {
            return Ok(());
        }
        let mut allow = self.allow.clone();
        allow.push(sid);
        let sd = Descriptor::from_sddl(&pipe_sec::sddl(&self.user, &allow))?;
        // The pipe has one descriptor for all its instances: set it through the anchor. The
        // low label comes with the first allowed SID (`pipe_sec`).
        {
            let h = self.anchor;
            let dacl = sd.dacl()?;
            let sacl = sd.sacl();
            let what = DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION
                | if sacl.is_null() {
                    0
                } else {
                    LABEL_SECURITY_INFORMATION
                };
            // SAFETY: the anchor was opened with WRITE_DAC and WRITE_OWNER; the ACLs live in `sd`.
            let rc = unsafe {
                SetSecurityInfo(
                    h,
                    SE_KERNEL_OBJECT,
                    what,
                    null_mut(),
                    null_mut(),
                    dacl,
                    sacl,
                )
            };
            if rc != 0 {
                return Err(os_error(rc));
            }
        }
        self.allow = allow;
        self.sd = sd;
        Ok(())
    }

    fn shutdown(&mut self) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        lock(&self.shared.queue).stopped = true;
        // SAFETY: ours, with no operation; the name goes once the others are closed too.
        unsafe { CloseHandle(self.anchor) };
        if let Some(h) = self.listener.take() {
            // SAFETY: ours; the cancelled accept still delivers its packet.
            unsafe {
                CancelIoEx(h, null_mut());
                CloseHandle(h);
            }
            self.instances -= 1;
        }
        for (_, c) in self.conns.drain() {
            // SAFETY: as above.
            unsafe {
                CancelIoEx(c.handle, null_mut());
                DisconnectNamedPipe(c.handle);
                CloseHandle(c.handle);
            }
            self.instances -= 1;
        }
    }
}
