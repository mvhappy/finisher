//! The launcher stays the program's parent on macOS too (plan 7C Decision 3, macOS
//! amendment). macOS has no subreaper and no parent-death signal, and a process that calls
//! `setsid` and loses its parent is re-parented to `launchd`, so the session's processes are
//! found by their sandbox instead: [`prepare`] makes a fresh **session mark**
//! ([`fin_jobhost::seatbelt_mark`]) that the program's profile carries (or, unconfined, the
//! mark-only profile), and every descendant inherits it and cannot shed it.
//!
//! [`prepare`] (in the launcher, before the fork) blocks the signals it waits for and opens a
//! `kqueue` that reports them (`EVFILT_SIGNAL`), the parent's exit (`EVFILT_PROC NOTE_EXIT`)
//! and a 1 s timer. [`Prepared::in_child`] (in the forked child, before the confinement and
//! the `exec`) puts the program in a process group of its own, with the signal mask it had.
//! [`Prepared::supervise`] (in the launcher) then:
//! - forwards [`FORWARDED`] signals to the program's process group;
//! - ends the session when the program exits, when it is hung up (`SIGHUP`: `finisherd`'s
//!   session end) and when its parent dies (`NOTE_EXIT`, or `getppid` changed, checked each
//!   second): it kills the program's group and every running process that carries the mark,
//!   repeatedly, until none is left and the program is reaped;
//! - exits as the program did: its exit code, or the same signal raised on itself.
//!
//! The launcher itself is not confined (it runs no code but this) and carries no mark; the
//! program may signal only processes of its own sandbox (`(target same-sandbox)`), so it
//! cannot kill its launcher. A launcher killed by anyone else (`SIGKILL` by its parent) leaves
//! the session running: `finisherd` hangs the launcher up and kills its group only after
//! `REAP_GRACE_MS`.

use std::time::Duration;

use fin_jobhost::seatbelt_mark;

pub use crate::reap::FORWARDED;
use crate::reap::{block, child_group_and_mask, exit_as, forward, os_err, reap_available};

/// What [`prepare`] set up, for the child and the launcher after the fork.
pub struct Prepared {
    parent: libc::pid_t,
    old_mask: libc::sigset_t,
    kq: std::os::fd::OwnedFd,
    mark: String,
}

fn kevent_add(kq: libc::c_int, ident: usize, filter: i16, fflags: u32, data: isize) -> bool {
    // SAFETY: plain data, every field set below.
    let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
    ev.ident = ident;
    ev.filter = filter;
    ev.flags = libc::EV_ADD;
    ev.fflags = fflags;
    ev.data = data;
    // SAFETY: one valid change, no events out.
    unsafe { libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 }
}

/// Before the fork: the session's mark, the blocked signals and the `kqueue` that reports
/// them, the parent's exit and the timer.
pub fn prepare() -> Result<Prepared, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    // SAFETY: getppid has no failure mode.
    let parent = unsafe { libc::getppid() };
    let mark = seatbelt_mark::new_mark().map_err(|e| format!("getentropy: {e}"))?;
    let (_, old_mask) = block(&[libc::SIGCHLD, libc::SIGHUP])?;
    // SAFETY: kqueue takes no arguments; a kqueue is not inherited across `fork`.
    let raw = unsafe { libc::kqueue() };
    if raw < 0 {
        return Err(os_err("kqueue"));
    }
    // SAFETY: a fresh descriptor owned here.
    let kq = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
    let fd = kq.as_raw_fd();
    for &s in FORWARDED.iter().chain(&[libc::SIGCHLD, libc::SIGHUP]) {
        if !kevent_add(fd, s as usize, libc::EVFILT_SIGNAL, 0, 0) {
            return Err(os_err("kevent(EVFILT_SIGNAL)"));
        }
    }
    // The parent may already be gone (`ESRCH`); the timer's `getppid` check sees that.
    let _ = kevent_add(fd, parent as usize, libc::EVFILT_PROC, libc::NOTE_EXIT, 0);
    if !kevent_add(fd, 0, libc::EVFILT_TIMER, libc::NOTE_SECONDS, 1) {
        return Err(os_err("kevent(EVFILT_TIMER)"));
    }
    // SAFETY: as above.
    if unsafe { libc::getppid() } != parent {
        return Err("the launcher's parent exited".into());
    }
    Ok(Prepared {
        parent,
        old_mask,
        kq,
        mark,
    })
}

/// What one `kevent` reported.
enum Event {
    Signal(libc::c_int),
    ParentExit,
    Tick,
}

impl Prepared {
    /// The session's mark, which the program's profile must carry.
    pub fn mark(&self) -> &str {
        &self.mark
    }

    /// Nothing to undo when the program could not be started.
    pub fn abandon(&self) {}

    /// In the forked child: a process group of its own and the signal mask the launcher
    /// started with.
    pub fn in_child(&self) -> Result<(), String> {
        child_group_and_mask(&self.old_mask)
    }

    fn next(&self) -> Option<Event> {
        use std::os::fd::AsRawFd;
        // SAFETY: plain data the call fills.
        let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: room for one event; no changes; no timeout (the timer ticks).
        let n = unsafe {
            libc::kevent(
                self.kq.as_raw_fd(),
                std::ptr::null(),
                0,
                &mut ev,
                1,
                std::ptr::null(),
            )
        };
        if n <= 0 {
            return None; // EINTR, or nothing: look again
        }
        Some(match ev.filter {
            libc::EVFILT_SIGNAL => Event::Signal(ev.ident as libc::c_int),
            libc::EVFILT_PROC => Event::ParentExit,
            _ => Event::Tick,
        })
    }

    /// In the launcher after the fork: serves the session of `child` and exits as it did.
    pub fn supervise(self, child: libc::pid_t) -> ! {
        // Both sides set the group, so a signal forwarded at once finds it.
        // SAFETY: setpgid on our own child; EACCES once it has exec'd is ignored.
        unsafe { libc::setpgid(child, child) };
        let mut main: Option<libc::c_int> = None;
        // SAFETY: getppid has no failure mode.
        let orphaned = || unsafe { libc::getppid() } != self.parent;
        // A child that exited before the kqueue saw `SIGCHLD` is reaped here.
        if !reap_available(child, &mut main) || main.is_some() {
            self.end(child, main)
        }
        loop {
            match self.next() {
                Some(Event::Signal(libc::SIGCHLD)) => {
                    // No child left at all also ends the session (the program is gone).
                    if !reap_available(child, &mut main) || main.is_some() {
                        break;
                    }
                }
                Some(Event::Signal(libc::SIGHUP)) | Some(Event::ParentExit) => break,
                Some(Event::Signal(s)) => forward(child, s),
                Some(Event::Tick) | None => {
                    if orphaned() {
                        break;
                    }
                }
            }
        }
        self.end(child, main)
    }

    /// Kills the program's group and every process that carries the mark until none is left
    /// and the program is reaped, then exits as the program did.
    fn end(&self, child: libc::pid_t, mut main: Option<libc::c_int>) -> ! {
        // Once the program is reaped its group's id may be free for another process; its
        // members carry the mark and go below either way.
        if main.is_none() {
            // SAFETY: kill with plain integers; the program is alive or an unreaped zombie.
            unsafe { libc::kill(-child, libc::SIGKILL) };
        }
        loop {
            let signalled = seatbelt_mark::kill_marked(&self.mark);
            let children = reap_available(child, &mut main);
            if signalled == 0 && !children {
                break;
            }
            if children && main.is_none() {
                // SAFETY: our own child, alive or an unreaped zombie.
                unsafe { libc::kill(child, libc::SIGKILL) };
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        exit_as(main.unwrap_or(libc::SIGKILL))
    }
}
