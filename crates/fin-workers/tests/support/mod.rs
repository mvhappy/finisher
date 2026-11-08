//! A minimal host for the `library` family tests: spawns `fin-workers library`, answers
//! host calls through a closure and records everything the worker sends, in order.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use fin_jobs::codec::{self, Decoded};
use fin_jobs::{
    CancelReason, Dirs, FileGrant, FileMode, HostMsg, JobError, Limits, Start, StartJob, WorkerMsg,
};
use fin_library::{Expect, RawRel, VolumeIdentity};
use serde_json::{Value, json};

pub const BIN: &str = env!("CARGO_BIN_EXE_fin-workers");
const WAIT: Duration = Duration::from_secs(20);

/// What the worker sent, in order.
#[derive(Debug, Clone)]
pub enum Ev {
    Begin(String),
    Item(String, Option<Value>),
    ItemFailed(String, String),
    Call(String, Value),
    Progress(f64),
    Checkpoint(Value),
}

/// How a job ended.
#[derive(Debug)]
pub enum End {
    Result(Value),
    Error(JobError),
    /// The process died without answering.
    Died(ExitStatus),
}

#[derive(Debug)]
pub struct Run {
    pub events: Vec<Ev>,
    /// Non-`alive` messages that were already waiting when a call was about to be answered:
    /// a worker that waits for replies has none.
    pub early: Vec<String>,
    pub end: End,
}

impl Run {
    pub fn calls(&self, method: &str) -> Vec<&Value> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Ev::Call(m, p) if m == method => Some(p),
                _ => None,
            })
            .collect()
    }

    pub fn items(&self) -> Vec<&str> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Ev::Item(i, _) => Some(i.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn progress(&self) -> Vec<f64> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Ev::Progress(d) => Some(*d),
                _ => None,
            })
            .collect()
    }

    pub fn result(&self) -> &Value {
        match &self.end {
            End::Result(v) => v,
            e => panic!("a result, got {e:?}"),
        }
    }

    pub fn error(&self) -> &JobError {
        match &self.end {
            End::Error(e) => e,
            e => panic!("an error, got {e:?}"),
        }
    }
}

pub struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Vec<u8>>,
    host_seq: u64,
    worker_seq: u64,
}

pub fn spawn(family: &str) -> Proc {
    let mut child = Command::new(BIN)
        .arg(family)
        .env_clear()
        .env("FIN_TEST_MARKER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match r.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
            }
        }
    });
    Proc {
        stdin: child.stdin.take(),
        child,
        lines,
        host_seq: 0,
        worker_seq: 0,
    }
}

impl Proc {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    fn decode(&mut self, line: &[u8]) -> WorkerMsg {
        self.worker_seq += 1;
        match codec::decode_worker(line, self.worker_seq, false).unwrap() {
            Decoded::Msg(m) => m,
            Decoded::Unknown { t, .. } => panic!("unknown message {t}"),
        }
    }

    /// The next message other than `alive`, or `None` when the process ended.
    fn recv(&mut self) -> Option<WorkerMsg> {
        loop {
            match self.lines.recv_timeout(WAIT) {
                Ok(line) => match self.decode(&line) {
                    WorkerMsg::Alive {} => continue,
                    m => return Some(m),
                },
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => panic!("no message within {WAIT:?}"),
            }
        }
    }

    pub fn hello(&mut self) -> fin_jobs::Hello {
        match self.recv() {
            Some(WorkerMsg::Hello(h)) => h,
            m => panic!("hello first, got {m:?}"),
        }
    }

    pub fn send(&mut self, msg: &HostMsg) {
        self.host_seq += 1;
        let line = codec::encode(self.host_seq, msg).unwrap();
        let _ = self.stdin.as_mut().unwrap().write_all(&line);
    }

    pub fn cancel(&mut self, job: &str) {
        self.send(&HostMsg::Cancel {
            job: job.into(),
            reason: CancelReason::Cancel,
        });
    }

    /// Reads until the job ends, answering each call with `reply(method, params)`. Messages
    /// already waiting when a reply is about to go out are recorded in `early`.
    pub fn drive(&mut self, reply: impl FnMut(&str, &Value) -> Result<Value, JobError>) -> Run {
        self.drive_cancelling(None, reply)
    }

    /// [`Proc::drive`], sending a `cancel` for `j1` just before the reply to the `n`th call
    /// (counted from 1), and giving the worker 50 ms to see it.
    pub fn drive_cancelling(
        &mut self,
        cancel_at_call: Option<usize>,
        mut reply: impl FnMut(&str, &Value) -> Result<Value, JobError>,
    ) -> Run {
        let mut calls = 0;
        let mut events = Vec::new();
        let mut early = Vec::new();
        let end = loop {
            let Some(msg) = self.recv() else {
                break End::Died(self.wait());
            };
            match msg {
                WorkerMsg::Begin { item, .. } => events.push(Ev::Begin(item)),
                WorkerMsg::Item {
                    item, value, error, ..
                } => events.push(match error {
                    Some(e) => Ev::ItemFailed(item, e.code),
                    None => Ev::Item(item, value),
                }),
                WorkerMsg::Progress(p) => events.push(Ev::Progress(p.done)),
                WorkerMsg::Checkpoint { data, .. } => events.push(Ev::Checkpoint(data)),
                WorkerMsg::Call { id, method, params } => {
                    events.push(Ev::Call(method.clone(), params.clone()));
                    let answer = reply(&method, &params);
                    calls += 1;
                    if cancel_at_call == Some(calls) {
                        self.cancel("j1");
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    while let Ok(line) = self.lines.try_recv() {
                        match self.decode(&line) {
                            WorkerMsg::Alive {} => {}
                            m => early.push(m.type_name().to_owned()),
                        }
                    }
                    let (value, error) = match answer {
                        Ok(v) => (Some(v), None),
                        Err(e) => (None, Some(e)),
                    };
                    self.send(&HostMsg::Reply { id, value, error });
                }
                WorkerMsg::Result { value, .. } => break End::Result(value),
                WorkerMsg::Error { error, .. } => break End::Error(error),
                WorkerMsg::Ready {} | WorkerMsg::Diag { .. } | WorkerMsg::Hello(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        };
        Run { events, early, end }
    }

    pub fn wait(&mut self) -> ExitStatus {
        let until = Instant::now() + WAIT;
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(Instant::now() < until, "the process did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A job directory with `out/` and `in/`, and the grants for the files a test names.
pub struct Job {
    pub dir: tempfile::TempDir,
}

impl Job {
    pub fn new() -> Job {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        std::fs::create_dir_all(dir.path().join("in")).unwrap();
        Job { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name.trim_end_matches('/'))
    }

    pub fn out(&self) -> PathBuf {
        self.path("out")
    }

    pub fn write_in(&self, name: &str, lines: &[Value]) {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(self.path(name), text).unwrap();
    }

    pub fn start(&self, kind: &str, params: Value, grants: &[&str]) -> Start {
        let files = grants
            .iter()
            .map(|name| FileGrant {
                name: (*name).into(),
                path: self.path(name),
                mode: if name.ends_with('/') {
                    FileMode::ReadWrite
                } else {
                    FileMode::Read
                },
                bytes: None,
            })
            .collect();
        Start {
            job: StartJob {
                id: "j1".into(),
                kind: kind.into(),
                kind_version: 1,
                params,
                checkpoint: None,
                attempt: 1,
                extra: None,
            },
            limits: Limits::default(),
            files,
            helpers: vec![],
            dirs: Dirs {
                job: self.dir.path().to_path_buf(),
                tmp: self.path("tmp"),
            },
            diagnostics: false,
        }
    }
}

/// Spawns the `library` family and starts `start` on it.
pub fn run_job(start: Start) -> Proc {
    let mut p = spawn("library");
    p.hello();
    p.send(&HostMsg::Start(start));
    p
}

pub fn write_file(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
    p
}

/// What the catalog would expect for the file at `path`.
#[cfg(unix)]
pub fn expect_of(path: &Path) -> Expect {
    let m = std::fs::symlink_metadata(path).unwrap();
    Expect {
        size: m.len(),
        mtime_ns: m.mtime() * 1_000_000_000 + m.mtime_nsec(),
        file_id: Some(m.ino().to_be_bytes().to_vec()),
        dev: None,
    }
}

/// What the catalog would expect for the file at `path`: on Windows its entry as the native
/// source lists it (size, 100 ns mtime, 128-bit id).
#[cfg(windows)]
pub fn expect_of(path: &Path) -> Expect {
    use fin_library::Source;
    let src = fin_library::source::NativeSource::new(path.parent().unwrap());
    let dir = src.open_dir(&RawRel::root()).unwrap();
    let name = path.file_name().unwrap().to_str().unwrap().as_bytes();
    let e = src.stat_child(&dir, name).unwrap().expect("listed");
    Expect {
        size: e.size,
        mtime_ns: e.mtime_ns,
        file_id: e.file_id,
        dev: None,
    }
}

pub fn rel(parts: &[&str]) -> RawRel {
    RawRel::new(parts.iter().map(|p| p.as_bytes().to_vec()).collect()).unwrap()
}

/// The identity of `root` as `library.add` would have stored it.
pub fn identity_of(root: &Path) -> VolumeIdentity {
    fin_library::identity(root).unwrap()
}

/// The overlay file of a root.
pub fn overlay(root: &Path, spec: Value) {
    std::fs::write(root.join(fin_library::overlay::FILE), spec.to_string()).unwrap();
}

pub fn no_calls(_: &str, _: &Value) -> Result<Value, JobError> {
    Ok(json!({}))
}
