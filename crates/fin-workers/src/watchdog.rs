//! `fin-workers watchdog <host pid>` (Unix, 5C-1 Decision 15): kills the host's process
//! groups and cgroups when the host dies, so a grandchild or a worker that never reads its
//! stdin cannot outlive it.
//!
//! The host writes `+g <pgid>` / `-g <pgid>` and `+c <cgroup dir>` / `-c <cgroup dir>` lines
//! to stdin. When the host exits (Linux `pidfd_open` + `poll`, macOS `kqueue`
//! `EVFILT_PROC`/`NOTE_EXIT`, and a `getppid` check every second either way) or stdin reaches
//! EOF, the watchdog first reads what is left on stdin (the dead host wrote it before dying,
//! so a late `-g` still counts), then sends `SIGKILL` to every listed group, writes `1` to
//! every listed `cgroup.kill`, and exits 0. It ignores `SIGINT`, `SIGTERM`, `SIGHUP` and
//! `SIGPIPE` (then writes `ready` on stdout, which the host waits for), serves no job and
//! starts no process.

use std::collections::BTreeSet;
use std::io;

#[derive(Default)]
struct List {
    groups: BTreeSet<i32>,
    cgroups: BTreeSet<String>,
    partial: Vec<u8>,
}

impl List {
    fn feed(&mut self, bytes: &[u8]) {
        self.partial.extend_from_slice(bytes);
        while let Some(i) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=i).collect();
            self.line(String::from_utf8_lossy(&line[..i]).trim_end());
        }
    }

    fn line(&mut self, line: &str) {
        let (g, c) = (&mut self.groups, &mut self.cgroups);
        match line.split_once(' ') {
            Some(("+g", a)) => _ = a.parse().map(|p| g.insert(p)),
            Some(("-g", a)) => _ = a.parse().map(|p: i32| g.remove(&p)),
            Some(("+c", a)) => _ = c.insert(a.to_owned()),
            Some(("-c", a)) => _ = c.remove(a),
            _ => {}
        }
    }

    fn kill_all(&self) {
        for &pgid in self.groups.iter().filter(|&&g| g > 1) {
            // SAFETY: signals a process group the host listed; no memory is touched.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
        for dir in &self.cgroups {
            let _ = std::fs::write(format!("{dir}/cgroup.kill"), b"1");
        }
    }
}

/// What woke the watchdog.
enum Wake {
    Input,
    HostGone,
    Timeout,
}

/// Reads what stdin holds into `list`; `false` at EOF or on an error.
fn read_some(list: &mut List) -> bool {
    let mut buf = [0u8; 4096];
    loop {
        // SAFETY: reads into a local buffer.
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if n > 0 {
            list.feed(&buf[..n as usize]);
            return true;
        }
        if n == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
}

/// Reads stdin until EOF or until it is empty (non-blocking).
fn drain(list: &mut List) {
    // SAFETY: sets a flag on our own stdin.
    unsafe { libc::fcntl(0, libc::F_SETFL, libc::O_NONBLOCK) };
    while read_some(list) {}
}

#[cfg(target_os = "linux")]
struct Waiter {
    pidfd: Option<i32>,
}

#[cfg(target_os = "linux")]
impl Waiter {
    fn new(host: i32) -> Waiter {
        // SAFETY: pidfd_open has no memory arguments; a failure (ENOSYS) leaves the getppid
        // check.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, host, 0) } as i32;
        Waiter {
            pidfd: (fd >= 0).then_some(fd),
        }
    }

    fn wait(&self) -> Wake {
        let mut fds = [
            libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.pidfd.unwrap_or(-1),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: polls two local pollfds.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, 1000) };
        if r > 0 && fds[1].revents != 0 {
            Wake::HostGone
        } else if r > 0 && fds[0].revents != 0 {
            Wake::Input
        } else {
            Wake::Timeout
        }
    }
}

#[cfg(target_os = "macos")]
struct Waiter {
    kq: i32,
}

#[cfg(target_os = "macos")]
impl Waiter {
    fn new(host: i32) -> Waiter {
        // SAFETY: kqueue and kevent on local structures; a failed registration (the host is
        // already gone) leaves the getppid check.
        unsafe {
            let kq = libc::kqueue();
            let mut ev: [libc::kevent; 2] = std::mem::zeroed();
            ev[0].ident = host as usize;
            ev[0].filter = libc::EVFILT_PROC;
            ev[0].flags = libc::EV_ADD;
            ev[0].fflags = libc::NOTE_EXIT;
            ev[1].ident = 0;
            ev[1].filter = libc::EVFILT_READ;
            ev[1].flags = libc::EV_ADD;
            for e in &ev {
                libc::kevent(kq, e, 1, std::ptr::null_mut(), 0, std::ptr::null());
            }
            Waiter { kq }
        }
    }

    fn wait(&self) -> Wake {
        let ts = libc::timespec {
            tv_sec: 1,
            tv_nsec: 0,
        };
        // SAFETY: one out-event into a local buffer.
        let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
        let n = unsafe { libc::kevent(self.kq, std::ptr::null(), 0, &mut ev, 1, &ts) };
        match (n, ev.filter) {
            (1, libc::EVFILT_PROC) => Wake::HostGone,
            (1, _) => Wake::Input,
            _ => Wake::Timeout,
        }
    }
}

/// The watchdog's `main`; returns the exit code.
pub fn run(args: &[String]) -> i32 {
    let Some(host) = args.first().and_then(|a| a.parse::<i32>().ok()) else {
        eprintln!("fin-workers: usage: fin-workers watchdog <host pid>");
        return 2;
    };
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGPIPE] {
        // SAFETY: sets a signal to be ignored.
        unsafe { libc::signal(sig, libc::SIG_IGN) };
    }
    let waiter = Waiter::new(host);
    // The host waits for this line: no signal reaches us before the dispositions are set.
    // SAFETY: writes a static buffer to stdout (a pipe the host reads once).
    unsafe { libc::write(1, b"ready\n".as_ptr().cast(), 6) };
    let mut list = List::default();
    // SAFETY: getppid has no preconditions.
    while unsafe { libc::getppid() } == host {
        match waiter.wait() {
            Wake::Input if !read_some(&mut list) => break,
            Wake::HostGone => break,
            Wake::Input | Wake::Timeout => {}
        }
    }
    drain(&mut list);
    list.kill_all();
    0
}
