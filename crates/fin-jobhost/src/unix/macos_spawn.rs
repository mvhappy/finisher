//! macOS: `posix_spawn` (Decision 13) with `POSIX_SPAWN_SETPGROUP`,
//! `POSIX_SPAWN_CLOEXEC_DEFAULT` (only the three dup'ed descriptors survive),
//! `POSIX_SPAWN_START_SUSPENDED`, default signal dispositions, an empty signal mask and the QoS
//! class. The process is placed and registered while suspended, then resumed with `SIGCONT`
//! (Task 3 Step 0 proves that `SIGCONT` resumes it). A failure before the resume kills the
//! suspended process.

use std::ffi::{CString, OsStr};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::ptr;

use super::{SpawnCtx, Spawned, reap, registered};
use crate::domain::Placement;
use crate::spawner::{PlatformCommand, Qos};

const QOS_CLASS_UTILITY: libc::c_uint = 0x11;
const QOS_CLASS_BACKGROUND: libc::c_uint = 0x09;

unsafe extern "C" {
    // <spawn.h> (libSystem): not in the libc crate.
    fn posix_spawnattr_set_qos_class_np(
        attr: *mut libc::posix_spawnattr_t,
        qos_class: libc::c_uint,
    ) -> libc::c_int;
}

fn cstr(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL"))
}

fn check(r: libc::c_int) -> io::Result<()> {
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(r))
    }
}

/// `posix_spawnattr_t` and `posix_spawn_file_actions_t`, destroyed on drop.
struct Attrs {
    attr: libc::posix_spawnattr_t,
    actions: libc::posix_spawn_file_actions_t,
}

impl Attrs {
    fn new() -> io::Result<Attrs> {
        // SAFETY: zeroed handles are only used after their *_init succeeded.
        let mut a: Attrs = unsafe { std::mem::zeroed() };
        // SAFETY: initialises locally owned objects; on failure the first is destroyed.
        unsafe {
            if let Err(e) = check(libc::posix_spawnattr_init(&mut a.attr)) {
                std::mem::forget(a);
                return Err(e);
            }
            if let Err(e) = check(libc::posix_spawn_file_actions_init(&mut a.actions)) {
                libc::posix_spawnattr_destroy(&mut a.attr);
                std::mem::forget(a);
                return Err(e);
            }
        }
        Ok(a)
    }
}

impl Drop for Attrs {
    fn drop(&mut self) {
        // SAFETY: both were initialised in `spawn`.
        unsafe {
            libc::posix_spawnattr_destroy(&mut self.attr);
            libc::posix_spawn_file_actions_destroy(&mut self.actions);
        }
    }
}

pub(super) fn spawn(cmd: PlatformCommand, ctx: &SpawnCtx<'_>) -> io::Result<Spawned> {
    let path = cstr(cmd.program.as_os_str())?;
    let cwd = cstr(cmd.cwd.as_os_str())?;
    let mut argv_c = vec![path.clone()];
    for a in &cmd.args {
        argv_c.push(cstr(a)?);
    }
    let mut envp_c = Vec::with_capacity(cmd.env.len());
    for (k, v) in &cmd.env {
        let mut kv = k.as_bytes().to_vec();
        kv.push(b'=');
        kv.extend_from_slice(v.as_bytes());
        envp_c.push(CString::new(kv).map_err(|_| io::Error::other("NUL in the environment"))?);
    }
    let mut argv: Vec<*mut libc::c_char> = argv_c.iter().map(|c| c.as_ptr() as _).collect();
    argv.push(ptr::null_mut());
    let mut envp: Vec<*mut libc::c_char> = envp_c.iter().map(|c| c.as_ptr() as _).collect();
    envp.push(ptr::null_mut());

    let (stdin_r, stdin_w) = io::pipe()?;
    let (stdout_r, stdout_w) = io::pipe()?;
    let (stderr_r, stderr_w) = io::pipe()?;
    let pgid = ctx.group.pgid();

    let mut a = Attrs::new()?;
    let mut pid: libc::pid_t = 0;
    // SAFETY: plain libc calls on locally owned objects; every pointer outlives the call.
    unsafe {
        let flags = libc::POSIX_SPAWN_SETPGROUP
            | libc::POSIX_SPAWN_CLOEXEC_DEFAULT
            | libc::POSIX_SPAWN_START_SUSPENDED
            | libc::POSIX_SPAWN_SETSIGDEF
            | libc::POSIX_SPAWN_SETSIGMASK;
        check(libc::posix_spawnattr_setflags(&mut a.attr, flags as _))?;
        check(libc::posix_spawnattr_setpgroup(&mut a.attr, pgid))?;
        let mut all: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut all);
        check(libc::posix_spawnattr_setsigdefault(&mut a.attr, &all))?;
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        check(libc::posix_spawnattr_setsigmask(&mut a.attr, &none))?;
        let qos = match ctx.qos {
            Qos::Normal => None,
            Qos::Utility => Some(QOS_CLASS_UTILITY),
            Qos::Background => Some(QOS_CLASS_BACKGROUND),
        };
        if let Some(q) = qos {
            check(posix_spawnattr_set_qos_class_np(&mut a.attr, q))?;
        }
        for (fd, to) in [
            (stdin_r.as_raw_fd(), 0),
            (stdout_w.as_raw_fd(), 1),
            (stderr_w.as_raw_fd(), 2),
        ] {
            check(libc::posix_spawn_file_actions_adddup2(
                &mut a.actions,
                fd,
                to,
            ))?;
        }
        check(libc::posix_spawn_file_actions_addchdir_np(
            &mut a.actions,
            cwd.as_ptr(),
        ))?;
        loop {
            let r = libc::posix_spawn(
                &mut pid,
                path.as_ptr(),
                &a.actions,
                &a.attr,
                argv.as_ptr(),
                envp.as_ptr(),
            );
            if r != libc::EINTR {
                check(r)?;
                break;
            }
        }
    }
    drop((stdin_r, stdout_w, stderr_w));
    let pid = pid as u32;
    let mut place = Placement { pgid, pid };
    if let Err(e) = ctx.domain.place(ctx.group, ctx.leaf, &mut place) {
        // SAFETY: our suspended child, not yet reaped.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        reap(pid);
        return Err(e);
    }
    registered(ctx, pid)?;
    #[cfg(feature = "test-hooks")]
    if let Some(d) = ctx.resume_delay {
        std::thread::sleep(d);
    }
    // SAFETY: resumes our own suspended child.
    if unsafe { libc::kill(pid as i32, libc::SIGCONT) } != 0 {
        let e = io::Error::last_os_error();
        // SAFETY: as above.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        reap(pid);
        return Err(e);
    }
    Ok(Spawned {
        pid,
        stdin: stdin_w,
        stdout: stdout_r,
        stderr: stderr_r,
        os: (),
    })
}
