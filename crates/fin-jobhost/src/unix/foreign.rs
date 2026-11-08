//! Foreign processes (Decision 22): members of a group the host did not spawn. The process
//! groups list a pgid's members (Linux a `/proc` scan, macOS `proc_listpids(PROC_PGRP_ONLY)`);
//! the cgroup domain lists its `cgroup.procs`. Each scan (at `result`, before a kill and at
//! each exit) adds what it sees to [`Seen`], so a grandchild killed with its group is still
//! counted at the exit.

use std::collections::BTreeSet;
use std::sync::Mutex;

/// The pids a group's domain has seen: the host's and the others.
#[derive(Default)]
pub(crate) struct Seen {
    inner: Mutex<(BTreeSet<u32>, BTreeSet<u32>)>,
}

impl Seen {
    /// A process the host spawned.
    pub fn spawned(&self, pid: u32) {
        self.lock().0.insert(pid);
    }

    /// Records `members`; returns the foreign processes seen so far.
    pub fn note(&self, members: impl IntoIterator<Item = u32>) -> u32 {
        let mut g = self.lock();
        let (spawned, foreign) = &mut *g;
        foreign.extend(members.into_iter().filter(|p| !spawned.contains(p)));
        foreign.len() as u32
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (BTreeSet<u32>, BTreeSet<u32>)> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The processes whose process group is `pgid`.
#[cfg(target_os = "linux")]
pub(crate) fn pgid_members(pgid: i32) -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            // `pid (comm) state ppid pgrp …`; comm may hold spaces and parentheses.
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| {
                    let rest = s.rsplit_once(") ")?.1.to_owned();
                    rest.split(' ').nth(2)?.parse::<i32>().ok()
                })
                == Some(pgid)
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub(crate) fn pgid_members(pgid: i32) -> Vec<u32> {
    const PROC_PGRP_ONLY: u32 = 2;
    let mut buf = vec![0 as libc::c_int; 4096];
    let bytes = (buf.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int;
    // SAFETY: proc_listpids writes at most `bytes` bytes of pids into `buf`; it returns the
    // number of bytes written.
    let n =
        unsafe { libc::proc_listpids(PROC_PGRP_ONLY, pgid as u32, buf.as_mut_ptr().cast(), bytes) };
    let count = (n.max(0) as usize / std::mem::size_of::<libc::c_int>()).min(buf.len());
    buf.truncate(count);
    buf.into_iter()
        .filter(|&p| p > 0)
        .map(|p| p as u32)
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn pgid_members(_pgid: i32) -> Vec<u32> {
    Vec::new()
}
