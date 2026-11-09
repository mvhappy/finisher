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
    let enforced = match mechanism() {
        Mechanism::Disabled {
            reason: DisabledReason::LandlockDisabled,
        } => false,
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
            true
        }
    };
    let e = os::exec(&req, &policy, enforced, &args.argv);
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

/// Writes a `SANDBOX_VIOLATION` line on the request channel, which only the launcher holds (the
/// program shares stderr and could forge lines there). Where the channel is one-way (a plain
/// pipe, as in the suites) the line goes to stderr instead, which a parent must not trust.
#[cfg(windows)]
fn violation(line: &str) {
    let sent = REPORT
        .lock()
        .ok()
        .and_then(|mut held| held.as_mut().map(|f| writeln!(f, "{line}").is_ok()))
        .unwrap_or(false);
    if !sent {
        eprintln!("{line}");
    }
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

    /// Replaces the process with `argv`, without [`launch::SOCKET_ENV`] (the policy is
    /// already applied to this process). Returns only on failure.
    pub fn exec(
        _req: &fin_sandbox::launch::LaunchRequest,
        _policy: &SandboxPolicy,
        _enforced: bool,
        argv: &[std::ffi::OsString],
    ) -> std::io::Error {
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        for v in fin_sandbox::launch::SOCKET_ENV {
            cmd.env_remove(v);
        }
        cmd.exec()
    }
}

#[cfg(windows)]
mod os {
    use std::fs::File;
    use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};

    use fin_sandbox::SandboxPolicy;
    use fin_sandbox::launch::{EXIT_UNAVAILABLE, LaunchRequest, SOCKET_ENV, failure_line};
    use fin_sandbox::windows::launcher::{self, Failure};
    use fin_sandbox::windows::plan;
    use windows_sys::Win32::Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
        SetHandleInformation,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    type InvalidParameter =
        Option<unsafe extern "C" fn(*const u16, *const u16, *const u16, u32, usize)>;

    unsafe extern "C" {
        fn _get_osfhandle(fd: i32) -> isize;
        fn _close(fd: i32) -> i32;
        fn _set_thread_local_invalid_parameter_handler(h: InvalidParameter) -> InvalidParameter;
    }

    unsafe extern "C" fn ignore(_: *const u16, _: *const u16, _: *const u16, _: u32, _: usize) {}

    /// `_get_osfhandle(fd)`, or -1: a number that is not a C runtime descriptor (a handle
    /// value) would otherwise reach the runtime's invalid-parameter handler, which ends the
    /// process with a fast-fail.
    fn crt_handle(fd: i32) -> isize {
        // SAFETY: the handler is this thread's, replaced for the one lookup and restored.
        unsafe {
            let old = _set_thread_local_invalid_parameter_handler(Some(ignore));
            let h = _get_osfhandle(fd);
            _set_thread_local_invalid_parameter_handler(old);
            h
        }
    }

    /// The request's pipe: C runtime descriptor `fd` (how Node passes `stdio[3]`), else an
    /// inherited handle whose value is `fd`. Taken over, so nothing else can read it.
    pub fn take_fd(fd: i32) -> Result<File, String> {
        let crt = crt_handle(fd);
        if crt != -1 && crt != -2 {
            let mut dup: HANDLE = std::ptr::null_mut();
            // SAFETY: a handle of this process, duplicated without inheritance, then the
            // descriptor (and its handle) closed.
            unsafe {
                let me = GetCurrentProcess();
                if DuplicateHandle(me, crt as HANDLE, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS)
                    == 0
                {
                    return Err(format!(
                        "descriptor {fd}: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                _close(fd);
                return Ok(File::from(OwnedHandle::from_raw_handle(dup as RawHandle)));
            }
        }
        let h = fd as usize as HANDLE;
        let mut flags = 0u32;
        // SAFETY: only queries the handle's flags.
        if unsafe { GetHandleInformation(h, &mut flags) } == 0 {
            return Err(format!(
                "descriptor {fd}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // Not inheritable: nothing the launcher starts may receive it (review B3).
        // SAFETY: only the handle's inherit flag changes.
        if unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(format!(
                "descriptor {fd}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: an open handle handed to this process to own.
        Ok(File::from(unsafe {
            OwnedHandle::from_raw_handle(h as RawHandle)
        }))
    }

    /// Nothing to apply in-process: the container is made at the start.
    pub fn apply(_policy: &SandboxPolicy) -> Result<(), String> {
        Ok(())
    }

    /// The handle list of the start passes the standard handles alone.
    pub fn cloexec_from_3() -> Result<(), String> {
        Ok(())
    }

    pub fn namespaces() -> fin_sandbox::Namespaces {
        fin_sandbox::Namespaces::NotApplicable
    }

    pub fn hide_sockets(_policy: &SandboxPolicy, _home: &std::path::Path) -> Result<(), String> {
        Ok(())
    }

    /// Runs `argv` in its container (`enforced`), or unconfined (the F2 stub), and leaves with
    /// its exit code. Returns only when the program cannot be started.
    pub fn exec(
        req: &LaunchRequest,
        policy: &SandboxPolicy,
        enforced: bool,
        argv: &[std::ffi::OsString],
    ) -> std::io::Error {
        let profile = Some(req.profile());
        if !enforced {
            let mut cmd = std::process::Command::new(&argv[0]);
            cmd.args(&argv[1..]);
            for v in SOCKET_ENV {
                cmd.env_remove(v);
            }
            return match cmd.status() {
                Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                Err(e) => e,
            };
        }
        let unavailable = |need: String| -> ! {
            eprintln!("{}", failure_line("SANDBOX_UNAVAILABLE", profile, &need));
            std::process::exit(EXIT_UNAVAILABLE)
        };
        let plan = plan::launch(req, policy).unwrap_or_else(|e| unavailable(e));
        match launcher::run(req.profile(), policy, &plan, argv, &mut super::violation) {
            Ok(code) => std::process::exit(code),
            Err(Failure::Setup(need)) => unavailable(need),
            Err(Failure::Start(e)) => e,
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    use fin_sandbox::SandboxPolicy;

    pub fn take_fd(fd: i32) -> Result<std::fs::File, String> {
        Err(format!("descriptor {fd}: no launcher on this OS"))
    }

    pub fn apply(_policy: &SandboxPolicy) -> Result<(), String> {
        Err("no launcher sandbox on this OS".into())
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

    pub fn exec(
        _req: &fin_sandbox::launch::LaunchRequest,
        _policy: &SandboxPolicy,
        _enforced: bool,
        _argv: &[std::ffi::OsString],
    ) -> std::io::Error {
        std::io::Error::other("no launcher sandbox on this OS")
    }
}
