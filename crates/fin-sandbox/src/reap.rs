//! What the Linux and macOS session reapers share (plan 7C Decision 3 as amended): the
//! launcher stays the program's parent, forwards [`FORWARDED`] signals to the program's process
//! group, reaps its children and, once the session is over, exits as the program did. How each
//! OS finds the session's other descendants differs: [`crate::linux::reaper`] (a subreaper and
//! the optional cgroup) and [`crate::macos::reaper`] (the session's Seatbelt mark).

/// The signals the launcher passes on to the program's process group.
pub const FORWARDED: &[libc::c_int] = &[
    libc::SIGTERM,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGUSR1,
    libc::SIGUSR2,
    libc::SIGWINCH,
    libc::SIGCONT,
    libc::SIGALRM,
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
];

pub(crate) fn os_err(call: &str) -> String {
    format!("{call}: {}", std::io::Error::last_os_error())
}

/// Sends `sig` to the program's group, or to the program where it left that group.
pub(crate) fn forward(child: libc::pid_t, sig: libc::c_int) {
    // SAFETY: kill with plain integers.
    unsafe {
        if libc::kill(-child, sig) != 0 {
            libc::kill(child, sig);
        }
    }
}

/// Reaps every exited child without waiting; the program's status goes to `main`. Answers
/// whether any child is left.
pub(crate) fn reap_available(child: libc::pid_t, main: &mut Option<libc::c_int>) -> bool {
    loop {
        let mut status = 0;
        // SAFETY: a valid out pointer.
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if r == child {
            *main = Some(status);
        } else if r == 0 {
            return true;
        } else if r < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return false; // ECHILD: none left
        }
    }
}

/// Blocks [`FORWARDED`] and `extra` (the launcher waits for them instead), after resetting an
/// inherited `SIG_IGN` (or `SA_NOCLDWAIT`) on `SIGCHLD`, which survives `exec` and would let
/// the kernel reap the program unseen. Answers the set blocked and the mask before.
pub(crate) fn block(extra: &[libc::c_int]) -> Result<(libc::sigset_t, libc::sigset_t), String> {
    // SAFETY: plain data, initialised by sigemptyset before use.
    let mut waited: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    let mut old_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointers to owned sets.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        libc::sigemptyset(&mut waited);
        for &s in FORWARDED.iter().chain(extra) {
            libc::sigaddset(&mut waited, s);
        }
        if libc::sigprocmask(libc::SIG_BLOCK, &waited, &mut old_mask) != 0 {
            return Err(os_err("sigprocmask"));
        }
    }
    Ok((waited, old_mask))
}

/// In the forked child: a process group of its own and the signal mask the launcher started
/// with.
pub(crate) fn child_group_and_mask(old_mask: &libc::sigset_t) -> Result<(), String> {
    // The launcher sets the same group from its side, and while its call is under way macOS
    // refuses the child's own (`EPERM`, the child not yet leading a group): what counts is
    // that the child leads its group, so it looks again for up to 100 ms.
    for _ in 0..100 {
        // SAFETY: setpgid and getpgrp on the calling process.
        if unsafe { libc::setpgid(0, 0) } == 0 || unsafe { libc::getpgrp() == libc::getpid() } {
            break;
        }
        // SAFETY: a plain sleep.
        unsafe { libc::usleep(1000) };
    }
    // SAFETY: as above.
    if unsafe { libc::getpgrp() != libc::getpid() } {
        return Err(os_err("setpgid"));
    }
    // SAFETY: a valid pointer to the saved mask.
    if unsafe { libc::sigprocmask(libc::SIG_SETMASK, old_mask, std::ptr::null_mut()) } != 0 {
        return Err(os_err("sigprocmask"));
    }
    Ok(())
}

/// Exits with the program's wait status: its code, or its signal raised on the launcher.
pub(crate) fn exit_as(status: libc::c_int) -> ! {
    if libc::WIFEXITED(status) {
        std::process::exit(libc::WEXITSTATUS(status));
    }
    let sig = if libc::WIFSIGNALED(status) {
        libc::WTERMSIG(status)
    } else {
        libc::SIGKILL
    };
    let none = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: plain calls on the launcher itself, with valid pointers.
    unsafe {
        libc::setrlimit(libc::RLIMIT_CORE, &none);
        libc::signal(sig, libc::SIG_DFL);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, sig);
        libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
        libc::raise(sig);
    }
    std::process::exit(128 + sig)
}
