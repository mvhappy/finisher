//! The host's completion port (Decision 14), **advisory only**: Job messages are not
//! guaranteed to arrive, so exits come from process handles and the facts from the Jobs at
//! each exit. One thing the port speeds up: `JOB_OBJECT_MSG_JOB_MEMORY_LIMIT` ends the group
//! at once (marked `oom`), as a cgroup's `memory.oom.group` would, rather than leaving its
//! processes to fail allocations one by one. And `JOB_OBJECT_MSG_NEW_PROCESS` lets the group
//! recognise a console host while it is still alive (`conhost.rs`).

use std::collections::HashMap;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED, PostQueuedCompletionStatus,
};
use windows_sys::Win32::System::JobObjects::{
    JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JobObjectAssociateCompletionPortInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::INFINITE;

use super::job::GroupJob;
use super::{check, owned, raw};
use crate::LogFn;

/// `JOB_OBJECT_MSG_NEW_PROCESS` and `JOB_OBJECT_MSG_JOB_MEMORY_LIMIT` (winnt.h).
const MSG_NEW_PROCESS: u32 = 6;
const MSG_JOB_MEMORY_LIMIT: u32 = 10;
/// The key that stops the thread; group keys start at 1.
const STOP: usize = 0;

pub(crate) struct Port {
    port: OwnedHandle,
    groups: Mutex<HashMap<usize, Weak<GroupJob>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    log: LogFn,
}

impl Port {
    pub fn new(log: LogFn) -> io::Result<Arc<Port>> {
        // SAFETY: a new port, associated with no file.
        let port = owned(unsafe {
            CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1)
        })?;
        Ok(Arc::new(Port {
            port,
            groups: Mutex::default(),
            thread: Mutex::default(),
            log,
        }))
    }

    /// Starts the `jobhost-port` thread.
    pub fn start(self: &Arc<Port>) -> io::Result<()> {
        let p = self.clone();
        let t = std::thread::Builder::new()
            .name("jobhost-port".into())
            .spawn(move || p.run())?;
        *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
        Ok(())
    }

    /// Associates a group Job with the port under its key.
    pub fn register(&self, g: &Arc<GroupJob>) -> io::Result<()> {
        let info = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
            CompletionKey: g.key as *mut core::ffi::c_void,
            CompletionPort: raw(&self.port),
        };
        // SAFETY: `info` is the struct this class takes, with its size.
        check(unsafe {
            SetInformationJobObject(
                raw(&g.job),
                JobObjectAssociateCompletionPortInformation,
                (&raw const info).cast(),
                size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
            )
        })?;
        self.groups().insert(g.key, Arc::downgrade(g));
        Ok(())
    }

    pub fn unregister(&self, key: usize) {
        self.groups().remove(&key);
    }

    fn groups(&self) -> std::sync::MutexGuard<'_, HashMap<usize, Weak<GroupJob>>> {
        self.groups.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stops the thread (if it runs) and waits for it.
    pub fn stop(&self) {
        let t = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(t) = t {
            // SAFETY: posts one packet to our own port.
            unsafe { PostQueuedCompletionStatus(raw(&self.port), 0, STOP, std::ptr::null()) };
            let _ = t.join();
        }
    }

    fn run(&self) {
        loop {
            let (mut msg, mut key) = (0u32, 0usize);
            let mut ov: *mut OVERLAPPED = std::ptr::null_mut();
            // SAFETY: valid out-pointers; for Job messages `ov` carries a pid, not a pointer.
            let ok = unsafe {
                GetQueuedCompletionStatus(raw(&self.port), &mut msg, &mut key, &mut ov, INFINITE)
            };
            if ok == 0 {
                (self.log)(&format!(
                    "the job completion port failed: {}",
                    io::Error::last_os_error()
                ));
                return;
            }
            if key == STOP {
                return;
            }
            if msg != MSG_JOB_MEMORY_LIMIT && msg != MSG_NEW_PROCESS {
                continue;
            }
            let Some(g) = self.groups().get(&key).and_then(Weak::upgrade) else {
                continue;
            };
            if msg == MSG_JOB_MEMORY_LIMIT {
                g.over_memory();
            } else {
                // For Job messages the "overlapped" pointer is the process id.
                g.noted(ov as usize as u32);
            }
        }
    }
}
