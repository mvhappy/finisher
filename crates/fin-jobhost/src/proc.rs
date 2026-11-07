//! One process: its state, its stdout and stderr readers and its waiter (Decisions 4, 11a,
//! 19, 19a). The readers hand events to the sink and may block there; a [`Gauge`] records when
//! a reader is waiting on its pipe, so liveness counts only that time. The waiter observes the
//! exit, lets the readers drain (EOF, or `exit_drain_ms` of an empty pipe since the exit), then
//! sends `Exit` through the same lock as the messages, so a `result` written just before
//! `exit(0)` always comes first.

use std::io::{self, BufReader, PipeReader, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak, mpsc};
use std::time::{Duration, Instant};

use fin_jobs::codec::{self, Line, MAX_LINE};
use fin_jobs::{HostMsg, JobError, WorkerMsg};

use crate::domain::LeafHandle;
use crate::exit::{ExitReason, MAX_STDERR_LINE, RawExit, Tail};
use crate::framing::{Expect, Frame, Framing};
use crate::host::Inner;
use crate::writer::Writer;
use crate::{EventSink, ExitInfo, GroupId, HostEvent, ProcId, Watch};

/// When a reader started waiting on its pipe, and whether it reached EOF.
pub(crate) struct Gauge {
    base: Instant,
    /// Microseconds since `base` + 1 while waiting; 0 while busy.
    since: AtomicU64,
    eof: AtomicBool,
}

impl Gauge {
    fn new(base: Instant) -> Arc<Gauge> {
        Arc::new(Gauge {
            base,
            since: AtomicU64::new(0),
            eof: AtomicBool::new(false),
        })
    }

    /// Since when the reader has been waiting on its pipe, if it is.
    pub fn waiting_since(&self) -> Option<Instant> {
        match self.since.load(Ordering::Acquire) {
            0 => None,
            us => Some(self.base + Duration::from_micros(us - 1)),
        }
    }

    pub fn eof(&self) -> bool {
        self.eof.load(Ordering::Acquire)
    }

    fn set_eof(&self) {
        self.eof.store(true, Ordering::Release);
    }

    /// Drained since `exit`: EOF, or waiting on an empty pipe for `drain` since `exit`.
    fn drained(&self, exit: Instant, drain: Duration, now: Instant) -> bool {
        self.eof()
            || self
                .waiting_since()
                .is_some_and(|s| now.saturating_duration_since(s.max(exit)) >= drain)
    }
}

/// A pipe whose reads are recorded on a [`Gauge`].
struct Gauged {
    pipe: PipeReader,
    gauge: Arc<Gauge>,
}

impl Read for Gauged {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let us = self.gauge.base.elapsed().as_micros() as u64 + 1;
        self.gauge.since.store(us, Ordering::Release);
        let r = self.pipe.read(buf);
        self.gauge.since.store(0, Ordering::Release);
        r
    }
}

/// What the timers and the kills look at.
pub(crate) struct ProcState {
    pub hello: bool,
    /// The job last started on it (`send(start)`).
    pub job: Option<String>,
    pub watch: Option<Watch>,
    pub watch_gen: u64,
    pub last_progress: Instant,
    /// The reason it will be reported with, once the host decided to kill it.
    pub kill: Option<ExitReason>,
    pub exited: bool,
}

pub(crate) struct Proc {
    pub id: ProcId,
    pub group: GroupId,
    pub pid: u32,
    /// The group's first process (Unix: the pgid's owner, reaped only at close).
    pub leader: bool,
    pub started: Instant,
    /// The self-sandbox profile requested (`hello.selfSandbox` must match).
    pub self_sandbox: Option<String>,
    pub writer: Writer,
    pub out: Arc<Gauge>,
    pub err: Arc<Gauge>,
    state: Mutex<ProcState>,
    /// Held while an event is handed to the sink; `true` once `Exit` went out.
    emit: Mutex<bool>,
    sink: EventSink,
    tail: Mutex<Tail>,
    /// The process's leaf domain, held while the process lives (a leaf Job, Task 5).
    _leaf: LeafHandle,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Proc {
    pub fn new(
        id: ProcId,
        group: GroupId,
        pid: u32,
        leader: bool,
        self_sandbox: Option<String>,
        sink: EventSink,
        leaf: LeafHandle,
    ) -> Proc {
        let started = Instant::now();
        Proc {
            id,
            group,
            pid,
            leader,
            started,
            self_sandbox,
            writer: Writer::default(),
            out: Gauge::new(started),
            err: Gauge::new(started),
            state: Mutex::new(ProcState {
                hello: false,
                job: None,
                watch: None,
                watch_gen: 0,
                last_progress: started,
                kill: None,
                exited: false,
            }),
            emit: Mutex::new(false),
            sink,
            tail: Mutex::default(),
            _leaf: leaf,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, ProcState> {
        lock(&self.state)
    }

    /// Records the reason it will be reported with, unless it already has one or exited.
    /// `true` if the reason was taken.
    pub fn mark_kill(&self, reason: ExitReason) -> bool {
        let mut st = self.state();
        if st.exited || st.kill.is_some() {
            return false;
        }
        st.kill = Some(reason);
        true
    }

    fn emit(&self, e: HostEvent) {
        let closed = lock(&self.emit);
        if !*closed {
            self.sink.send(e);
        }
    }

    fn emit_exit(&self, info: ExitInfo) {
        let mut closed = lock(&self.emit);
        *closed = true;
        self.sink.send(HostEvent::Exit {
            proc: self.id,
            info,
        });
    }

    pub fn stderr_tail(&self) -> String {
        lock(&self.tail).text()
    }
}

/// The stdout reader thread: frames, checks and forwards every line.
pub(crate) fn read_stdout(p: Arc<Proc>, inner: Weak<Inner>, pipe: PipeReader, expect: Expect) {
    let gauge = p.out.clone();
    let mut r = BufReader::with_capacity(64 << 10, Gauged { pipe, gauge });
    let mut framing = Framing::new(expect);
    let mut buf = Vec::new();
    loop {
        let frame = match codec::read_line(&mut r, &mut buf, MAX_LINE) {
            Ok(Line::Line) => {
                let job = p.state().job.clone();
                framing.line(&buf, job.as_deref())
            }
            Ok(Line::TooLong { .. }) => framing.too_long(),
            Ok(Line::Eof) | Err(_) => break,
        };
        match frame {
            Frame::Hello(hello) => {
                p.state().hello = true;
                if let Some(i) = inner.upgrade() {
                    i.on_hello(&p);
                }
                p.emit(HostEvent::Hello { proc: p.id, hello });
            }
            Frame::Msg(msg) => {
                if matches!(
                    msg,
                    WorkerMsg::Begin { .. } | WorkerMsg::Item { .. } | WorkerMsg::Progress(_)
                ) {
                    p.state().last_progress = Instant::now();
                }
                let memory = match msg {
                    WorkerMsg::Ready {} => inner.upgrade().and_then(|i| i.group_memory(p.group)),
                    _ => None,
                };
                p.emit(HostEvent::Message {
                    proc: p.id,
                    msg,
                    memory,
                });
            }
            Frame::Alive | Frame::DropUnknown => {}
            Frame::ReplyUnknown(id) => {
                let error = JobError::new("UNKNOWN_MESSAGE", "this host does not know the message");
                let _ = p.writer.send(&HostMsg::Reply {
                    id,
                    value: None,
                    error: Some(error),
                });
            }
            Frame::Fail(reason) => {
                if let Some(i) = inner.upgrade() {
                    i.kill_for(p.id, reason);
                }
                break;
            }
        }
    }
    p.out.set_eof();
}

/// The stderr reader thread: keeps the tail and forwards lines (≤ 4 KiB; the rest of a longer
/// line is dropped).
pub(crate) fn read_stderr(p: Arc<Proc>, pipe: PipeReader) {
    let gauge = p.err.clone();
    let mut r = Gauged { pipe, gauge };
    let mut chunk = vec![0u8; 8 << 10];
    let mut line = Vec::new();
    loop {
        let n = match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        lock(&p.tail).push(&chunk[..n]);
        for part in chunk[..n].split_inclusive(|&b| b == b'\n') {
            let (text, end) = match part.strip_suffix(b"\n") {
                Some(t) => (t, true),
                None => (part, false),
            };
            let room = MAX_STDERR_LINE.saturating_sub(line.len());
            line.extend_from_slice(&text[..text.len().min(room)]);
            if end {
                send_stderr(&p, &mut line);
            }
        }
    }
    if !line.is_empty() {
        send_stderr(&p, &mut line);
    }
    p.err.set_eof();
}

fn send_stderr(p: &Proc, line: &mut Vec<u8>) {
    let text = String::from_utf8_lossy(line)
        .trim_end_matches('\r')
        .to_owned();
    line.clear();
    p.emit(HostEvent::Stderr {
        proc: p.id,
        line: text,
    });
}

/// The waiter thread: observes the exit (a leader stays a zombie), lets the readers drain,
/// sends `Exit`, updates the tables, and reaps a leader once its group is closed (`reap`'s
/// sender is dropped).
pub(crate) fn wait_exit(
    p: Arc<Proc>,
    inner: Weak<Inner>,
    os: crate::os::OsProc,
    drain: Duration,
    reap: Option<mpsc::Receiver<()>>,
) {
    let raw = crate::os::wait(&os, p.pid, !p.leader).unwrap_or(RawExit {
        code: None,
        signal: None,
    });
    p.state().exited = true;
    p.writer.kill();
    let exit = Instant::now();
    loop {
        let now = Instant::now();
        if p.out.drained(exit, drain, now) && p.err.drained(exit, drain, now) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let info = match inner.upgrade() {
        Some(i) => i.exit_info(&p, raw),
        None => crate::host::bare_exit_info(&p, raw),
    };
    p.emit_exit(info);
    if let Some(i) = inner.upgrade() {
        i.exited(&p);
    }
    if let Some(rx) = reap {
        // Returns when the group is closed (its sender dropped).
        let _ = rx.recv();
        crate::os::reap(p.pid);
    }
}
