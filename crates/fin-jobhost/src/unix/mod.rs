//! Unix spawning and reaping. Linux forks with `pre_exec` (Decision 12); macOS uses
//! `posix_spawn`, suspended until its registration (Decision 13). Every blocking call retries
//! `EINTR`. Exits are observed with `waitpid(pid)` (never `-1`), and a group leader with
//! `waitid(… WNOWAIT)`, reaped only when its group closes (Decision 17).

use std::io::{self, PipeReader, PipeWriter};

use crate::domain::{GroupHandle, KillDomain, LeafHandle};
use crate::exit::RawExit;
use crate::spawner::{PlatformCommand, Qos};
use crate::{HostError, HostOptions, KillTree};

#[cfg(target_os = "linux")]
#[path = "../linux/cgroup.rs"]
pub(crate) mod cgroup;
pub(crate) mod foreign;
#[cfg(target_os = "linux")]
mod linux_spawn;
#[cfg(target_os = "macos")]
mod macos_spawn;
pub(crate) mod memory;
pub(crate) mod pgroup;
pub(crate) mod watchdog;

/// The OS's part of a process (nothing on Unix: the pid is enough).
pub(crate) type OsProc = ();

/// A started process and the host's ends of its pipes.
pub(crate) struct Spawned {
    pub pid: u32,
    pub stdin: PipeWriter,
    pub stdout: PipeReader,
    pub stderr: PipeReader,
    pub os: OsProc,
}

/// What a spawn needs besides the command.
pub(crate) struct SpawnCtx<'a> {
    pub domain: &'a dyn KillDomain,
    pub group: &'a GroupHandle,
    pub leaf: &'a LeafHandle,
    pub qos: Qos,
    /// Linux: the child exits 70 if its parent is no longer this pid.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub host_pid: u32,
    #[cfg(feature = "test-hooks")]
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub resume_delay: Option<std::time::Duration>,
}

/// The kill domain for these options: delegated cgroups where Decision 16's conditions hold
/// (Linux, unless `cgroups` is `Off` or `FIN_CGROUPS=off`), else process groups; with the
/// watchdog when `opts.watchdog` (started after the host moved into its cgroup leaf).
pub(crate) fn default_domain(opts: &HostOptions) -> Result<Box<dyn KillDomain>, HostError> {
    #[cfg(target_os = "linux")]
    if let Some(root) = cgroup_root(opts) {
        match cgroup::Cgroups::new(root, opts.log.clone()) {
            Ok(cg) => return Ok(Box::new(cg.with_watchdog(start_watchdog(opts)?))),
            Err(e) => (opts.log)(&format!("cgroup setup failed ({e}); using process groups")),
        }
    }
    Ok(Box::new(pgroup::ProcessGroups {
        watchdog: start_watchdog(opts)?,
    }))
}

#[cfg(target_os = "linux")]
fn cgroup_root(opts: &HostOptions) -> Option<cgroup::CgroupRoot> {
    let off = opts.cgroups == crate::CgroupMode::Off
        || std::env::var_os("FIN_CGROUPS").is_some_and(|v| v == "off");
    if off {
        return None;
    }
    cgroup::detect_cgroup(&opts.log)
}

fn start_watchdog(
    opts: &HostOptions,
) -> Result<Option<std::sync::Arc<watchdog::Watchdog>>, HostError> {
    if !opts.watchdog {
        return Ok(None);
    }
    watchdog::Watchdog::start(&opts.binary_dir, opts.log.clone())
        .map(|w| Some(std::sync::Arc::new(w)))
        .map_err(HostError::Spawn)
}

#[allow(dead_code)]
pub(crate) const KILL_TREE: KillTree = KillTree::ProcessGroup;

/// Starts `cmd` in its group and leaf; the process has its pgid and is registered before it
/// runs any of its own code (macOS) or before `spawn` returns (Linux).
pub(crate) fn spawn(cmd: PlatformCommand, ctx: &SpawnCtx<'_>) -> io::Result<Spawned> {
    #[cfg(target_os = "linux")]
    return linux_spawn::spawn(cmd, ctx);
    #[cfg(target_os = "macos")]
    return macos_spawn::spawn(cmd, ctx);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (cmd, ctx);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no job host on this OS",
        ))
    }
}

/// After the process exists: the group's pgid (the first process makes it) and the domain's
/// registration. An error kills and reaps the process.
#[allow(dead_code)]
fn registered(ctx: &SpawnCtx<'_>, pid: u32) -> io::Result<()> {
    if let Err(e) = ctx.domain.placed(ctx.group, ctx.leaf, pid) {
        kill_failed(ctx, pid);
        return Err(e);
    }
    // Only now: a failed first registration leaves no stale pgid for the next spawn.
    if ctx.group.pgid() == 0 {
        ctx.group
            .pgid
            .store(pid as i32, std::sync::atomic::Ordering::Release);
    }
    Ok(())
}

/// Kills and reaps a process whose spawn failed after it existed. The group's first process
/// made its own process group, which is killed whole.
fn kill_failed(ctx: &SpawnCtx<'_>, pid: u32) {
    let target = if ctx.group.pgid() == 0 {
        -(pid as i32)
    } else {
        pid as i32
    };
    // SAFETY: our own child (or the group it alone made), not yet reaped.
    unsafe { libc::kill(target, libc::SIGKILL) };
    reap(pid);
}

/// The nice value for a QoS (Linux; macOS uses QoS classes).
#[allow(dead_code)]
pub(crate) fn nice_of(qos: Qos) -> Option<i32> {
    match qos {
        Qos::Normal => None,
        Qos::Utility => Some(5),
        Qos::Background => Some(10),
    }
}

/// Retries `f` while it fails with `EINTR` (`-1` and `errno`).
pub(crate) fn retry(mut f: impl FnMut() -> libc::c_int) -> io::Result<libc::c_int> {
    loop {
        let r = f();
        if r != -1 {
            return Ok(r);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Waits for `pid` to exit. `reap`: `waitpid` (the pid is released); otherwise `waitid(…
/// WNOWAIT)`, which leaves the process a zombie so its pgid cannot be reused.
pub(crate) fn wait(_os: &OsProc, pid: u32, reap: bool) -> io::Result<RawExit> {
    if reap {
        let mut status: libc::c_int = 0;
        // SAFETY: waits for our own child; `status` is a valid out-pointer.
        retry(|| unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) })?;
        return Ok(decode_status(status));
    }
    // SAFETY: a zeroed siginfo_t is a valid out-buffer for waitid.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    retry(|| unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOWAIT,
        )
    })?;
    #[cfg(target_os = "linux")]
    // SAFETY: waitid filled a SIGCHLD siginfo.
    let status = unsafe { info.si_status() };
    #[cfg(not(target_os = "linux"))]
    let status = info.si_status;
    Ok(match info.si_code {
        libc::CLD_EXITED => RawExit {
            code: Some(i64::from(status)),
            signal: None,
        },
        _ => RawExit {
            code: None,
            signal: Some(status),
        },
    })
}

fn decode_status(status: libc::c_int) -> RawExit {
    if libc::WIFEXITED(status) {
        RawExit {
            code: Some(i64::from(libc::WEXITSTATUS(status))),
            signal: None,
        }
    } else if libc::WIFSIGNALED(status) {
        RawExit {
            code: None,
            signal: Some(libc::WTERMSIG(status)),
        }
    } else {
        RawExit {
            code: None,
            signal: None,
        }
    }
}

/// Reaps an exited (zombie) child.
pub(crate) fn reap(pid: u32) {
    let mut status: libc::c_int = 0;
    // SAFETY: our own child; `status` is a valid out-pointer.
    let _ = retry(|| unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) });
}

/// `kill(-pgid, SIGKILL)`; a group with no process left is not an error.
pub(crate) fn kill_pgid(pgid: i32) -> io::Result<()> {
    if pgid <= 1 {
        return Ok(());
    }
    // SAFETY: signals a process group we created (its leader is unreaped while it is open).
    if unsafe { libc::kill(-pgid, libc::SIGKILL) } == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::ESRCH) => Ok(()),
        // macOS answers EPERM for a group of ours whose only member is the zombie leader:
        // no live member, nothing to kill.
        #[cfg(target_os = "macos")]
        Some(libc::EPERM) => Ok(()),
        _ => Err(e),
    }
}
