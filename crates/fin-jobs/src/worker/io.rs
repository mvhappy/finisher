//! The runtime's channel to its host: the stdout writer (one mutex over the buffered writer
//! and the `seq` counter, flushed per message), the stdin reader thread (decodes
//! [`HostMsg`], routes `reply` to the waiting caller, `cancel` to the job's token, everything
//! else to the run loop, and exits the process on EOF) and the `alive` thread.

use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::codec::{self, CodecError, Line, MAX_LINE};
use crate::msg_host::{Attach, HostMsg, Start};
use crate::msg_worker::WorkerMsg;
use crate::worker::calls::Calls;
use crate::worker::ctx::CancelToken;
use crate::{CancelReason, DEFAULT_ALIVE_MS, EXIT_HOST_GONE};

/// A runtime's exit code when its host sends a line it cannot decode.
pub(crate) const EXIT_HOST_PROTOCOL: i32 = 65;

/// Where a runtime talks to its host. [`Channel::stdio`] is the real one; tests drive a
/// runtime over in-memory pipes with `hard_exit: false`.
pub struct Channel {
    pub input: Box<dyn Read + Send>,
    pub output: Box<dyn Write + Send>,
    /// Exit the process at once on stdin EOF (70), a broken stdout (70) or an undecodable
    /// host line (65). Without it the run returns those codes when it is next idle.
    pub hard_exit: bool,
    /// The self-sandbox request ([`crate::sandbox::ENV`]'s value).
    pub self_sandbox: Option<String>,
    /// The pid `hello` reports.
    pub pid: u32,
}

impl Channel {
    /// The process's stdin and stdout, the request from the environment, and exits.
    pub fn stdio() -> Channel {
        Channel {
            input: Box::new(std::io::stdin()),
            output: Box::new(std::io::stdout()),
            hard_exit: true,
            self_sandbox: crate::sandbox::from_env(),
            pid: std::process::id(),
        }
    }
}

/// What the reader hands the run loop.
pub(crate) enum Inbound {
    /// The job is already registered for `cancel` with this token.
    Start(Box<Start>, CancelToken),
    Attach(Attach, CancelToken),
    Shutdown,
    HostGone,
    Protocol,
}

struct OutState {
    w: BufWriter<Box<dyn Write + Send>>,
    seq: u64,
    last: Instant,
}

/// The writer to the host.
pub(crate) struct Out {
    state: Mutex<OutState>,
    hard_exit: bool,
    broken: AtomicBool,
}

impl Out {
    fn new(w: Box<dyn Write + Send>, hard_exit: bool) -> Out {
        Out {
            state: Mutex::new(OutState {
                w: BufWriter::new(w),
                seq: 0,
                last: Instant::now(),
            }),
            hard_exit,
            broken: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OutState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Encodes and writes one message with the next `seq`. An encode error (a line over the
    /// cap) is returned and consumes no `seq`; a write error means the host is gone.
    pub(crate) fn send(&self, msg: &WorkerMsg) -> Result<(), CodecError> {
        let mut s = self.lock();
        let line = codec::encode(s.seq + 1, msg)?;
        s.seq += 1;
        self.write(&mut s, &line);
        Ok(())
    }

    fn write(&self, s: &mut OutState, bytes: &[u8]) {
        if self.broken.load(Ordering::Relaxed) {
            return;
        }
        let ok = s.w.write_all(bytes).and_then(|()| s.w.flush());
        s.last = Instant::now();
        if ok.is_err() {
            self.broken.store(true, Ordering::Relaxed);
            if self.hard_exit {
                std::process::exit(EXIT_HOST_GONE);
            }
        }
    }

    /// Time since the last write.
    fn idle_for(&self) -> Duration {
        self.lock().last.elapsed()
    }

    /// Writes `bytes` as they are, consuming no `seq` (test-hooks garbage).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn send_raw(&self, bytes: &[u8]) {
        let mut s = self.lock();
        self.write(&mut s, bytes);
    }

    /// Writes the object `build(seq)` returns with the next `seq` (test-hooks).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn send_value(&self, build: impl FnOnce(u64) -> serde_json::Value) {
        let mut s = self.lock();
        s.seq += 1;
        let mut line = build(s.seq).to_string().into_bytes();
        line.push(b'\n');
        self.write(&mut s, &line);
    }

    /// Skips one `seq` (test-hooks: a gap).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn skip_seq(&self) {
        self.lock().seq += 1;
    }
}

/// The `alive` thread's controls.
struct Alive {
    interval_ms: AtomicU32,
    paused: AtomicBool,
    stop: Mutex<bool>,
    cv: Condvar,
}

/// The running job's token, and a stop that outlives it: once the host is gone (or, for a
/// helper, sent `shutdown`) every token is cancelled, also one registered later.
#[derive(Default)]
struct Current {
    job: Option<(String, CancelToken)>,
    stopped: bool,
}

/// State the run loop, the reader, the `alive` thread and every [`crate::worker::JobCtx`]
/// share.
pub(crate) struct Shared {
    pub(crate) out: Out,
    pub(crate) calls: Calls,
    current: Mutex<Current>,
    alive: Alive,
    ignore_eof: AtomicBool,
    hard_exit: bool,
    /// A helper: `shutdown` also cancels the running `run`.
    shutdown_cancels: bool,
}

impl Shared {
    pub(crate) fn new(
        output: Box<dyn Write + Send>,
        hard_exit: bool,
        shutdown_cancels: bool,
    ) -> Arc<Shared> {
        Arc::new(Shared {
            out: Out::new(output, hard_exit),
            calls: Calls::default(),
            current: Mutex::new(Current::default()),
            alive: Alive {
                interval_ms: AtomicU32::new(DEFAULT_ALIVE_MS),
                paused: AtomicBool::new(false),
                stop: Mutex::new(false),
                cv: Condvar::new(),
            },
            ignore_eof: AtomicBool::new(false),
            hard_exit,
            shutdown_cancels,
        })
    }

    /// The job whose `cancel` messages reach `token`.
    /// Registers `job` for `cancel` (from the reader, as it decodes `start`/`attach`, so a
    /// `cancel` right behind it is never lost) and returns its token.
    fn register(&self, job: &str) -> CancelToken {
        let token = CancelToken::new();
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if current.stopped {
            token.cancel(CancelReason::Cancel);
        }
        current.job = Some((job.to_owned(), token.clone()));
        token
    }

    /// The job ended: its `cancel` messages go nowhere now.
    pub(crate) fn finish(&self, job: &str) {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if current.job.as_ref().is_some_and(|(id, _)| id == job) {
            current.job = None;
        }
    }

    /// Cancels the job `job` (or whichever runs, with `None`).
    fn cancel(&self, job: Option<&str>, reason: CancelReason) {
        let current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((id, token)) = current.job.as_ref()
            && job.is_none_or(|j| j == id)
        {
            token.cancel(reason);
        }
    }

    /// Cancels the running job and every later one.
    fn stop(&self) {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        current.stopped = true;
        if let Some((_, token)) = current.job.as_ref() {
            token.cancel(CancelReason::Cancel);
        }
    }

    pub(crate) fn set_alive_interval(&self, ms: u32) {
        self.alive.interval_ms.store(ms.max(1), Ordering::Relaxed);
        self.alive.cv.notify_all();
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn pause_alive(&self) {
        self.alive.paused.store(true, Ordering::Relaxed);
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn ignore_eof(&self) {
        self.ignore_eof.store(true, Ordering::Relaxed);
    }

    /// Stops the `alive` thread (the run is over).
    pub(crate) fn stop_alive(&self) {
        *self.alive.stop.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.alive.cv.notify_all();
    }
}

/// Starts the `alive` thread: it wakes every half interval and sends `alive` when nothing was
/// written for a whole one.
pub(crate) fn spawn_alive(shared: &Arc<Shared>) {
    let shared = Arc::clone(shared);
    let _ = std::thread::Builder::new()
        .name("fin-jobs-alive".into())
        .spawn(move || {
            let a = &shared.alive;
            let mut stop = a.stop.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let interval =
                    Duration::from_millis(u64::from(a.interval_ms.load(Ordering::Relaxed)));
                let (guard, _) =
                    a.cv.wait_timeout(stop, interval / 2)
                        .unwrap_or_else(|e| e.into_inner());
                stop = guard;
                if *stop {
                    return;
                }
                if !a.paused.load(Ordering::Relaxed) && shared.out.idle_for() >= interval {
                    drop(stop);
                    let _ = shared.out.send(&WorkerMsg::Alive {});
                    stop = a.stop.lock().unwrap_or_else(|e| e.into_inner());
                }
            }
        });
}

/// Starts the stdin reader thread.
pub(crate) fn spawn_reader(input: Box<dyn Read + Send>, shared: &Arc<Shared>, tx: Sender<Inbound>) {
    let shared = Arc::clone(shared);
    let _ = std::thread::Builder::new()
        .name("fin-jobs-reader".into())
        .spawn(move || read_loop(input, &shared, &tx));
}

fn read_loop(input: Box<dyn Read + Send>, shared: &Shared, tx: &Sender<Inbound>) {
    let mut r = BufReader::new(input);
    let mut buf = Vec::new();
    let mut seq = 1u64;
    loop {
        let line = match codec::read_line(&mut r, &mut buf, MAX_LINE) {
            Ok(Line::Line) => &buf,
            Ok(Line::Eof) | Err(_) => return host_gone(shared, tx),
            Ok(Line::TooLong { len }) => {
                return protocol(shared, tx, &CodecError::TooLong { len });
            }
        };
        let msg = match codec::decode_host(line, seq) {
            Ok(m) => m,
            Err(e) => return protocol(shared, tx, &e),
        };
        seq += 1;
        let inbound = match msg {
            HostMsg::Reply { id, value, error } => {
                shared.calls.resolve(id, value, error);
                continue;
            }
            HostMsg::Cancel { job, reason } => {
                shared.cancel(Some(&job), reason);
                continue;
            }
            HostMsg::Start(s) => {
                let token = shared.register(&s.job.id);
                Inbound::Start(Box::new(s), token)
            }
            HostMsg::Attach(a) => {
                let token = shared.register(&a.job);
                Inbound::Attach(a, token)
            }
            HostMsg::Shutdown {} => {
                if shared.shutdown_cancels {
                    shared.stop();
                }
                Inbound::Shutdown
            }
        };
        if tx.send(inbound).is_err() {
            return;
        }
    }
}

fn host_gone(shared: &Shared, tx: &Sender<Inbound>) {
    if shared.ignore_eof.load(Ordering::Relaxed) {
        return;
    }
    if shared.hard_exit {
        std::process::exit(EXIT_HOST_GONE);
    }
    shared.calls.fail_all();
    shared.stop();
    let _ = tx.send(Inbound::HostGone);
}

fn protocol(shared: &Shared, tx: &Sender<Inbound>, e: &CodecError) {
    eprintln!("fin-jobs: the host sent a bad line: {e}");
    if shared.hard_exit {
        std::process::exit(EXIT_HOST_PROTOCOL);
    }
    shared.calls.fail_all();
    shared.stop();
    let _ = tx.send(Inbound::Protocol);
}
