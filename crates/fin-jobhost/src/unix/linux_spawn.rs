//! Linux: `std::process::Command` with `pre_exec` (Decision 12). Everything the child needs is
//! prepared before the fork; the child-side steps allocate nothing and take no lock, and each
//! is an async-signal-safe syscall retried on `EINTR`. In order: `setpgid`,
//! `PR_SET_PDEATHSIG`, the `getppid` check, the domain's writes (a leaf `cgroup.procs`),
//! `setpriority`, the request's `RLIMIT_NOFILE`, `close_range(3, ~0, CLOEXEC)` (or `FD_CLOEXEC` up to `RLIMIT_NOFILE`), then
//! the spawner's steps. Nothing of the worker runs before it is in its group.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use super::{SpawnCtx, Spawned, nice_of, registered};
use crate::domain::Placement;
use crate::spawner::{PlatformCommand, PreExec};

/// The child-side steps and what they need, all prepared in the parent.
struct ChildSteps {
    pgid: libc::pid_t,
    host_pid: libc::pid_t,
    nice: Option<libc::c_int>,
    /// The highest fd to mark close-on-exec when `close_range` is missing.
    nofile: libc::c_int,
    /// The request's `RLIMIT_NOFILE` (soft and hard, never above the hard limit; sp8b
    /// Decision 9).
    open_files: Option<libc::rlim_t>,
    writes: Vec<(OwnedFd, &'static [u8])>,
    extra: Vec<PreExec>,
}

fn check(r: libc::c_int) -> io::Result<()> {
    if r == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl ChildSteps {
    /// Runs in the child between fork and exec: no allocation, no lock.
    fn run(&self) -> io::Result<()> {
        // SAFETY: each call is async-signal-safe and touches only this process.
        unsafe {
            check(libc::setpgid(0, self.pgid))?;
            check(libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL))?;
            if libc::getppid() != self.host_pid {
                // The host died between the fork and here.
                libc::_exit(fin_jobs::EXIT_HOST_GONE);
            }
            for (fd, bytes) in &self.writes {
                let mut done = 0;
                while done < bytes.len() {
                    let r = libc::write(
                        fd.as_raw_fd(),
                        bytes[done..].as_ptr().cast(),
                        bytes.len() - done,
                    );
                    if r < 0 {
                        let e = io::Error::last_os_error();
                        if e.kind() != io::ErrorKind::Interrupted {
                            return Err(e);
                        }
                        continue;
                    }
                    done += r as usize;
                }
            }
            if let Some(n) = self.nice {
                // Best effort: a host already niced further may not lower it.
                libc::setpriority(libc::PRIO_PROCESS, 0, n);
            }
            if let Some(n) = self.open_files {
                let mut r: libc::rlimit = std::mem::zeroed();
                check(libc::getrlimit(libc::RLIMIT_NOFILE, &mut r))?;
                let n = n.min(r.rlim_max);
                let r = libc::rlimit {
                    rlim_cur: n,
                    rlim_max: n,
                };
                check(libc::setrlimit(libc::RLIMIT_NOFILE, &r))?;
            }
        }
        cloexec_from_3(self.nofile);
        for step in &self.extra {
            step()?;
        }
        Ok(())
    }
}

/// Marks every descriptor from 3 close-on-exec: `close_range(3, ~0, CLOEXEC)`, or
/// `FD_CLOEXEC` up to `nofile` where it is missing. Async-signal-safe; allocates nothing.
pub(super) fn cloexec_from_3(nofile: libc::c_int) {
    // SAFETY: plain syscalls on this process's descriptor table.
    unsafe {
        let r = libc::syscall(
            libc::SYS_close_range,
            3 as libc::c_uint,
            libc::c_uint::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        );
        if r != 0 {
            for fd in 3..nofile {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
    }
}

/// The soft `RLIMIT_NOFILE`, capped, for the `close_range` fallback.
pub(super) fn nofile() -> libc::c_int {
    // SAFETY: a zeroed rlimit is a valid out-buffer.
    let mut lim: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: getrlimit writes one rlimit.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        return 4096;
    }
    lim.rlim_cur.min(1 << 20) as libc::c_int
}

pub(super) fn spawn(cmd: PlatformCommand, ctx: &SpawnCtx<'_>) -> io::Result<Spawned> {
    let mut place = Placement {
        pgid: ctx.group.pgid(),
        child_writes: Vec::new(),
    };
    ctx.domain.place(ctx.group, ctx.leaf, &mut place)?;
    let (stdin_r, stdin_w) = io::pipe()?;
    let (stdout_r, stdout_w) = io::pipe()?;
    let (stderr_r, stderr_w) = io::pipe()?;
    let steps = ChildSteps {
        pgid: place.pgid,
        host_pid: ctx.host_pid as libc::pid_t,
        nice: nice_of(ctx.qos),
        nofile: nofile(),
        open_files: cmd.job_limits.open_files.map(libc::rlim_t::from),
        writes: place.child_writes,
        extra: cmd.pre_exec,
    };
    let mut c = Command::new(&cmd.program);
    c.args(&cmd.args)
        .env_clear()
        .envs(cmd.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&cmd.cwd)
        .stdin(Stdio::from(stdin_r))
        .stdout(Stdio::from(stdout_w))
        .stderr(Stdio::from(stderr_w));
    // SAFETY: `ChildSteps::run` allocates nothing and calls only async-signal-safe functions
    // (the spawner's steps promise the same, Decision 12).
    unsafe { c.pre_exec(move || steps.run()) };
    // std waits for the exec (or its error) on a close-on-exec pipe, retrying EINTR.
    let child = c.spawn()?;
    // Our copies of the child's ends close here, so EOF reaches both sides.
    drop(c);
    let pid = child.id();
    // `Child` neither kills nor reaps on drop; the waiter thread reaps by pid.
    drop(child);
    registered(ctx, pid)?;
    Ok(Spawned {
        pid,
        stdin: stdin_w,
        stdout: stdout_r,
        stderr: stderr_r,
        os: (),
    })
}
