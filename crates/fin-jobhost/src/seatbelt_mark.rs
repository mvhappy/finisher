//! Seatbelt session marks (macOS; plan 7C Decision 3, macOS amendment). macOS has no
//! subreaper, and a process that calls `setsid` and loses its parent is re-parented to
//! `launchd`, so no ancestor can find it by parentage; unprivileged processes cannot make an
//! audit session or a coalition either. What every descendant does keep is its sandbox: it is
//! inherited across `fork` and `exec`, and a sandboxed process can neither leave it nor apply
//! another (a nested `sandbox_init` is refused). So each session's profile carries a **mark**,
//! a `mach-lookup` rule naming a random service no one provides, and whoever ends the session
//! finds its processes by asking the kernel which processes' sandboxes allow that name
//! (`sandbox_check`, the call the system's own daemons use to check a peer).
//!
//! A process carries `mark` when it is sandboxed, its sandbox allows looking `mark` up, and
//! refuses looking up [`not`]`(mark)`: a profile that allows every name (some system agents'
//! do) is told apart by that second name, which a session's profile refuses (with `(deny
//! default)`, or explicitly in the mark-only profile of an unconfined session). Only processes
//! of this user are considered; nothing else could be signalled.

use std::ffi::{CString, c_char, c_int};

/// The `(param …)` that holds the mark in a profile.
pub const PARAM: &str = "FIN_SESSION_MARK";
/// The `(param …)` that holds [`not`]`(mark)` in the mark-only profile.
pub const PARAM_NOT: &str = "FIN_SESSION_MARK_NOT";
/// The rule a confined session's profile ends with.
pub const RULE: &str = "(allow mach-lookup (global-name (param \"FIN_SESSION_MARK\")))\n";
/// The whole profile of a session that is not confined: everything allowed, and the mark.
pub const MARK_ONLY: &str = "(version 1)\n(allow default)\n\
                             (deny mach-lookup (global-name (param \"FIN_SESSION_MARK_NOT\")))\n";

const SANDBOX_FILTER_NONE: c_int = 0;
const SANDBOX_FILTER_GLOBAL_NAME: c_int = 2;
/// Do not log the check as a violation.
const SANDBOX_CHECK_NO_REPORT: c_int = 0x4000_0000;

unsafe extern "C" {
    // libsystem_sandbox (part of libSystem). Variadic: the filter's argument follows `type`.
    fn sandbox_check(pid: libc::pid_t, operation: *const c_char, r#type: c_int, ...) -> c_int;
}

/// A new random mark: `org.finisher.session.<32 hex digits>`.
pub fn new_mark() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    // SAFETY: a valid buffer of at most 256 bytes.
    if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("org.finisher.session.{hex}"))
}

/// The name a marked profile refuses (the mark-only profile names it, a confined one refuses
/// it with `(deny default)`).
pub fn not(mark: &str) -> String {
    format!("{mark}.not")
}

/// The profile parameters a marked profile needs.
pub fn params(mark: &str) -> [(String, String); 2] {
    [(PARAM.into(), mark.into()), (PARAM_NOT.into(), not(mark))]
}

fn allows_lookup(pid: libc::pid_t, name: &CString) -> bool {
    // SAFETY: NUL-terminated strings that outlive the call; the filter takes one string.
    unsafe {
        sandbox_check(
            pid,
            c"mach-lookup".as_ptr(),
            SANDBOX_FILTER_GLOBAL_NAME | SANDBOX_CHECK_NO_REPORT,
            name.as_ptr(),
        ) == 0
    }
}

/// Whether `pid`'s sandbox carries `mark` (see the module doc).
pub fn carries(pid: libc::pid_t, mark: &str) -> bool {
    let (Ok(yes), Ok(no)) = (CString::new(mark), CString::new(not(mark))) else {
        return false;
    };
    // SAFETY: no filter argument.
    let sandboxed = unsafe { sandbox_check(pid, std::ptr::null(), SANDBOX_FILTER_NONE) } == 1;
    sandboxed && allows_lookup(pid, &yes) && !allows_lookup(pid, &no)
}

/// `pid`'s owner and whether it is a zombie, or `None` when it is gone.
fn short_info(pid: libc::pid_t) -> Option<(libc::uid_t, bool)> {
    // SAFETY: plain data the call fills.
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as c_int;
    // SAFETY: a valid buffer of `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    (n == size).then_some((info.pbsi_uid, info.pbsi_status == libc::SZOMB))
}

/// Every process now running (zombies excluded) of this user whose sandbox carries `mark`.
pub fn marked(mark: &str) -> Vec<libc::pid_t> {
    // SAFETY: getuid has no failure mode.
    let me = unsafe { libc::getuid() };
    let mut buf: Vec<libc::pid_t> = Vec::new();
    loop {
        // SAFETY: a NULL buffer asks for the count.
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return Vec::new();
        }
        buf.resize(n as usize + 64, 0);
        let bytes = (buf.len() * std::mem::size_of::<libc::pid_t>()) as c_int;
        // SAFETY: `buf` holds `bytes` bytes; the call returns the number of pids written.
        let got = unsafe { libc::proc_listallpids(buf.as_mut_ptr().cast(), bytes) };
        if got < 0 {
            return Vec::new();
        }
        // A full buffer may have missed some: ask again, larger.
        if (got as usize) < buf.len() {
            buf.truncate(got as usize);
            break;
        }
    }
    buf.into_iter()
        .filter(|&p| p > 1)
        .filter(|&p| short_info(p).is_some_and(|(uid, zombie)| uid == me && !zombie))
        .filter(|&p| carries(p, mark))
        .collect()
}

/// `SIGKILL`s every running process that carries `mark`; answers how many were signalled.
pub fn kill_marked(mark: &str) -> usize {
    marked(mark)
        .into_iter()
        // SAFETY: kill with plain integers.
        .filter(|&p| unsafe { libc::kill(p, libc::SIGKILL) } == 0)
        .count()
}

/// Kills every process that carries `mark` until none is left running, at most `limit`
/// passes apart by 2 ms (each pass also takes what the last one's victims forked before they
/// died). Answers whether none is left.
pub fn sweep(mark: &str, limit: usize) -> bool {
    for _ in 0..limit {
        if kill_marked(mark) == 0 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    marked(mark).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_fresh_and_well_formed() {
        let (a, b) = (new_mark().unwrap(), new_mark().unwrap());
        assert_ne!(a, b);
        let hex = a.strip_prefix("org.finisher.session.").unwrap();
        assert_eq!(hex.len(), 32);
        assert!(hex.bytes().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn an_unsandboxed_process_carries_no_mark() {
        let mark = new_mark().unwrap();
        assert!(!carries(std::process::id() as i32, &mark));
        assert!(marked(&mark).is_empty());
    }
}
