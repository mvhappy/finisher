//! `fin-sandbox-probe` (test-hooks builds only): applies a sandbox to itself, performs one
//! named attempt and prints `{"op": …, "errno": n | null}` on stdout. The escape suites pass
//! a case only on an observed denial (plan 7C Global Constraints).
//!
//! ```text
//! fin-sandbox-probe [--abi <n>] [--thread] (--policy <file> | --self <file> | --none) <op> <args…>
//! ```
//! - `--policy`: a `SandboxPolicy` (JSON) applied with Landlock at `--abi` (default: the
//!   kernel's); `--self`: a `SelfSandbox` request (JSON) given to `fin_sandbox::self_apply`, as
//!   `fin-workers` does; `--none`: no sandbox (controls).
//! - `--thread`: a second thread exists before the sandbox is applied, and the attempt runs on
//!   it (below ABI 8 the apply must refuse; from ABI 8 the thread must be restricted).
//! - ops: `read <file>`, `list <dir>`, `write <file>`, `create <file>`, `truncate <file>`,
//!   `unlink <file>`, `rmdir <dir>`, `mkdir <dir>`, `mkfifo <path>`, `link <from> <to>`,
//!   `rename <from> <to>`, `symlink <target> <link>`, `exec <file>`, `connect-tcp <port>`,
//!   `bind-tcp <port>`, `listen-tcp` (`listen` on an unbound socket), `bind-unix <path>`,
//!   `connect-unix <path>`, `send-udp <port>` (one datagram to loopback), `signal-parent`,
//!   `open-tree <dir> <rel>` (a clone of the mounts at `dir`, then `connect` to `rel` in it),
//!   `unshare-user` (a nested user namespace).
//!
//! Exit 0 when the attempt ran (whatever its result), 3 when the sandbox could not be applied
//! (`{"apply": "<error>"}`), 2 for a usage error.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "fin-sandbox-probe: Linux only (Task 2); macOS and Windows probes come with their tasks"
    );
    std::process::exit(2)
}

#[cfg(target_os = "linux")]
fn main() {
    probe::main()
}

#[cfg(target_os = "linux")]
mod probe {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use fin_sandbox::SandboxPolicy;
    use fin_sandbox::linux::apply::{restrict_self, restrict_self_at};
    use fin_sandbox::linux::landlock::Abi;

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
                    let n = args.get(1).and_then(|n| n.parse().ok());
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
            Sandbox::Policy(p) => match abi {
                Some(a) => restrict_self_at(&p, a)
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                None => restrict_self(&p).map(|_| ()).map_err(|e| e.to_string()),
            },
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
        // SAFETY: a socket we own and a fully initialised address.
        unsafe {
            let s = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
            rc(s)?;
            let mut a: libc::sockaddr_in = std::mem::zeroed();
            a.sin_family = libc::AF_INET as libc::sa_family_t;
            a.sin_port = port.to_be();
            a.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
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
            let s = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
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
            let s = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
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
            let s = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
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
                "bind-tcp" => tcp(port(1), true),
                "listen-tcp" => listen_tcp(),
                "bind-unix" => unix(a(1), true),
                "connect-unix" => unix(a(1), false),
                "send-udp" => udp(port(1)),
                "open-tree" => open_tree(a(1), a(2)),
                "unshare-user" => rc(libc::unshare(libc::CLONE_NEWUSER)),
                "signal-parent" => rc(libc::kill(libc::getppid(), 0)),
                _ => usage(&format!("unknown op {op:?}")),
            }
        };
        let errno = r.err();
        serde_json::json!({ "op": op, "errno": errno }).to_string()
    }
}
