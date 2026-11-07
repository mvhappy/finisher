//! Test-only access to the watchdog and the spawn broker (feature `test-hooks`).

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
