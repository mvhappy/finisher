//! Encoder sessions and `ENCODER_HUNG` (plan 6B-1 Decision 20, spec §17, arch §27.3).
//!
//! Each encoder is opened and driven on a thread of its own (a platform backend sets up its
//! per-thread state, COM on Windows, in its `open`). The session counts an encoder as hung
//! when:
//! - opening it, or one call into it (`send`, one `receive`, `finish`), runs past the deadline
//!   (`max(2 s, 4 frame durations × 30)`, [`EncoderConfig::hang_deadline`]); the clock covers
//!   the call only, so a long flush that keeps producing packets is not a hang;
//! - it holds [`HUNG_FRAMES`] frames without producing a packet (counted on its thread);
//! - after `finish`, nothing comes out for the deadline.
//!
//! Deviation from Decision 20's wording: before `finish`, "no packet for the deadline" is not
//! timed (a slow caller, a decoder, would trip it); the frame bound and the per-call deadline
//! stand in for it, and the end of the stream catches the rest.
//!
//! A hung encoder is abandoned: its thread is detached, never joined, and drops the encoder
//! itself whenever its call returns (it never blocks on the session: events are unbounded).
//! The session opens the next allowed candidate and answers [`Fed::Restart`] /
//! [`Output::Restart`]: the caller drops every packet received and sends again from its first
//! frame. A second hang in the session, or a hang with no candidate left, is
//! `ENCODER_HUNG {backend, adapter}`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError, channel,
    sync_channel,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fin_container::CodecConfig;
use fin_core::CoreError;
use fin_media::backend::BackendEntry;
use serde_json::json;

use crate::config::EncoderConfig;
use crate::frame::{EncodedPacket, RawFrame};
use crate::registry::{EncoderBackend, OpenToken};
use crate::traits::VideoEncoder;

/// Frames an encoder may hold without producing a packet (lookahead included) before it
/// counts as hung.
pub const HUNG_FRAMES: u32 = 300;

/// Hangs one session tolerates before failing.
const MAX_HANGS: u32 = 2;

/// Why a session moved to another encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// The encoder did not open.
    OpenFailed,
    /// `ENCODER_HUNG`'s deadline passed (opening it, or encoding).
    Hung,
}

/// One move from an encoder to the next (recorded as a warning in the job's result).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallbackNote {
    pub from: &'static str,
    pub to: &'static str,
    pub reason: FallbackReason,
}

/// What `send` or `finish` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed {
    Accepted,
    /// The encoder hung and another took its place: start again from the first frame.
    Restart,
}

/// What `receive` gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Packet(EncodedPacket),
    /// Nothing yet (before `finish`).
    Pending,
    /// Every packet has come out (after `finish`); every later `receive` says so again.
    End,
    /// The encoder hung and another took its place: drop every packet received and start
    /// again from the first frame.
    Restart,
}

enum Cmd {
    Frame(Box<RawFrame>, i64, bool),
    Finish,
    Config(SyncSender<CodecConfig>),
}

enum Evt {
    Opened,
    Packet(EncodedPacket),
    Failed(CoreError),
    End,
}

/// Shared between the session and the encoder's thread.
#[derive(Default)]
struct Shared {
    /// When the encoder entered the call it is in, if it is in one.
    busy: Mutex<Option<Instant>>,
    /// Packets the encoder produced.
    produced: AtomicU32,
}

impl Shared {
    fn set(&self, v: Option<Instant>) {
        *self.busy.lock().unwrap_or_else(|p| p.into_inner()) = v;
    }

    /// Runs one encoder call under the busy clock.
    fn call<T>(&self, f: impl FnOnce() -> T) -> T {
        self.set(Some(Instant::now()));
        let r = f();
        self.set(None);
        r
    }

    fn busy_for(&self) -> Duration {
        let at = *self.busy.lock().unwrap_or_else(|p| p.into_inner());
        at.map_or(Duration::ZERO, |t| t.elapsed())
    }
}

/// The encoder's thread and its channels.
struct Runner {
    tx: SyncSender<Cmd>,
    rx: Receiver<Evt>,
    shared: Arc<Shared>,
}

/// The thread's loop: opens the encoder, then serves commands until the session drops its
/// sender. Every exit drops the encoder here, on this thread.
fn serve(
    backend: Arc<dyn EncoderBackend>,
    cfg: EncoderConfig,
    cmds: Receiver<Cmd>,
    events: Sender<Evt>,
    shared: Arc<Shared>,
) {
    let mut enc: Box<dyn VideoEncoder> = match shared.call(|| backend.open(&cfg, OpenToken(()))) {
        Ok(e) => e,
        Err(e) => {
            let _ = events.send(Evt::Failed(e));
            return;
        }
    };
    if events.send(Evt::Opened).is_err() {
        return;
    }
    for cmd in cmds {
        let (r, end) = match cmd {
            Cmd::Frame(f, pts, key) => (shared.call(|| enc.send(&f, pts, key)), false),
            Cmd::Finish => (shared.call(|| enc.finish()), true),
            Cmd::Config(reply) => {
                let _ = reply.send(shared.call(|| enc.config()));
                continue;
            }
        };
        if let Err(e) = r {
            let _ = events.send(Evt::Failed(e));
            return;
        }
        loop {
            match shared.call(|| enc.receive()) {
                Ok(Some(p)) => {
                    shared.produced.fetch_add(1, Ordering::SeqCst);
                    if events.send(Evt::Packet(p)).is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = events.send(Evt::Failed(e));
                    return;
                }
            }
        }
        // The thread stays for `config` until the session drops its sender.
        if end && events.send(Evt::End).is_err() {
            return;
        }
    }
}

/// How starting a candidate went.
enum Start {
    Ready(Runner),
    Failed(CoreError),
    /// Opening it passed the deadline; its thread is abandoned.
    Hung,
}

impl Runner {
    fn start(backend: Arc<dyn EncoderBackend>, cfg: &EncoderConfig, deadline: Duration) -> Start {
        let (tx, cmds) = sync_channel::<Cmd>(2);
        let (events, rx) = channel::<Evt>();
        let shared = Arc::new(Shared::default());
        let (s, c) = (shared.clone(), cfg.clone());
        let spawned = std::thread::Builder::new()
            .name("fin-encoder".into())
            .spawn(move || serve(backend, c, cmds, events, s));
        if spawned.is_err() {
            return Start::Failed(CoreError::new(
                "ENCODE_FAILED",
                "could not start an encoder thread",
            ));
        }
        match rx.recv_timeout(deadline) {
            Ok(Evt::Opened) => Start::Ready(Runner { tx, rx, shared }),
            Ok(Evt::Failed(e)) => Start::Failed(e),
            Ok(_) | Err(RecvTimeoutError::Disconnected) => {
                Start::Failed(CoreError::new("ENCODE_FAILED", "the encoder did not open"))
            }
            Err(RecvTimeoutError::Timeout) => Start::Hung,
        }
    }
}

/// An open encoder, its policy entry, and the allowed candidates left for fallback.
pub struct EncoderSession {
    cfg: EncoderConfig,
    deadline: Duration,
    entry: &'static BackendEntry,
    hw: bool,
    runner: Runner,
    rest: VecDeque<Arc<dyn EncoderBackend>>,
    notes: Vec<FallbackNote>,
    hangs: u32,
    /// Frames accepted by the current encoder.
    frames: u32,
    /// Packets handed to the caller from the current encoder.
    packets: u32,
    finished: bool,
    ended: bool,
    /// The last packet, or `finish`.
    progress: Instant,
}

impl EncoderSession {
    /// Opens the first of `candidates` that opens (the policy has filtered them).
    pub(crate) fn start(
        cfg: EncoderConfig,
        mut candidates: VecDeque<Arc<dyn EncoderBackend>>,
    ) -> Result<EncoderSession, CoreError> {
        let deadline = cfg.hang_deadline();
        let mut notes = Vec::new();
        let mut hangs = 0;
        let (entry, hw, runner) = open_next(
            &cfg,
            deadline,
            &mut candidates,
            &mut notes,
            &mut hangs,
            None,
        )?;
        Ok(EncoderSession {
            deadline,
            cfg,
            entry,
            hw,
            runner,
            rest: candidates,
            notes,
            hangs,
            frames: 0,
            packets: 0,
            finished: false,
            ended: false,
            progress: Instant::now(),
        })
    }

    /// Shortens `ENCODER_HUNG`'s deadline for the encoders opened from now on, and for the
    /// current one's calls (tests).
    #[cfg(feature = "testing")]
    pub fn with_hang_deadline(mut self, d: Duration) -> Self {
        self.deadline = d;
        self
    }

    /// The backend now encoding.
    pub fn entry(&self) -> &'static BackendEntry {
        self.entry
    }

    pub fn hw(&self) -> bool {
        self.hw
    }

    /// Every move from one encoder to another, in order (empty when none was needed).
    pub fn fell_back(&self) -> &[FallbackNote] {
        &self.notes
    }

    /// The configuration record for the container (`av1C`, `avcC`), from the encoder now in
    /// use. Ask after `End` (encoders that build it from their output have it then). An
    /// encoder that hangs here, its work done, is `ENCODER_HUNG` without a fallback.
    pub fn config(&mut self) -> Result<CodecConfig, CoreError> {
        let (reply, answer) = sync_channel(1);
        if self.push(Cmd::Config(reply))? == Fed::Restart {
            return Err(self.hung_error());
        }
        answer
            .recv_timeout(self.deadline)
            .map_err(|_| self.hung_error())
    }

    /// Hands a frame to the encoder. Drain `receive` between sends (one packet comes out per
    /// frame).
    pub fn send(&mut self, frame: RawFrame, pts: i64, force_key: bool) -> Result<Fed, CoreError> {
        let produced = self.runner.shared.produced.load(Ordering::SeqCst);
        if self.frames.saturating_sub(produced) >= HUNG_FRAMES {
            return self.hang().map(|()| Fed::Restart);
        }
        let fed = self.push(Cmd::Frame(Box::new(frame), pts, force_key))?;
        if fed == Fed::Accepted {
            self.frames += 1;
        }
        Ok(fed)
    }

    /// Ends the stream: `receive` then waits for the remaining packets and gives `End`.
    pub fn finish(&mut self) -> Result<Fed, CoreError> {
        let fed = self.push(Cmd::Finish)?;
        if fed == Fed::Accepted {
            self.finished = true;
            self.progress = Instant::now();
        }
        Ok(fed)
    }

    /// The next packet: `Pending` while the encoder works (before `finish`); after `finish`
    /// it waits for one, or `End`.
    pub fn receive(&mut self) -> Result<Output, CoreError> {
        if self.ended {
            return Ok(Output::End);
        }
        loop {
            let got = if self.finished {
                match self.runner.rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(e) => Some(e),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return Err(self.failure()),
                }
            } else {
                match self.runner.rx.try_recv() {
                    Ok(e) => Some(e),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return Err(self.failure()),
                }
            };
            match got {
                Some(Evt::Packet(p)) => {
                    self.packets += 1;
                    self.progress = Instant::now();
                    return Ok(Output::Packet(p));
                }
                Some(Evt::Failed(e)) => return Err(e),
                Some(Evt::End) if self.packets < self.frames => {
                    return Err(CoreError::new(
                        "ENCODE_FAILED",
                        "the encoder ended with frames still missing",
                    )
                    .with_details(json!({
                        "backend": self.entry.name, "frames": self.frames, "packets": self.packets,
                    })));
                }
                Some(Evt::End) => {
                    self.ended = true;
                    return Ok(Output::End);
                }
                Some(Evt::Opened) => {}
                None => {
                    let stalled = self.finished && self.progress.elapsed() > self.deadline;
                    if stalled || self.runner.shared.busy_for() > self.deadline {
                        return self.hang().map(|()| Output::Restart);
                    }
                    if !self.finished {
                        return Ok(Output::Pending);
                    }
                }
            }
        }
    }

    /// Queues a command; an encoder stuck in a call past the deadline is a hang (`Restart`).
    fn push(&mut self, cmd: Cmd) -> Result<Fed, CoreError> {
        let mut cmd = cmd;
        loop {
            match self.runner.tx.try_send(cmd) {
                Ok(()) => return Ok(Fed::Accepted),
                Err(TrySendError::Full(c)) => {
                    if self.runner.shared.busy_for() > self.deadline {
                        return self.hang().map(|()| Fed::Restart);
                    }
                    cmd = c;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(TrySendError::Disconnected(_)) => return Err(self.failure()),
            }
        }
    }

    /// The thread is gone: the error it sent, else a generic failure.
    fn failure(&mut self) -> CoreError {
        while let Ok(e) = self.runner.rx.try_recv() {
            if let Evt::Failed(e) = e {
                return e;
            }
        }
        CoreError::new("ENCODE_FAILED", "the encoder stopped unexpectedly")
            .with_details(json!({ "backend": self.entry.name }))
    }

    fn hung_error(&self) -> CoreError {
        hung(self.entry.name, self.hw, &self.cfg)
    }

    /// Abandons the current encoder and opens the next candidate (`Ok`), or fails with
    /// `ENCODER_HUNG` on the second hang or when none is left.
    fn hang(&mut self) -> Result<(), CoreError> {
        self.hangs += 1;
        let err = self.hung_error();
        if self.hangs >= MAX_HANGS || self.rest.is_empty() {
            return Err(err);
        }
        let from = self.entry.name;
        let (entry, hw, runner) = open_next(
            &self.cfg,
            self.deadline,
            &mut self.rest,
            &mut self.notes,
            &mut self.hangs,
            Some(from),
        )
        .map_err(|e| if e.code == "ENCODER_HUNG" { e } else { err })?;
        // Replacing the runner drops the hung one's channels; its thread is never joined.
        self.runner = runner;
        self.entry = entry;
        self.hw = hw;
        self.frames = 0;
        self.packets = 0;
        self.finished = false;
        self.ended = false;
        self.progress = Instant::now();
        Ok(())
    }
}

/// `ENCODER_HUNG` for `backend`; `adapter` names the GPU only for a hardware encoder.
fn hung(backend: &str, hw: bool, cfg: &EncoderConfig) -> CoreError {
    let adapter = cfg.adapter.filter(|_| hw).map(|a| json!(a.id));
    CoreError::new(
        "ENCODER_HUNG",
        format!("the {backend} encoder stopped producing output"),
    )
    .with_details(json!({ "backend": backend, "adapter": adapter }))
    .with_fix("the job can be retried; a GPU driver update may help")
}

/// Opens the first of `candidates` that opens in time, noting each move (`after` is the
/// encoder replaced after a hang). An open that passes the deadline counts as a hang.
fn open_next(
    cfg: &EncoderConfig,
    deadline: Duration,
    candidates: &mut VecDeque<Arc<dyn EncoderBackend>>,
    notes: &mut Vec<FallbackNote>,
    hangs: &mut u32,
    after: Option<&'static str>,
) -> Result<(&'static BackendEntry, bool, Runner), CoreError> {
    let mut passed: Vec<(&'static str, FallbackReason)> = after
        .map(|a| (a, FallbackReason::Hung))
        .into_iter()
        .collect();
    let mut last = None;
    while let Some(c) = candidates.pop_front() {
        let name = c.entry().name;
        match Runner::start(c.clone(), cfg, deadline) {
            Start::Ready(runner) => {
                for (from, reason) in passed {
                    notes.push(FallbackNote {
                        from,
                        to: name,
                        reason,
                    });
                }
                return Ok((c.entry(), c.hw(), runner));
            }
            Start::Failed(e) => {
                passed.push((name, FallbackReason::OpenFailed));
                last = Some(e);
            }
            Start::Hung => {
                *hangs += 1;
                let e = hung(name, c.hw(), cfg);
                if *hangs >= MAX_HANGS {
                    return Err(e);
                }
                passed.push((name, FallbackReason::Hung));
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| {
        crate::registry::unavailable(&crate::registry::codec_name(cfg.codec), "encode")
    }))
}
