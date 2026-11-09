//! `fin-sandbox-probe` (test-hooks builds only): applies a sandbox to itself, performs one
//! named attempt and prints `{"op": …, "errno": n | null}` on stdout. The escape suites pass
//! a case only on an observed denial (plan 7C Global Constraints).
//!
//! ```text
//! fin-sandbox-probe [--abi <n>] [--thread] (--policy <file> | --self <file> | --none) <op> <args…>
//! ```
//! - `--policy`: a `SandboxPolicy` (JSON) applied with Landlock at `--abi` (default: the
//!   kernel's; Linux) or with Seatbelt (macOS); `--self`: a `SelfSandbox` request (JSON) given
//!   to `fin_sandbox::self_apply`, as `fin-workers` does; `--none`: no sandbox (controls).
//! - `--thread`: a second thread exists before the sandbox is applied, and the attempt runs on
//!   it (below ABI 8 the apply must refuse; from ABI 8 the thread must be restricted).
//! - ops: `read <file>`, `list <dir>`, `write <file>`, `create <file>`, `truncate <file>`,
//!   `unlink <file>`, `rmdir <dir>`, `mkdir <dir>`, `mkfifo <path>`, `link <from> <to>`,
//!   `rename <from> <to>`, `symlink <target> <link>`, `exec <file>`, `connect-tcp <port>`,
//!   `connect-ip <ipv4> <port>`, `bind-tcp <port>`, `listen-tcp` (`listen` on an unbound socket), `bind-unix <path>`,
//!   `connect-unix <path>`, `send-udp <port>` (one datagram to loopback), `signal-parent`,
//!   `open-tree <dir> <rel>` (a clone of the mounts at `dir`, then `connect` to `rel` in it),
//!   `unshare-user` (a nested user namespace), `fork` (a child that exits at once); macOS only:
//!   `mach-lookup <service>` (`bootstrap_look_up`; a refusal is reported as `EPERM`),
//!   `shm-open <name>` (read-only), `sysctl <name>`, `system-socket` (`PF_SYSTEM`),
//!   `signal-self`.
//!
//! Exit 0 when the attempt ran (whatever its result), 3 when the sandbox could not be applied
//! (`{"apply": "<error>"}`), 2 for a usage error.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() {
    eprintln!("fin-sandbox-probe: Linux and macOS only; the Windows probe comes with Task 9");
    std::process::exit(2)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() {
    probe::main()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod probe {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use fin_sandbox::SandboxPolicy;

    /// A Landlock ABI (`--abi`, Linux).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    #[derive(Clone, Copy)]
    struct Abi(u32);

    /// The policy applied to this process (`--policy`).
    #[cfg(target_os = "linux")]
    fn apply(p: &SandboxPolicy, abi: Option<Abi>) -> Result<(), String> {
        use fin_sandbox::linux::apply::{restrict_self, restrict_self_at};
        use fin_sandbox::linux::landlock::Abi as LlAbi;
        match abi {
            Some(a) => restrict_self_at(p, LlAbi(a.0)).map(|_| ()),
            None => restrict_self(p).map(|_| ()),
        }
        .map_err(|e| e.to_string())
    }

    #[cfg(target_os = "macos")]
    fn apply(p: &SandboxPolicy, abi: Option<Abi>) -> Result<(), String> {
        if abi.is_some() {
            usage("--abi is Linux only");
        }
        fin_sandbox::macos::apply::restrict_self(p, None).map_err(|e| e.to_string())
    }

    /// `socket(2)` with close-on-exec.
    fn socket(family: libc::c_int, kind: libc::c_int) -> libc::c_int {
        // SAFETY: plain syscalls; the descriptor is ours.
        unsafe {
            #[cfg(target_os = "linux")]
            return libc::socket(family, kind | libc::SOCK_CLOEXEC, 0);
            #[cfg(target_os = "macos")]
            {
                let s = libc::socket(family, kind, 0);
                if s >= 0 {
                    libc::fcntl(s, libc::F_SETFD, libc::FD_CLOEXEC);
                }
                s
            }
        }
    }

    fn usage(msg: &str) -> ! {
        eprintln!("fin-sandbox-probe: {msg}");
        std::process::exit(2)
    }

    enum Sandbox {
        Policy(Box<SandboxPolicy>),
        SelfReq(fin_jobs::sandbox::SelfSandbox),
        None,
    }

    fn read_json<T: serde::de::DeserializeOwned>(path: &str) -> T {
        let text = std::fs::read(path).unwrap_or_else(|e| usage(&format!("{path}: {e}")));
        serde_json::from_slice(&text).unwrap_or_else(|e| usage(&format!("{path}: {e}")))
    }

    pub fn main() {
        let mut args: Vec<String> = std::env::args().skip(1).collect();
        let mut abi = None;
        let mut thread = false;
        let mut sandbox = None;
        while let Some(flag) = args.first().cloned() {
            match flag.as_str() {
                "--abi" => {
                    let n = args.get(1).and_then(|n| n.parse::<u32>().ok());
                    abi = Some(Abi(n.unwrap_or_else(|| usage("--abi needs a number"))));
                    args.drain(..2);
                }
                "--thread" => {
                    thread = true;
                    args.remove(0);
                }
                "--policy" | "--self" => {
                    let file = args.get(1).cloned().unwrap_or_else(|| usage("a file"));
                    sandbox = Some(if flag == "--policy" {
                        Sandbox::Policy(Box::new(read_json(&file)))
                    } else {
                        Sandbox::SelfReq(read_json(&file))
                    });
                    args.drain(..2);
                }
                "--none" => {
                    sandbox = Some(Sandbox::None);
                    args.remove(0);
                }
                _ => break,
            }
        }
        let sandbox = sandbox.unwrap_or_else(|| usage("--policy, --self or --none"));
        if args.is_empty() {
            usage("no op");
        }
        let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<String>();
        let worker = thread.then(|| {
            std::thread::spawn(move || {
                if let Ok(args) = rx.recv() {
                    let _ = done_tx.send(attempt(&args));
                }
            })
        });
        let applied = match sandbox {
            Sandbox::Policy(p) => apply(&p, abi),
            Sandbox::SelfReq(r) => fin_sandbox::self_apply(&r).map(|_| ()),
            Sandbox::None => Ok(()),
        };
        if let Err(e) = applied {
            println!("{}", serde_json::json!({ "apply": e }));
            std::process::exit(3);
        }
        let line = match worker {
            Some(w) => {
                tx.send(args)
                    .unwrap_or_else(|_| usage("the thread is gone"));
                let line = done_rx
                    .recv()
                    .unwrap_or_else(|_| usage("the thread is gone"));
                let _ = w.join();
                line
            }
            None => attempt(&args),
        };
        println!("{line}");
    }

    fn c(p: &str) -> CString {
        CString::new(Path::new(p).as_os_str().as_bytes()).unwrap_or_else(|_| usage("a NUL"))
    }

    /// `Ok(())` or the errno of the attempt.
    fn rc(r: libc::c_int) -> Result<(), i32> {
        if r < 0 {
            Err(io::Error::last_os_error().raw_os_error().unwrap_or(0))
        } else {
            Ok(())
        }
    }

    fn open_close(path: &str, flags: libc::c_int) -> Result<(), i32> {
        // SAFETY: plain syscalls on a valid C string and our own descriptor.
        unsafe {
            let fd = libc::open(c(path).as_ptr(), flags | libc::O_CLOEXEC, 0o600);
            rc(fd)?;
            if flags & libc::O_DIRECTORY == 0 && flags & libc::O_ACCMODE == libc::O_RDONLY {
                let mut b = [0u8; 1];
                if libc::read(fd, b.as_mut_ptr().cast(), 1) < 0 {
                    let e = io::Error::last_os_error().raw_os_error().unwrap_or(0);
                    libc::close(fd);
                    return Err(e);
                }
            }
            libc::close(fd);
        }
        Ok(())
    }

    fn tcp(port: u16, bind: bool) -> Result<(), i32> {
        tcp_at(std::net::Ipv4Addr::LOCALHOST, port, bind)
    }

    fn tcp_at(ip: std::net::Ipv4Addr, port: u16, bind: bool) -> Result<(), i32> {
        // SAFETY: a socket we own and a fully initialised address.
        unsafe {
            let s = socket(libc::AF_INET, libc::SOCK_STREAM);
            rc(s)?;
            let mut a: libc::sockaddr_in = std::mem::zeroed();
            a.sin_family = libc::AF_INET as libc::sa_family_t;
            a.sin_port = port.to_be();
            a.sin_addr.s_addr = u32::from(ip).to_be();
            let len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
            let addr = (&raw const a).cast::<libc::sockaddr>();
            let r = if bind {
                libc::bind(s, addr, len)
            } else {
                libc::connect(s, addr, len)
            };
            let out = rc(r);
            libc::close(s);
            out
        }
    }

    /// `listen` on an unbound TCP socket (the kernel binds an ephemeral port itself).
    fn listen_tcp() -> Result<(), i32> {
        // SAFETY: a socket we own.
        unsafe {
            let s = socket(libc::AF_INET, libc::SOCK_STREAM);
            rc(s)?;
            let out = rc(libc::listen(s, 1));
            libc::close(s);
            out
        }
    }

    /// One datagram to `127.0.0.1:<port>`.
    fn udp(port: u16) -> Result<(), i32> {
        // SAFETY: a socket we own, a fully initialised address and a one-byte buffer.
        unsafe {
            let s = socket(libc::AF_INET, libc::SOCK_DGRAM);
            rc(s)?;
            let mut a: libc::sockaddr_in = std::mem::zeroed();
            a.sin_family = libc::AF_INET as libc::sa_family_t;
            a.sin_port = port.to_be();
            a.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
            let len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
            let b = *b"x";
            let n = libc::sendto(s, b.as_ptr().cast(), 1, 0, (&raw const a).cast(), len);
            let out = rc(if n < 0 { -1 } else { 0 });
            libc::close(s);
            out
        }
    }

    #[cfg(target_os = "linux")]
    /// `open_tree(<dir>, OPEN_TREE_CLONE)` (a copy of the mounts below `dir`, as they are
    /// beneath any mount on top), then `connect` to `<rel>` inside the copy.
    fn open_tree(dir: &str, rel: &str) -> Result<(), i32> {
        let d = c(dir);
        // SAFETY: a valid path; the descriptor is ours.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_open_tree,
                libc::AT_FDCWD,
                d.as_ptr(),
                0x1 | libc::O_CLOEXEC as libc::c_long, // OPEN_TREE_CLONE | OPEN_TREE_CLOEXEC
            )
        };
        rc(fd as i32)?;
        let out = unix(&format!("/proc/self/fd/{fd}/{rel}"), false);
        // SAFETY: our own descriptor.
        unsafe { libc::close(fd as i32) };
        out
    }

    /// `bind` (which creates the socket file) or `connect` on a pathname socket.
    fn unix(path: &str, bind: bool) -> Result<(), i32> {
        let bytes = Path::new(path).as_os_str().as_bytes();
        // SAFETY: a socket we own and a fully initialised address whose path fits.
        unsafe {
            let mut a: libc::sockaddr_un = std::mem::zeroed();
            if bytes.len() >= a.sun_path.len() {
                usage("the socket path is too long");
            }
            a.sun_family = libc::AF_UNIX as libc::sa_family_t;
            for (d, b) in a.sun_path.iter_mut().zip(bytes) {
                *d = *b as libc::c_char;
            }
            let s = socket(libc::AF_UNIX, libc::SOCK_STREAM);
            rc(s)?;
            let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
            let addr = (&raw const a).cast::<libc::sockaddr>();
            let r = if bind {
                libc::bind(s, addr, len)
            } else {
                libc::connect(s, addr, len)
            };
            let out = rc(r);
            libc::close(s);
            out
        }
    }

    /// A child that exits at once, waited for.
    fn fork() -> Result<(), i32> {
        // SAFETY: the child only calls `_exit`.
        unsafe {
            let pid = libc::fork();
            rc(pid)?;
            if pid == 0 {
                libc::_exit(0);
            }
            let mut st = 0;
            libc::waitpid(pid, &mut st, 0);
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        static bootstrap_port: libc::mach_port_t;
        fn bootstrap_look_up(
            bp: libc::mach_port_t,
            name: *const libc::c_char,
            sp: *mut libc::mach_port_t,
        ) -> libc::c_int;
    }

    /// `bootstrap_look_up(<service>)`: `EPERM` when the sandbox refuses it (the bootstrap
    /// server answers `BOOTSTRAP_NOT_PRIVILEGED`, 1100), `ENOENT` for any other failure
    /// (`BOOTSTRAP_UNKNOWN_SERVICE`, 1102, above all).
    #[cfg(target_os = "macos")]
    fn mach_lookup(name: &str) -> Result<(), i32> {
        let n = CString::new(name).unwrap_or_else(|_| usage("a NUL"));
        let mut port: libc::mach_port_t = 0;
        // SAFETY: a valid name and out pointer; `bootstrap_port` is set by libSystem.
        let kr = unsafe { bootstrap_look_up(bootstrap_port, n.as_ptr(), &mut port) };
        match kr {
            0 => Ok(()),
            1100 => Err(libc::EPERM),
            _ => Err(libc::ENOENT),
        }
    }

    fn attempt(args: &[String]) -> String {
        let op = args[0].as_str();
        let a = |i: usize| -> &str {
            args.get(i)
                .map(String::as_str)
                .unwrap_or_else(|| usage(&format!("{op} needs {i} argument(s)")))
        };
        let port = |i: usize| -> u16 { a(i).parse().unwrap_or_else(|_| usage("a port")) };
        // SAFETY: each arm is a plain syscall on valid C strings.
        let r = unsafe {
            match op {
                "read" => open_close(a(1), libc::O_RDONLY),
                "list" => open_close(a(1), libc::O_RDONLY | libc::O_DIRECTORY),
                "write" => open_close(a(1), libc::O_WRONLY),
                "create" => open_close(a(1), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL),
                "truncate" => rc(libc::truncate(c(a(1)).as_ptr(), 0)),
                "unlink" => rc(libc::unlink(c(a(1)).as_ptr())),
                "rmdir" => rc(libc::rmdir(c(a(1)).as_ptr())),
                "mkdir" => rc(libc::mkdir(c(a(1)).as_ptr(), 0o700)),
                "mkfifo" => rc(libc::mkfifo(c(a(1)).as_ptr(), 0o600)),
                "link" => rc(libc::link(c(a(1)).as_ptr(), c(a(2)).as_ptr())),
                "rename" => rc(libc::rename(c(a(1)).as_ptr(), c(a(2)).as_ptr())),
                "symlink" => rc(libc::symlink(c(a(1)).as_ptr(), c(a(2)).as_ptr())),
                "exec" => {
                    let p = c(a(1));
                    let argv = [p.as_ptr(), std::ptr::null()];
                    let envp = [std::ptr::null()];
                    rc(libc::execve(p.as_ptr(), argv.as_ptr(), envp.as_ptr()))
                }
                "connect-tcp" => tcp(port(1), false),
                "connect-ip" => {
                    let ip = a(1).parse().unwrap_or_else(|_| usage("an IPv4 address"));
                    tcp_at(ip, port(2), false)
                }
                "bind-tcp" => tcp(port(1), true),
                "listen-tcp" => listen_tcp(),
                "bind-unix" => unix(a(1), true),
                "connect-unix" => unix(a(1), false),
                "send-udp" => udp(port(1)),
                #[cfg(target_os = "linux")]
                "open-tree" => open_tree(a(1), a(2)),
                #[cfg(target_os = "linux")]
                "unshare-user" => rc(libc::unshare(libc::CLONE_NEWUSER)),
                "fork" => fork(),
                #[cfg(target_os = "macos")]
                "mach-lookup" => mach_lookup(a(1)),
                #[cfg(target_os = "macos")]
                "shm-open" => {
                    let fd = libc::shm_open(c(a(1)).as_ptr(), libc::O_RDONLY);
                    rc(fd).map(|()| {
                        libc::close(fd);
                    })
                }
                #[cfg(target_os = "macos")]
                "sysctl" => {
                    let mut v = [0u8; 64];
                    let mut n = v.len();
                    let name = c(a(1));
                    rc(libc::sysctlbyname(
                        name.as_ptr(),
                        v.as_mut_ptr().cast(),
                        &mut n,
                        std::ptr::null_mut(),
                        0,
                    ))
                }
                #[cfg(target_os = "macos")]
                "system-socket" => {
                    let s = libc::socket(libc::PF_SYSTEM, libc::SOCK_DGRAM, libc::SYSPROTO_CONTROL);
                    rc(s).map(|()| {
                        libc::close(s);
                    })
                }
                "signal-self" => rc(libc::kill(libc::getpid(), 0)),
                "signal-parent" => rc(libc::kill(libc::getppid(), 0)),
                _ => usage(&format!("unknown op {op:?}")),
            }
        };
        let errno = r.err();
        serde_json::json!({ "op": op, "errno": errno }).to_string()
    }
}
