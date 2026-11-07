//! The host's side of `fin-workers watchdog` (Decision 15). The watchdog is started at
//! `Host::start` in its own process group, with only stdin and stdout (pipes) and stderr, and
//! the start waits for its `ready` line (sent once it ignores `SIGINT`, `SIGTERM`, `SIGHUP`
//! and `SIGPIPE`). It is not a worker and never reaches the event sink. This side keeps the list of
//! groups (`+g`/`-g`) and cgroups (`+c`/`-c`) and writes each change as it happens, so a `-g`
//! is on the pipe before the host reaps that group's leader.
//!
//! A monitor thread reaps the watchdog. If it dies, the host logs it, starts a new one (the
//! first restart at once, later ones at most once a minute) and re-sends the whole list.
//! Dropping the [`Watchdog`] closes its stdin, and it exits after killing what is still listed.

use std::collections::BTreeSet;
use std::io::{self, PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use crate::LogFn;

const RESTART_EVERY: Duration = Duration::from_secs(60);

#[derive(Default)]
struct State {
    pid: Option<u32>,
    stdin: Option<PipeWriter>,
    groups: BTreeSet<i32>,
    cgroups: BTreeSet<PathBuf>,
    stopping: bool,
}

struct Shared {
    program: PathBuf,
    log: LogFn,
    state: Mutex<State>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub(crate) struct Watchdog {
    shared: Arc<Shared>,
    /// Answers once the monitor reaped the last watchdog (after the drop closed its stdin).
    gone: Mutex<mpsc::Receiver<()>>,
}

fn write_line(st: &mut State, line: &str) {
    if let Some(w) = &mut st.stdin
        && w.write_all(line.as_bytes()).is_err()
    {
        // The watchdog is gone; the monitor restarts it and re-sends the list.
        st.stdin = None;
    }
}

impl Watchdog {
    /// Starts `<binary_dir>/fin-workers watchdog <our pid>`.
    pub fn start(binary_dir: &Path, log: LogFn) -> io::Result<Watchdog> {
        let program = binary_dir.join("fin-workers");
        let (pid, stdin) = spawn(&program)?;
        let shared = Arc::new(Shared {
            program,
            log,
            state: Mutex::new(State {
                pid: Some(pid),
                stdin: Some(stdin),
                ..State::default()
            }),
        });
        let (tx, rx) = mpsc::channel();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("jobhost-watchdog".into())
            .spawn(move || {
                monitor(&s, pid);
                let _ = tx.send(());
            })?;
        Ok(Watchdog {
            shared,
            gone: Mutex::new(rx),
        })
    }

    pub fn add_group(&self, pgid: i32) {
        let mut st = self.shared.lock();
        if st.groups.insert(pgid) {
            write_line(&mut st, &format!("+g {pgid}\n"));
        }
    }

    pub fn remove_group(&self, pgid: i32) {
        let mut st = self.shared.lock();
        if st.groups.remove(&pgid) {
            write_line(&mut st, &format!("-g {pgid}\n"));
        }
    }

    #[cfg(target_os = "linux")]
    pub fn add_cgroup(&self, dir: &Path) {
        let mut st = self.shared.lock();
        if st.cgroups.insert(dir.to_path_buf()) {
            write_line(&mut st, &format!("+c {}\n", dir.display()));
        }
    }

    #[cfg(target_os = "linux")]
    pub fn remove_cgroup(&self, dir: &Path) {
        let mut st = self.shared.lock();
        if st.cgroups.remove(dir) {
            write_line(&mut st, &format!("-c {}\n", dir.display()));
        }
    }

    #[cfg(feature = "test-hooks")]
    pub fn pid(&self) -> Option<u32> {
        self.shared.lock().pid
    }

    #[cfg(feature = "test-hooks")]
    pub fn raw_line(&self, line: &str) {
        write_line(&mut self.shared.lock(), line);
    }

    #[cfg(feature = "test-hooks")]
    pub fn groups(&self) -> Vec<i32> {
        self.shared.lock().groups.iter().copied().collect()
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        {
            let mut st = self.shared.lock();
            st.stopping = true;
            st.stdin = None;
        }
        // It kills what is still listed and exits; wait a little so that happens before the
        // host reaps its leaders.
        let rx = self.gone.lock().unwrap_or_else(|e| e.into_inner());
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }
}

/// Reaps the watchdog and restarts it until the host stops.
fn monitor(shared: &Shared, mut pid: u32) {
    let mut last_restart: Option<Instant> = None;
    loop {
        let raw = super::wait(&(), pid, true);
        {
            let mut st = shared.lock();
            st.pid = None;
            st.stdin = None;
            if st.stopping {
                return;
            }
        }
        (shared.log)(&format!(
            "the watchdog (pid {pid}) exited ({raw:?}); restarting it"
        ));
        loop {
            if let Some(t) = last_restart {
                while Instant::now() < t + RESTART_EVERY {
                    if shared.lock().stopping {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
            last_restart = Some(Instant::now());
            let mut st = shared.lock();
            if st.stopping {
                return;
            }
            match spawn(&shared.program) {
                Ok((p, w)) => {
                    st.stdin = Some(w);
                    st.pid = Some(p);
                    let groups: Vec<i32> = st.groups.iter().copied().collect();
                    let cgroups: Vec<PathBuf> = st.cgroups.iter().cloned().collect();
                    for g in groups {
                        write_line(&mut st, &format!("+g {g}\n"));
                    }
                    for c in cgroups {
                        write_line(&mut st, &format!("+c {}\n", c.display()));
                    }
                    pid = p;
                    break;
                }
                Err(e) => {
                    drop(st);
                    (shared.log)(&format!("the watchdog did not restart: {e}"));
                }
            }
        }
    }
}

/// Starts the watchdog and waits for its `ready` line.
fn spawn(program: &Path) -> io::Result<(u32, PipeWriter)> {
    let (stdin_r, stdin_w) = io::pipe()?;
    let (mut ready_r, ready_w) = io::pipe()?;
    let pid = spawn_os(program, stdin_r, ready_w)?;
    let mut buf = [0u8; 6];
    if let Err(e) = io::Read::read_exact(&mut ready_r, &mut buf).and_then(|()| {
        (&buf == b"ready\n")
            .then_some(())
            .ok_or_else(|| io::Error::other("the watchdog said something else"))
    }) {
        // SAFETY: our own child, not yet reaped.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        super::reap(pid);
        return Err(io::Error::other(format!("the watchdog did not start: {e}")));
    }
    Ok((pid, stdin_w))
}

/// Starts the watchdog process: its own process group, an empty environment, cwd `/`, only
/// stdin, stdout and stderr inherited.
#[cfg(target_os = "linux")]
fn spawn_os(program: &Path, r: io::PipeReader, out: PipeWriter) -> io::Result<u32> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let nofile = super::linux_spawn::nofile();
    let mut c = Command::new(program);
    c.arg("watchdog")
        .arg(std::process::id().to_string())
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::from(r))
        .stdout(Stdio::from(out))
        .stderr(Stdio::inherit())
        .process_group(0);
    // SAFETY: only async-signal-safe syscalls, no allocation.
    unsafe {
        c.pre_exec(move || {
            super::linux_spawn::cloexec_from_3(nofile);
            Ok(())
        })
    };
    let child = c.spawn()?;
    drop(c);
    // `Child` neither kills nor reaps on drop; the monitor reaps by pid.
    Ok(child.id())
}

#[cfg(target_os = "macos")]
fn spawn_os(program: &Path, r: io::PipeReader, out: PipeWriter) -> io::Result<u32> {
    super::macos_spawn::spawn_watchdog(program, &r, &out)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn spawn_os(_program: &Path, _r: io::PipeReader, _out: PipeWriter) -> io::Result<u32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no watchdog on this OS",
    ))
}
