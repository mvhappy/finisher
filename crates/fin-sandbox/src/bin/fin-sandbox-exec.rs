//! `fin-sandbox-exec --policy-fd <n> -- <program> [args…]`: confines a program `finisherd` starts
//! itself (an agent harness, a hook command) and becomes it. See `fin_sandbox::launch`.

use std::io::Read;

use fin_sandbox::launch::{
    self, EXIT_CANNOT_EXEC, EXIT_NOT_FOUND, EXIT_UNAVAILABLE, EXIT_USAGE, MAX_REQUEST,
};
use fin_sandbox::policy::Profile;
use fin_sandbox::status::{DisabledReason, Mechanism, mechanism};

fn fail(exit: i32, code: &str, profile: Option<Profile>, need: &str) -> ! {
    eprintln!("{}", launch::failure_line(code, profile, need));
    std::process::exit(exit)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = launch::parse_args(&args).unwrap_or_else(|e| fail(EXIT_USAGE, "USAGE", None, &e));
    if let Some(v) = launch::refused_env(|v| std::env::var_os(v).is_some()) {
        fail(
            EXIT_USAGE,
            "USAGE",
            None,
            &format!("{v} is set: the request comes only through --policy-fd"),
        );
    }
    let bytes = read_fd(args.policy_fd)
        .unwrap_or_else(|e| fail(EXIT_UNAVAILABLE, "SANDBOX_UNAVAILABLE", None, &e));
    let (req, policy) = launch::read_request(&bytes)
        .unwrap_or_else(|e| fail(EXIT_UNAVAILABLE, "SANDBOX_UNAVAILABLE", None, &e));
    let profile = Some(req.profile());
    match mechanism() {
        Mechanism::Disabled {
            reason: DisabledReason::LandlockDisabled,
        } => {}
        Mechanism::Disabled { reason } => fail(
            EXIT_UNAVAILABLE,
            "SANDBOX_UNAVAILABLE",
            profile,
            &format!("a launcher sandbox on this OS ({reason:?})"),
        ),
        Mechanism::Unavailable { need } => {
            fail(EXIT_UNAVAILABLE, "SANDBOX_UNAVAILABLE", profile, &need)
        }
        Mechanism::Enforced { .. } => {
            if let Err(e) = os::apply(&policy) {
                fail(EXIT_UNAVAILABLE, "SANDBOX_UNAVAILABLE", profile, &e);
            }
        }
    }
    let e = os::exec(&args.argv);
    let exit = if e.kind() == std::io::ErrorKind::NotFound {
        EXIT_NOT_FOUND
    } else {
        EXIT_CANNOT_EXEC
    };
    fail(
        exit,
        "EXEC_FAILED",
        profile,
        &format!("{}: {e}", args.argv[0]),
    )
}

/// Reads the request from the inherited descriptor to its end and closes it.
fn read_fd(fd: i32) -> Result<Vec<u8>, String> {
    let file = os::take_fd(fd)?;
    let mut bytes = Vec::new();
    file.take(MAX_REQUEST + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("descriptor {fd}: {e}"))?;
    if bytes.len() as u64 > MAX_REQUEST {
        return Err(format!("the request is larger than {MAX_REQUEST} bytes"));
    }
    Ok(bytes)
}

#[cfg(unix)]
mod os {
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::os::unix::process::CommandExt;

    use fin_sandbox::SandboxPolicy;

    pub fn take_fd(fd: i32) -> Result<File, String> {
        // SAFETY: `fcntl(F_GETFD)` only queries the descriptor.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
            return Err(format!(
                "descriptor {fd}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: the descriptor is open and was handed to this process to own; nothing else in
        // it uses the number.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    #[cfg(target_os = "linux")]
    pub fn apply(policy: &SandboxPolicy) -> Result<(), String> {
        fin_sandbox::linux::apply::restrict_self(policy).map_err(|e| e.to_string())?;
        if let Some(n) = policy.limits.open_files {
            open_files(n)?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply(_policy: &SandboxPolicy) -> Result<(), String> {
        Err("the launcher sandbox of this OS (macOS: plan 7C Task 8)".into())
    }

    /// `RLIMIT_NOFILE` to `n` (never above the current hard limit).
    #[cfg(target_os = "linux")]
    fn open_files(n: u32) -> Result<(), String> {
        // SAFETY: plain data, written by getrlimit before use.
        let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) } != 0 {
            return Err(format!("getrlimit: {}", std::io::Error::last_os_error()));
        }
        let n = libc::rlim_t::from(n).min(r.rlim_max);
        let r = libc::rlimit {
            rlim_cur: n,
            rlim_max: n,
        };
        // SAFETY: a valid pointer to an initialised value.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &r) } != 0 {
            return Err(format!("setrlimit: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Replaces the process with `argv`. Descriptors 3 and up are marked close-on-exec first,
    /// so the program inherits only its standard streams. Returns only on failure.
    pub fn exec(argv: &[String]) -> std::io::Error {
        #[cfg(target_os = "linux")]
        // SAFETY: `close_range` with `CLOSE_RANGE_CLOEXEC` only sets flags on descriptors.
        unsafe {
            libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            );
        }
        std::process::Command::new(&argv[0]).args(&argv[1..]).exec()
    }
}

#[cfg(not(unix))]
mod os {
    use fin_sandbox::SandboxPolicy;

    pub fn take_fd(fd: i32) -> Result<std::fs::File, String> {
        Err(format!(
            "descriptor {fd}: the Windows launcher takes a handle (plan 7C Task 9)"
        ))
    }

    pub fn apply(_policy: &SandboxPolicy) -> Result<(), String> {
        Err("the Windows launcher (plan 7C Task 9)".into())
    }

    pub fn exec(_argv: &[String]) -> std::io::Error {
        std::io::Error::other("the Windows launcher (plan 7C Task 9)")
    }
}
