//! `fin-sandbox-exec --policy-fd <n> -- <program> [args…]`: confines a program `finisherd` starts
//! itself (an agent harness, a hook command) and becomes it. See `fin_sandbox::launch`.

use std::fs::File;
use std::io::{Read, Write};
use std::sync::Mutex;

use fin_sandbox::launch::{
    self, EXIT_CANNOT_EXEC, EXIT_NOT_FOUND, EXIT_UNAVAILABLE, EXIT_USAGE, MAX_REQUEST,
};
use fin_sandbox::policy::Profile;
use fin_sandbox::status::{DisabledReason, Mechanism, mechanism};

/// The request descriptor, kept after reading it (marked close-on-exec with the rest): one of
/// the launcher's own failures is also written back on it, so the parent can tell the
/// launcher's exit 78 from the program's (a socketpair; on a plain pipe the write fails and is
/// ignored). End of file with nothing written means the program was started.
static REPORT: Mutex<Option<File>> = Mutex::new(None);

fn fail(exit: i32, code: &str, profile: Option<Profile>, need: &str) -> ! {
    let line = launch::failure_line(code, profile, need);
    eprintln!("{line}");
    if let Ok(mut held) = REPORT.lock()
        && let Some(f) = held.as_mut()
    {
        let _ = writeln!(f, "{line}");
    }
    std::process::exit(exit)
}

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let args = match launch::parse_command(&args) {
        Ok(launch::Command::Launch(a)) => a,
        Ok(launch::Command::Probe) => {
            let probe = fin_sandbox::LauncherProbe {
                namespaces: os::namespaces(),
            };
            println!("{}", serde_json::to_string(&probe).expect("serialisable"));
            std::process::exit(0)
        }
        Err(e) => fail(EXIT_USAGE, "USAGE", None, &e),
    };
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
    // Before the restriction (Landlock does not reach descriptors opened before it), in every
    // mode: the program inherits its standard streams and nothing else.
    if let Err(e) = os::cloexec_from_3() {
        fail(EXIT_UNAVAILABLE, "SANDBOX_UNAVAILABLE", profile, &e);
    }
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
            // Unix sockets hidden first (Decision 26): the mounts need the capabilities the
            // namespace gives, which Landlock then keeps the program from using.
            if let Err(e) = os::hide_sockets(&policy, req.home()) {
                fail(EXIT_UNAVAILABLE, "SANDBOX_UNAVAILABLE", profile, &e);
            }
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
        &format!("{}: {e}", args.argv[0].to_string_lossy()),
    )
}

/// Reads the request from the inherited descriptor to its end; the descriptor is kept for
/// [`REPORT`] and closes at `exec` (close-on-exec) or exit.
fn read_fd(fd: i32) -> Result<Vec<u8>, String> {
    let mut file = os::take_fd(fd)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_REQUEST + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("descriptor {fd}: {e}"))?;
    if let Ok(mut held) = REPORT.lock() {
        *held = Some(file);
    }
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

    #[cfg(target_os = "linux")]
    pub fn namespaces() -> fin_sandbox::Namespaces {
        fin_sandbox::linux::namespace::probe()
    }

    #[cfg(not(target_os = "linux"))]
    pub fn namespaces() -> fin_sandbox::Namespaces {
        fin_sandbox::Namespaces::NotApplicable
    }

    /// The namespace of Decision 26 where the probe shows this host can make it; where the
    /// host refuses it, nothing (Landlock alone, as `SANDBOX_SOCKETS_EXPOSED` says). Any other
    /// probe failure, and a failure after a successful probe, is an error.
    #[cfg(target_os = "linux")]
    pub fn hide_sockets(policy: &SandboxPolicy, home: &std::path::Path) -> Result<(), String> {
        use fin_sandbox::linux::namespace::{self, Outcome};
        match namespace::probe_outcome() {
            Outcome::Available => {}
            Outcome::NotPermitted(_) => return Ok(()),
            Outcome::Failed(e) => return Err(format!("the socket namespace probe: {e}")),
        }
        let xdg = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
        let layout = namespace::plan(policy, Some(home), xdg.as_deref());
        namespace::enter(&layout).map_err(|e| format!("the socket namespace: {e}"))
    }

    #[cfg(not(target_os = "linux"))]
    pub fn hide_sockets(_policy: &SandboxPolicy, _home: &std::path::Path) -> Result<(), String> {
        Ok(())
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

    /// Marks every descriptor from 3 up close-on-exec: `close_range(CLOSE_RANGE_CLOEXEC)` on
    /// Linux, and where that is refused (an older kernel, a container's seccomp profile) or off
    /// Linux, `fcntl` on each number below the soft `RLIMIT_NOFILE`. Any failure is an error.
    pub fn cloexec_from_3() -> Result<(), String> {
        #[cfg(target_os = "linux")]
        // SAFETY: `close_range` with `CLOSE_RANGE_CLOEXEC` only sets flags on descriptors.
        if unsafe {
            libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        } == 0
        {
            return Ok(());
        }
        // SAFETY: plain data, written by getrlimit before use.
        let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) } != 0 {
            return Err(format!("getrlimit: {}", std::io::Error::last_os_error()));
        }
        let top = i32::try_from(r.rlim_cur.min(1 << 20)).unwrap_or(1 << 20);
        for fd in 3..top {
            // SAFETY: `fcntl` only queries and sets descriptor flags.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags < 0 {
                continue; // not open
            }
            // SAFETY: as above.
            if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
                return Err(format!(
                    "descriptor {fd}: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(())
    }

    /// Replaces the process with `argv`, without [`launch::SOCKET_ENV`]. Returns only on
    /// failure.
    pub fn exec(argv: &[std::ffi::OsString]) -> std::io::Error {
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        for v in fin_sandbox::launch::SOCKET_ENV {
            cmd.env_remove(v);
        }
        cmd.exec()
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

    pub fn cloexec_from_3() -> Result<(), String> {
        Ok(())
    }

    pub fn namespaces() -> fin_sandbox::Namespaces {
        fin_sandbox::Namespaces::NotApplicable
    }

    pub fn hide_sockets(_policy: &SandboxPolicy, _home: &std::path::Path) -> Result<(), String> {
        Ok(())
    }

    pub fn exec(_argv: &[std::ffi::OsString]) -> std::io::Error {
        std::io::Error::other("the Windows launcher (plan 7C Task 9)")
    }
}
