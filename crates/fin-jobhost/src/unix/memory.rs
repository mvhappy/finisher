//! One process's memory figure (Decision 20): Linux `RssAnon + RssShmem` from
//! `/proc/<pid>/status` (file-backed pages are reclaimable and not the job's), macOS
//! `ri_phys_footprint` from `proc_pid_rusage(RUSAGE_INFO_V4)`. `None` for a process that is
//! gone or unreadable.

#[cfg(target_os = "linux")]
pub(crate) fn sample(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = status.lines().find(|l| l.starts_with(name))?;
        let kb = line[name.len()..].trim().trim_end_matches("kB").trim();
        kb.parse::<u64>().ok().map(|k| k * 1024)
    };
    Some(field("RssAnon:")? + field("RssShmem:")?)
}

#[cfg(target_os = "macos")]
pub(crate) fn sample(pid: u32) -> Option<u64> {
    // SAFETY: a zeroed rusage_info_v4 is a valid out-buffer for this flavor.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: proc_pid_rusage writes one rusage_info_v4 into `info`.
    let r = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V4,
            (&mut info as *mut libc::rusage_info_v4).cast(),
        )
    };
    (r == 0).then_some(info.ri_phys_footprint)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn sample(_pid: u32) -> Option<u64> {
    None
}
