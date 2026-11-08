//! Test-only access to the watchdog, the spawn broker and single processes (feature
//! `test-hooks`).

use crate::Host;

#[cfg(unix)]
impl Host {
    /// The running watchdog's pid.
    pub fn watchdog_pid_for_test(&self) -> Option<u32> {
        self.inner.domain.watchdog().and_then(|w| w.pid())
    }

    /// Writes `line` to the watchdog as it is (a stranger's `+g`/`-g`).
    pub fn watchdog_line_for_test(&self, line: &str) {
        if let Some(w) = self.inner.domain.watchdog() {
            w.raw_line(line);
        }
    }

    /// The pgids the host has listed with the watchdog.
    pub fn watchdog_groups_for_test(&self) -> Vec<i32> {
        self.inner
            .domain
            .watchdog()
            .map(|w| w.groups())
            .unwrap_or_default()
    }
}

#[cfg(windows)]
impl Host {
    /// The spawn broker's pid and the image it was made from, if there is one.
    pub fn broker_for_test(&self) -> Option<(u32, std::path::PathBuf)> {
        self.inner.domain.broker_for_test()
    }
}

impl Host {
    /// Kills one process (`SIGKILL` / `TerminateProcess`), as a crash would: the host did not
    /// decide it, so it is reported `Exited`.
    pub fn kill_proc_for_test(&self, proc: crate::ProcId) {
        let Some(p) = self.inner.proc(proc) else {
            return;
        };
        if p.state().exited {
            return;
        }
        kill_pid(p.pid);
    }

    /// The process's waiter holds its observed exit for `hold`: a kill of its group then
    /// seems not to end it, so it is abandoned after the kill wait and exits `hold` later.
    pub fn abandon_for_test(&self, proc: crate::ProcId, hold: std::time::Duration) {
        if let Some(p) = self.inner.proc(proc) {
            p.state().hold_exit_for_test = Some(hold);
        }
    }
}

#[cfg(unix)]
fn kill_pid(pid: u32) {
    // SAFETY: a child of this host that has not exited (its waiter has not reaped it).
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: plain calls on a handle opened here and closed once.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !h.is_null() {
            TerminateProcess(h, crate::KILLED_EXIT_CODE);
            CloseHandle(h);
        }
    }
}
