//! The per-connection I/O of the pipe listener's loop ([`super`]): reads, writes and their
//! completions, and the check that a closing connection's client has read everything.

use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_PIPE_CONNECTED_STATE, FILE_PIPE_LOCAL_INFORMATION, FilePipeLocalInformation,
    NtQueryInformationFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, HANDLE,
};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

use super::{Kind, Loop, Op};
use crate::pipe_win::{READ_CHUNK, ReadDone, WriteDone};

pub(super) fn os_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX))
}

/// The end of a client's stream: it closed, or the connection is being closed.
fn is_end(code: u32) -> bool {
    matches!(
        code,
        ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA | ERROR_OPERATION_ABORTED
    )
}

impl Loop {
    pub(super) fn complete(&mut self, mut op: Op, bytes: u32, err: u32) {
        match std::mem::replace(&mut op.kind, Kind::Read(None)) {
            Kind::Accept(h) => {
                if self.listener == Some(h) {
                    self.listener = None;
                    if err == 0 || err == ERROR_PIPE_CONNECTED {
                        self.accepted(h);
                    } else {
                        self.drop_instance(h, err != ERROR_NO_DATA);
                    }
                }
            }
            Kind::Read(done) => {
                let done = done.unwrap_or_else(|| Box::new(|_| {}));
                let Some(c) = self.conns.get_mut(&op.conn) else {
                    return done(Ok(None));
                };
                c.read = None;
                if err == 0 && bytes == 0 && c.closing.is_none() {
                    // A zero-byte write from the client: not the end; read again.
                    return self.start_read(op.conn, done);
                }
                if c.closing.is_some() || (err != 0 && is_end(err)) {
                    return done(Ok(None));
                }
                if err != 0 {
                    return done(Err(os_error(err)));
                }
                op.buf.truncate(bytes as usize);
                done(Ok(Some(std::mem::take(&mut op.buf))));
            }
            Kind::Write(done) => {
                if let Some(c) = self.conns.get_mut(&op.conn) {
                    c.writes -= 1;
                }
                if let Some(done) = done {
                    done(if err == 0 { Ok(()) } else { Err(os_error(err)) });
                }
            }
        }
    }

    pub(super) fn start_read(&mut self, id: u32, done: ReadDone) {
        let Some(c) = self.conns.get(&id) else {
            return done(Ok(None));
        };
        if c.closing.is_some() {
            return done(Ok(None));
        }
        if c.read.is_some() {
            return done(Err(io::Error::other("a read is already pending")));
        }
        let h = c.handle;
        let op = Op::boxed(id, Kind::Read(Some(done)), vec![0u8; READ_CHUNK]);
        // SAFETY: the buffer and the op stay allocated until the packet arrives.
        let issued = self.issue(op, |op| unsafe {
            let op = &mut *op;
            ReadFile(
                h,
                op.buf.as_mut_ptr(),
                READ_CHUNK as u32,
                null_mut(),
                &mut op.ov,
            )
        });
        match issued {
            Ok(raw) => {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.read = Some(raw.cast());
                }
            }
            Err((mut op, e)) => {
                if let Kind::Read(Some(done)) = std::mem::replace(&mut op.kind, Kind::Read(None)) {
                    done(if is_end(e) {
                        Ok(None)
                    } else {
                        Err(os_error(e))
                    });
                }
            }
        }
    }

    pub(super) fn start_write(&mut self, id: u32, data: Vec<u8>, done: WriteDone) {
        let Some(c) = self.conns.get(&id).filter(|c| c.closing.is_none()) else {
            return done(Err(io::ErrorKind::BrokenPipe.into()));
        };
        let Ok(len) = u32::try_from(data.len()) else {
            return done(Err(io::ErrorKind::InvalidInput.into()));
        };
        let h = c.handle;
        let op = Op::boxed(id, Kind::Write(Some(done)), data);
        // SAFETY: the buffer and the op stay allocated until the packet arrives.
        let issued = self.issue(op, |op| unsafe {
            let op = &mut *op;
            WriteFile(h, op.buf.as_ptr(), len, null_mut(), &mut op.ov)
        });
        match issued {
            Ok(_) => {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.writes += 1;
                }
            }
            Err((mut op, e)) => {
                if let Kind::Write(Some(done)) = std::mem::replace(&mut op.kind, Kind::Read(None)) {
                    done(Err(os_error(e)));
                }
            }
        }
    }
}

/// Whether the client has read everything written to `h`, or is gone.
pub(super) fn drained(h: HANDLE) -> bool {
    let mut io: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut info = FILE_PIPE_LOCAL_INFORMATION::default();
    // SAFETY: `h` is a live pipe handle; `info` is the class's structure.
    let status = unsafe {
        NtQueryInformationFile(
            h,
            &mut io,
            (&mut info as *mut FILE_PIPE_LOCAL_INFORMATION).cast(),
            size_of::<FILE_PIPE_LOCAL_INFORMATION>() as u32,
            FilePipeLocalInformation,
        )
    };
    status < 0
        || info.NamedPipeState != FILE_PIPE_CONNECTED_STATE
        || info.WriteQuotaAvailable >= info.OutboundQuota
}
