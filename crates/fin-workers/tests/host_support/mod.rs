//! A `fin-jobhost` host over the test-hooks `fin-workers` binary, for the process tests
//! (`host_common.rs`, `host_unix.rs`, `host_windows.rs`, `host_death_windows.rs`). Events of every process arrive on one channel; the
//! helpers pick the next event of one process and keep the others in order.

#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use fin_jobhost::*;
use fin_jobs::test_kinds::SpawnPids;
use fin_jobs::{Dirs, Hello, HostMsg, Limits, Start, StartJob, WorkerMsg};
use serde_json::Value;

pub const BIN: &str = env!("CARGO_BIN_EXE_fin-workers");
pub const WAIT: Duration = Duration::from_secs(10);

pub fn bin_dir() -> PathBuf {
    Path::new(BIN).parent().unwrap().to_path_buf()
}

/// Test timers: long enough not to fire by accident, short where a test needs them.
pub fn timing() -> Timing {
    Timing {
        hello_ms: 10_000,
        hung_ms: 30_000,
        kill_wait_ms: 3_000,
        rss_sample_ms: 1_000,
        exit_drain_ms: 200,
    }
}

pub struct TestHost {
    pub host: Host,
    rx: Receiver<HostEvent>,
    backlog: Vec<HostEvent>,
    pub dir: tempfile::TempDir,
}

pub fn host() -> TestHost {
    host_with(|_| {})
}

pub fn host_with(f: impl FnOnce(&mut HostOptions)) -> TestHost {
    host_observed(f, |_| {})
}

/// A host whose events `observe` sees first, on the thread that sends them.
pub fn host_observed(
    f: impl FnOnce(&mut HostOptions),
    observe: impl Fn(&HostEvent) + Send + Sync + 'static,
) -> TestHost {
    let mut opts = HostOptions::new(bin_dir());
    opts.timing = timing();
    opts.watchdog = false;
    f(&mut opts);
    let (tx, rx) = mpsc::sync_channel(1024);
    let sink = EventSink::new(move |e| {
        observe(&e);
        let _ = tx.send(e);
    });
    let host = Host::start(opts, sink).expect("the host starts");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    TestHost {
        host,
        rx,
        backlog: Vec::new(),
        dir,
    }
}

/// The environment every test process gets: one marker and its temp directory.
pub fn base_env(dir: &Path) -> Vec<(OsString, OsString)> {
    let tmp = dir.join("tmp").into_os_string();
    vec![
        ("FIN_TEST_MARKER".into(), "1".into()),
        ("TMPDIR".into(), tmp),
    ]
}

pub fn proc_of(e: &HostEvent) -> Option<ProcId> {
    match e {
        HostEvent::Spawned { proc, .. }
        | HostEvent::Hello { proc, .. }
        | HostEvent::Message { proc, .. }
        | HostEvent::Stderr { proc, .. }
        | HostEvent::Exit { proc, .. } => Some(*proc),
        HostEvent::GroupEmpty { .. } | HostEvent::AbandonedExit { .. } => None,
    }
}

impl TestHost {
    pub fn group(&self) -> GroupId {
        self.group_qos(Qos::Normal)
    }

    pub fn group_qos(&self, qos: Qos) -> GroupId {
        self.host.open_group(GroupSpec {
            label: "test".into(),
            memory_bytes: None,
            qos,
        })
    }

    /// A worker request for `fin-workers <args…>`.
    pub fn req(&self, args: &[&str]) -> SpawnRequest {
        SpawnRequest {
            role: Role::Worker,
            profile: Profile::Worker,
            family: "test".into(),
            program: Program::Bundled("fin-workers".into()),
            args: args.iter().map(OsString::from).collect(),
            env: base_env(self.dir.path()),
            cwd: self.dir.path().to_path_buf(),
            grants: Vec::new(),
            qos: Qos::Normal,
            label: "test".into(),
        }
    }

    pub fn spawn(&self, group: GroupId, req: SpawnRequest) -> Result<ProcId, HostError> {
        let (tx, rx) = mpsc::channel();
        self.host.spawn(
            group,
            req,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        rx.recv_timeout(WAIT).expect("the spawn answers")
    }

    /// `fin-workers test` in `group`.
    pub fn worker(&self, group: GroupId) -> ProcId {
        self.spawn(group, self.req(&["test"]))
            .expect("the worker spawns")
    }

    /// The next event matching `pred`, keeping the others for later.
    pub fn next(&mut self, mut pred: impl FnMut(&HostEvent) -> bool) -> HostEvent {
        if let Some(i) = self.backlog.iter().position(&mut pred) {
            return self.backlog.remove(i);
        }
        let until = Instant::now() + WAIT;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let e = self.rx.recv_timeout(left).unwrap_or_else(|_| {
                panic!(
                    "no matching event within {WAIT:?}; backlog {:?}",
                    self.backlog
                )
            });
            if pred(&e) {
                return e;
            }
            self.backlog.push(e);
        }
    }

    /// The next event of `proc` other than `Spawned` and `Stderr`.
    pub fn next_of(&mut self, proc: ProcId) -> HostEvent {
        self.next(|e| {
            proc_of(e) == Some(proc)
                && !matches!(e, HostEvent::Spawned { .. } | HostEvent::Stderr { .. })
        })
    }

    pub fn hello(&mut self, proc: ProcId) -> Hello {
        match self.next_of(proc) {
            HostEvent::Hello { hello, .. } => hello,
            e => panic!("hello first, got {e:?}"),
        }
    }

    pub fn message(&mut self, proc: ProcId) -> WorkerMsg {
        match self.next_of(proc) {
            HostEvent::Message { msg, .. } => msg,
            e => panic!("a message, got {e:?}"),
        }
    }

    pub fn result(&mut self, proc: ProcId) -> Value {
        loop {
            match self.message(proc) {
                WorkerMsg::Result { value, .. } => return value,
                WorkerMsg::Progress(_) | WorkerMsg::Begin { .. } | WorkerMsg::Item { .. } => {}
                m => panic!("a result, got {m:?}"),
            }
        }
    }

    /// The process's `Exit`, skipping its other events.
    pub fn exit(&mut self, proc: ProcId) -> ExitInfo {
        match self.next(|e| matches!(e, HostEvent::Exit { proc: p, .. } if *p == proc)) {
            HostEvent::Exit { info, .. } => info,
            _ => unreachable!(),
        }
    }

    /// The backlog and every event that arrives within `d`.
    pub fn events_within(&mut self, d: Duration) -> Vec<HostEvent> {
        let until = Instant::now() + d;
        let mut out = std::mem::take(&mut self.backlog);
        while let Ok(e) = self
            .rx
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            out.push(e);
        }
        out
    }

    pub fn group_empty(&mut self, group: GroupId) {
        self.next(|e| matches!(e, HostEvent::GroupEmpty { group: g } if *g == group));
    }

    pub fn start_msg(&self, job: &str, kind: &str, params: Value) -> HostMsg {
        HostMsg::Start(Start {
            job: StartJob {
                id: job.into(),
                kind: kind.into(),
                kind_version: 1,
                params,
                checkpoint: None,
                attempt: 1,
                extra: None,
            },
            limits: Limits {
                alive_ms: 100,
                memory_bytes: None,
                read_bytes_per_sec: None,
            },
            files: Vec::new(),
            helpers: Vec::new(),
            dirs: Dirs {
                job: self.dir.path().to_path_buf(),
                tmp: self.dir.path().join("tmp"),
            },
            diagnostics: false,
        })
    }

    pub fn start(&self, proc: ProcId, kind: &str, params: Value) {
        self.host
            .send(proc, &self.start_msg("j1", kind, params))
            .expect("start is queued");
    }

    /// Spawns a `test` worker in a new group, starts `kind` and returns its result.
    pub fn run(&mut self, kind: &str, params: Value) -> (ProcId, Value) {
        let g = self.group();
        let p = self.worker(g);
        self.hello(p);
        self.start(p, kind, params);
        let v = self.result(p);
        (p, v)
    }

    /// Runs `kill_group` and waits for its callback.
    pub fn kill(&self, group: GroupId, reason: KillReason) {
        let (tx, rx) = mpsc::channel();
        self.host.kill_group(
            group,
            reason,
            Box::new(move || {
                let _ = tx.send(());
            }),
        );
        rx.recv_timeout(WAIT).expect("kill_group answers");
    }
}

/// A log that keeps its lines.
pub fn captured_log() -> (LogFn, Arc<std::sync::Mutex<Vec<String>>>) {
    let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
    let l = lines.clone();
    (
        Arc::new(move |line: &str| l.lock().unwrap().push(line.to_owned())),
        lines,
    )
}

/// A group with a memory limit.
pub fn limited_group(h: &TestHost, bytes: u64) -> GroupId {
    h.host.open_group(GroupSpec {
        label: "limited".into(),
        memory_bytes: Some(bytes),
        qos: Qos::Normal,
    })
}

/// A spawner that rewrites the command with `f`.
pub struct Rewrite<F>(pub F);

impl<F> Spawner for Rewrite<F>
where
    F: Fn(&mut PlatformCommand) + Send + Sync,
{
    fn name(&self) -> &'static str {
        "rewrite"
    }
    fn sandboxed(&self) -> bool {
        false
    }
    fn prepare(&self, _req: &SpawnRequest, cmd: &mut PlatformCommand) -> Result<(), SpawnError> {
        (self.0)(cmd);
        Ok(())
    }
}

pub fn rewrite(f: impl Fn(&mut PlatformCommand) + Send + Sync + 'static) -> Arc<dyn Spawner> {
    Arc::new(Rewrite(f))
}

/// Reads the pids `test.spawn` wrote into `name` (JSON, then the file's zero padding).
pub fn spawn_pids(h: &TestHost, name: &str) -> SpawnPids {
    let path = h.dir.path().join(name);
    let until = Instant::now() + WAIT;
    loop {
        if let Ok(bytes) = std::fs::read(&path) {
            let text = String::from_utf8_lossy(&bytes);
            let text = text.trim_end_matches('\0');
            if let Ok(p) = serde_json::from_str(text) {
                return p;
            }
        }
        assert!(Instant::now() < until, "test.spawn wrote no pids");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether the host killed the process: `SIGKILL` on Unix, [`KILLED_EXIT_CODE`] on Windows
/// (which has no signals).
pub fn killed(exit: &ExitInfo) -> bool {
    #[cfg(unix)]
    return exit.signal == Some("kill");
    #[cfg(windows)]
    return exit.signal.is_none() && exit.code == Some(i64::from(KILLED_EXIT_CODE));
}

/// Whether `pid` is gone: no such process, or a zombie nobody has reaped yet.
#[cfg(unix)]
pub fn gone(pid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    if unsafe { libc::kill(pid as i32, 0) } != 0 {
        return true;
    }
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        && let Some(rest) = stat.rsplit_once(") ")
    {
        return rest.1.starts_with('Z');
    }
    #[cfg(target_os = "macos")]
    {
        // A zombie answers `kill(pid, 0)`, but `proc_pidinfo` refuses it with ESRCH (macOS
        // 27; older releases filled in `SZOMB`), so either counts as gone.
        // SAFETY: a zeroed proc_bsdshortinfo is a valid out-buffer; proc_pidinfo fills it
        // and returns its size.
        let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as i32;
        let n = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDT_SHORTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdshortinfo).cast(),
                size,
            )
        };
        if n == size {
            return info.pbsi_status == libc::SZOMB;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Whether `pid` is gone: it cannot be opened, or it has exited.
#[cfg(windows)]
pub fn gone(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: opens a process by id; the handle is closed below.
    let h = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if h.is_null() {
        return true;
    }
    // SAFETY: a process handle we just opened.
    let running = unsafe { WaitForSingleObject(h, 0) } == WAIT_TIMEOUT;
    // SAFETY: as above, closed once.
    unsafe { CloseHandle(h) };
    !running
}

/// Ends `pid` from outside the host (`SIGKILL`; `TerminateProcess` with code 1).
pub fn kill_pid(pid: u32) {
    #[cfg(unix)]
    // SAFETY: a signal to a process this test started.
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL)
    };
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        // SAFETY: opens a process this test started; the handle is closed below.
        let h = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        assert!(!h.is_null(), "open {pid}");
        // SAFETY: a process handle we just opened, closed once.
        unsafe {
            TerminateProcess(h, 1);
            CloseHandle(h);
        }
    }
}

/// Polls [`gone`] for ≤ 5 s.
pub fn wait_gone(pid: u32) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if gone(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    gone(pid)
}
