//! The launcher stays the program's parent (plan 7C Decision 3 as amended after the Task 11
//! review): a process that leaves its process group (`setsid`, `setpgid`) must not outlive the
//! session, and only an ancestor that stays can find it.
//!
//! [`prepare`] (in the launcher, before the fork) makes the launcher a child subreaper
//! (`PR_SET_CHILD_SUBREAPER`: every orphaned descendant is re-parented to it, not to init),
//! asks for [`parent_death_signal`] when its own parent dies (`PR_SET_PDEATHSIG`), blocks the
//! signals it waits for, and, where a cgroup v2 kill domain is delegated (below), makes the
//! session's cgroup. [`Prepared::in_child`] (in the forked child, before the confinement and
//! the `exec`) puts the program in a process group of its own, in that cgroup, with the
//! signal mask it had. [`Prepared::supervise`] (in the launcher) then:
//! - forwards [`FORWARDED`] signals to the program's process group (the launcher's own group
//!   holds only the launcher, so a signal `finisherd` sends to it reaches the program once);
//! - ends the session when the program exits, when it is hung up (`SIGHUP`: `finisherd`'s
//!   session end, as a terminal's hang-up ends a login session) and when its parent dies (the
//!   death signal, or `getppid` changed, checked each second): it kills the program's group,
//!   the cgroup (`cgroup.kill`) and every remaining child, repeatedly, until none is left
//!   (each kill re-parents the killed process's children to it), then removes the cgroup;
//! - exits as the program did: its exit code, or the same signal raised on itself (core dumps
//!   off), so the exit codes and signals are still the program's.
//!
//! The cgroup is a bonus layer, used only where it can be: on cgroup v2 with `cgroup.kill`
//! (kernel ≥ 5.14), when the launcher's cgroup is writable and holds only the launcher's
//! parent and the parent's children (the conditions `fin-jobhost`'s `cgroup::detect` checks
//! for itself: a delegated unit, or the engine's `<cg>/host`), and not with `FIN_CGROUPS=off`.
//! Like that check, writability cannot tell a `Delegate=yes` unit from any other unit of the
//! user's own manager (systemd owns those too); such a unit would get a sub-cgroup it did not
//! delegate, which is the residual both share.
//! The session's cgroup is `<own>/fin-session-<launcher pid>`; no controller is enabled.
//!
//! The launcher itself is not confined (it runs no code but this); the program is, after the
//! fork. Where Landlock scopes signals (ABI ≥ 6) the confined program cannot signal its
//! unconfined launcher; below that it could kill it, and its orphans would then outlive the
//! session unless the cgroup kills them (the program itself dies with the launcher: its own
//! death signal is `SIGKILL`).

use std::path::{Path, PathBuf};
use std::time::Duration;

pub use crate::reap::FORWARDED;
use crate::reap::{block, child_group_and_mask, exit_as, forward, os_err, reap_available};

/// The signal the launcher asks for when its parent dies: not `SIGHUP`, because the kernel
/// sends it when the parent *thread* that started the launcher exits, so the launcher checks
/// that its parent process really changed.
pub fn parent_death_signal() -> libc::c_int {
    libc::SIGRTMIN()
}

/// In the child just before `exec`: the program dies with the launcher (`SIGKILL`), asked
/// for after the confinement (a change of credentials clears it), and refused when the
/// launcher is already gone.
pub fn die_with_launcher(launcher: libc::pid_t) -> Result<(), String> {
    // SAFETY: prctl with integer arguments only.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) } != 0 {
        return Err(os_err("PR_SET_PDEATHSIG"));
    }
    // SAFETY: as above.
    if unsafe { libc::getppid() } != launcher {
        return Err("the launcher exited".into());
    }
    Ok(())
}

/// What [`prepare`] set up, for the child and the launcher after the fork.
pub struct Prepared {
    parent: libc::pid_t,
    old_mask: libc::sigset_t,
    waited: libc::sigset_t,
    cgroup: Option<PathBuf>,
}

/// Before the fork: subreaper, parent-death signal, the blocked signals, the cgroup.
pub fn prepare() -> Result<Prepared, String> {
    // SAFETY: getppid has no failure mode.
    let parent = unsafe { libc::getppid() };
    // SAFETY: prctl with integer arguments only.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(os_err("PR_SET_CHILD_SUBREAPER"));
    }
    // SAFETY: as above.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, parent_death_signal(), 0, 0, 0) } != 0 {
        return Err(os_err("PR_SET_PDEATHSIG"));
    }
    // The parent died before the death signal was asked for: nobody is left to serve.
    // SAFETY: as above.
    if unsafe { libc::getppid() } != parent {
        return Err("the launcher's parent exited".into());
    }
    let (waited, old_mask) = block(&[libc::SIGCHLD, libc::SIGHUP, parent_death_signal()])?;
    Ok(Prepared {
        parent,
        old_mask,
        waited,
        cgroup: session_cgroup(),
    })
}

impl Prepared {
    /// The session's cgroup, where one was made.
    pub fn cgroup(&self) -> Option<&Path> {
        self.cgroup.as_deref()
    }

    /// Removes the session's cgroup when the program could not be started.
    pub fn abandon(&self) {
        if let Some(dir) = &self.cgroup {
            let _ = std::fs::remove_dir(dir);
        }
    }

    /// In the forked child: a process group of its own, the session's cgroup, and the signal
    /// mask the launcher started with.
    pub fn in_child(&self) -> Result<(), String> {
        child_group_and_mask(&self.old_mask)?;
        if let Some(dir) = &self.cgroup {
            // Not fatal: the reaper still kills every descendant.
            let _ = std::fs::write(dir.join("cgroup.procs"), b"0");
        }
        Ok(())
    }

    /// In the launcher after the fork: serves the session of `child` and exits as it did.
    pub fn supervise(self, child: libc::pid_t) -> ! {
        // Both sides set the group, so a signal forwarded at once finds it.
        // SAFETY: setpgid on our own child; EACCES once it has exec'd is ignored.
        unsafe { libc::setpgid(child, child) };
        let mut main: Option<libc::c_int> = None;
        loop {
            // SAFETY: plain data the call fills.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let wait = libc::timespec {
                tv_sec: 1,
                tv_nsec: 0,
            };
            // SAFETY: valid pointers to the set, the info and the timeout.
            let sig = unsafe { libc::sigtimedwait(&self.waited, &mut info, &wait) };
            // SAFETY: getppid has no failure mode.
            let orphaned = || unsafe { libc::getppid() } != self.parent;
            match sig {
                libc::SIGCHLD => {
                    // No child left at all also ends the session (the program is gone).
                    if !reap_available(child, &mut main) || main.is_some() {
                        break;
                    }
                }
                libc::SIGHUP => break,
                s if s == parent_death_signal() => {
                    if orphaned() {
                        break;
                    }
                }
                s if s > 0 => forward(child, s),
                _ => {
                    if orphaned() {
                        break;
                    }
                }
            }
        }
        kill_all(child, self.cgroup.as_deref(), &mut main);
        if let Some(dir) = &self.cgroup {
            remove_cgroup(dir);
        }
        exit_as(main.unwrap_or(libc::SIGKILL))
    }
}

/// Kills the program's group, the cgroup and every child until no child is left.
fn kill_all(child: libc::pid_t, cgroup: Option<&Path>, main: &mut Option<libc::c_int>) {
    // Once the program is reaped its group's id may be free for another process; its members
    // are the launcher's descendants and go below either way.
    if main.is_none() {
        // SAFETY: kill with plain integers; the program is alive or an unreaped zombie.
        unsafe { libc::kill(-child, libc::SIGKILL) };
    }
    if let Some(dir) = cgroup {
        let _ = std::fs::write(dir.join("cgroup.kill"), b"1");
    }
    while reap_available(child, main) {
        for pid in children() {
            // SAFETY: as above; each is our own child.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The launcher's children: `/proc/self/task/<pid>/children` (the launcher has one thread),
/// or, where the kernel lacks it, every process whose parent is the launcher.
fn children() -> Vec<libc::pid_t> {
    let me = std::process::id();
    if let Ok(text) = std::fs::read_to_string(format!("/proc/self/task/{me}/children")) {
        return text
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect();
    }
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&p| parent_of(p) == Some(me))
        .filter_map(|p| libc::pid_t::try_from(p).ok())
        .collect()
}

fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?.1.split(' ').nth(1)?.parse().ok()
}

// --- The optional cgroup layer ---

/// Where the cgroup2 hierarchy is mounted.
fn mount_point() -> PathBuf {
    std::fs::read_to_string("/proc/self/mounts")
        .ok()
        .and_then(|m| {
            m.lines()
                .map(|l| l.split(' ').collect::<Vec<_>>())
                .find(|f| f.len() > 2 && f[2] == "cgroup2")
                .map(|f| PathBuf::from(f[1]))
        })
        .unwrap_or_else(|| PathBuf::from("/sys/fs/cgroup"))
}

fn writable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: access only reads the path.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// The launcher's own cgroup, where it may make the session's (see the module doc).
pub fn delegated_cgroup() -> Result<PathBuf, String> {
    if std::env::var_os("FIN_CGROUPS").is_some_and(|v| v == "off") {
        return Err("FIN_CGROUPS=off".into());
    }
    let own =
        std::fs::read_to_string("/proc/self/cgroup").map_err(|e| format!("no cgroup: {e}"))?;
    let rel = own
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .ok_or("not on cgroup v2")?;
    let here = mount_point().join(rel.trim_start_matches('/'));
    let me = std::process::id();
    // SAFETY: getppid has no failure mode.
    let parent = u32::try_from(unsafe { libc::getppid() }).unwrap_or(0);
    let procs: Vec<u32> = std::fs::read_to_string(here.join("cgroup.procs"))
        .map_err(|e| format!("{}: {e}", here.display()))?
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    let ours = |p: u32| p == me || p == parent || parent_of(p) == Some(parent);
    if !procs.contains(&me) || !procs.iter().all(|&p| ours(p)) {
        return Err(format!("{} holds other processes", here.display()));
    }
    if !here.join("cgroup.kill").exists() {
        return Err("no cgroup.kill (kernel < 5.14)".into());
    }
    let files = ["cgroup.procs", "cgroup.subtree_control"];
    if !writable(&here) || files.iter().any(|f| !writable(&here.join(f))) {
        return Err(format!("{} is not delegated to us", here.display()));
    }
    Ok(here)
}

/// Makes `<own>/fin-session-<pid>` where [`delegated_cgroup`] allows it.
fn session_cgroup() -> Option<PathBuf> {
    let dir = delegated_cgroup()
        .ok()?
        .join(format!("fin-session-{}", std::process::id()));
    std::fs::create_dir(&dir).ok()?;
    Some(dir)
}

/// Removes the emptied session cgroup (its last process may take a moment to leave).
fn remove_cgroup(dir: &Path) {
    for _ in 0..500 {
        match std::fs::remove_dir(dir) {
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                std::thread::sleep(Duration::from_millis(2));
            }
            _ => return,
        }
    }
}
