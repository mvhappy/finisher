//! The local diagnostics and training-data journal (spec §21.1; plan D, Decisions 15–21): an
//! append-only, capped, crash-safe set of JSONL files in `<server-data>/events/`, written by
//! one thread per server data, that never blocks a caller and costs one atomic load when off.
//!
//! - [`Journal::record`] stamps an entry (time, context, the consent in force) and hands it
//!   to the writer over a channel bounded by entries and by bytes; a full channel drops it
//!   and counts the drop, which the writer reports as `journal.dropped`.
//! - Control messages (synchronous records, flushes, close) take an unbounded channel that
//!   never drops; the writer drains it before each data batch.
//! - The writer serialises, redacts content the stamp ∩ the gate does not allow, rotates at
//!   [`JournalOptions::max_file_bytes`] or a new UTC day, applies the age and size caps, and
//!   suspends (retrying every 60 s) on an I/O error or a panic of its own.
//!
//! No network: `fin-journal` depends on `fin-core` only among Finisher's crates.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::mpsc::{self, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use fin_core::CoreError;
use fin_core::events::{
    AnyPayload, Basis, Code, Ctx, IdRef, Name, Purposes, Source, require_kind, validate_payload,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ulid::Ulid;

mod clock;
mod control;
mod encode;
mod faults;
mod files;
mod read;
mod stats;
mod writer;

pub use clock::{Clock, ManualClock, SystemClock};
pub use faults::Faults;
pub use read::{Entries, Filter};
pub use stats::MAX_KEYS;

use control::{
    Control, Data, Pending, SAMPLE_BYTES, Sample, Stamp, SyncSlot, gate_of, pack, unpack,
};

/// A serialised entry larger than this is refused (`EVENT_TOO_LARGE`) and counted as dropped.
pub const MAX_ENTRY_BYTES: usize = 64 * 1024;

/// The effective consent (D3's `resolve` makes it): the purposes in force, the consent
/// revision and the weaker basis of the two purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent {
    pub purposes: Purposes,
    pub rev: u64,
    pub basis: Basis,
}

impl Consent {
    /// Nothing recorded.
    pub const OFF: Consent = Consent {
        purposes: Purposes::NONE,
        rev: 0,
        basis: Basis::Default,
    };
}

/// The journal's limits (Decisions 16, 18, 19). [`Default`] is the shipped configuration.
#[derive(Clone)]
pub struct JournalOptions {
    /// A file rotates before it would grow past this (8 MiB).
    pub max_file_bytes: u64,
    /// The size cap over every `j-*` file (128 MiB; settings `maxMiB`).
    pub max_total_bytes: u64,
    /// The age cap (30 days; settings `retentionDays`).
    pub max_age: Duration,
    /// Data channel capacity in entries (8,192).
    pub channel: usize,
    /// Data channel capacity in `approx_bytes` in flight (8 MiB).
    pub channel_bytes: usize,
    /// The writer flushes this often (1 s) or at `flush_bytes` buffered (64 KiB).
    pub flush_every: Duration,
    pub flush_bytes: usize,
    /// The stats window (1 minute).
    pub stats_every: Duration,
    pub clock: Arc<dyn Clock>,
}

impl Default for JournalOptions {
    fn default() -> Self {
        JournalOptions {
            max_file_bytes: 8 << 20,
            max_total_bytes: 128 << 20,
            max_age: Duration::from_secs(30 * 86_400),
            channel: 8192,
            channel_bytes: 8 << 20,
            flush_every: Duration::from_secs(1),
            flush_bytes: 64 << 10,
            stats_every: Duration::from_secs(60),
            clock: Arc::new(SystemClock),
        }
    }
}

impl fmt::Debug for JournalOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalOptions")
            .field("max_file_bytes", &self.max_file_bytes)
            .field("max_total_bytes", &self.max_total_bytes)
            .field("max_age", &self.max_age)
            .field("channel", &self.channel)
            .field("channel_bytes", &self.channel_bytes)
            .field("flush_every", &self.flush_every)
            .field("flush_bytes", &self.flush_bytes)
            .field("stats_every", &self.stats_every)
            .finish_non_exhaustive()
    }
}

/// A high-rate timing aggregated into per-window histograms (Decision 19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Metric {
    /// `rpc.stats`, keyed by `RPC_METHODS`.
    Rpc,
    /// `eval.stats`, keyed by `EVAL_KEYS`.
    Eval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JournalState {
    Off,
    On,
    Suspended,
}

/// What the journal holds and whether it is writing (`fin diagnostics show`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JournalStatus {
    pub state: JournalState,
    // Why it is suspended: `JOURNAL_UNWRITABLE`, `JOURNAL_DISK_FULL`, `JOURNAL_IO` or
    // `JOURNAL_PANIC`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub code: Option<String>,
    pub files: u64,
    pub bytes: u64,
    /// Entries dropped since the journal opened.
    pub dropped: u64,
    /// The oldest entry's `ts` and the newest's (ms since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub oldest: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub newest: Option<u64>,
}

/// A journal call that could not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalError {
    pub code: &'static str,
    pub message: String,
}

impl JournalError {
    fn new(code: &'static str, message: impl Into<String>) -> JournalError {
        JournalError {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for JournalError {}

impl From<JournalError> for CoreError {
    fn from(e: JournalError) -> CoreError {
        CoreError::new(e.code, e.message)
    }
}

#[derive(Debug, Default)]
struct Snapshot {
    suspended: Option<&'static str>,
    files: u64,
    bytes: u64,
    oldest: Option<u64>,
    newest: Option<u64>,
}

/// What the callers and the writer share.
pub(crate) struct Shared {
    /// The gate and the stamp, packed (`control::pack`).
    word: AtomicU64,
    inflight: AtomicUsize,
    /// Drops not yet reported, and all drops.
    drop_count: AtomicU64,
    drop_bytes: AtomicU64,
    dropped_total: AtomicU64,
    status: Mutex<Snapshot>,
    /// The process run (a ULID): every entry's `session` unless `record_value` names another.
    session: String,
    opts: JournalOptions,
    faults: Faults,
}

impl Shared {
    fn dropped(&self, bytes: usize) {
        self.drop_count.fetch_add(1, Relaxed);
        self.drop_bytes.fetch_add(bytes as u64, Relaxed);
        self.dropped_total.fetch_add(1, Relaxed);
    }

    fn lock_status(&self) -> MutexGuard<'_, Snapshot> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct Handle {
    sh: Arc<Shared>,
    dir: PathBuf,
    data: SyncSender<Data>,
    ctl: Sender<Control>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Handle {
    /// Queues a data message unless either bound is reached.
    fn send(&self, d: Data, bytes: usize) {
        let limit = self.sh.opts.channel_bytes;
        if self.sh.inflight.fetch_add(bytes, Relaxed) + bytes > limit {
            self.sh.inflight.fetch_sub(bytes, Relaxed);
            self.sh.dropped(bytes);
            return;
        }
        if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = self.data.try_send(d) {
            self.sh.inflight.fetch_sub(bytes, Relaxed);
            self.sh.dropped(bytes);
        }
    }

    /// Sends a control message and wakes the writer if it is waiting for data. A full data
    /// channel means the writer is busy and drains control before its next batch.
    fn control(&self, c: Control) -> bool {
        let sent = self.ctl.send(c).is_ok();
        let _ = self.data.try_send(Data::Wake);
        sent
    }

    fn pending(&self, source: Source, ctx: Option<Ctx>, p: AnyPayload, word: u64) -> Pending {
        Pending {
            ts: self.sh.opts.clock.now_ms(),
            session: None,
            source,
            bytes: encode::entry_bytes(ctx.as_ref(), &p),
            ctx,
            payload: p,
            stamp: unpack(word),
        }
    }

    fn flush(&self, timeout: Duration) -> Result<(), JournalError> {
        if Journal::on_writer_thread() {
            return Err(JournalError::new(
                "JOURNAL_BUSY",
                "flush from the writer thread",
            ));
        }
        let (ack, rx) = mpsc::sync_channel(1);
        if !self.control(Control::Flush(ack)) {
            return Err(JournalError::new(
                "JOURNAL_UNAVAILABLE",
                "the journal is closed",
            ));
        }
        rx.recv_timeout(timeout).unwrap_or_else(|_| {
            Err(JournalError::new(
                "JOURNAL_TIMEOUT",
                "the journal writer did not answer",
            ))
        })
    }
}

/// The journal of one server data. Cloning shares it; the writer stops at [`Journal::close`]
/// or when the last clone is dropped.
#[derive(Clone)]
pub struct Journal {
    h: Option<Arc<Handle>>,
}

impl fmt::Debug for Journal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Journal")
            .field("gate", &self.gate())
            .finish_non_exhaustive()
    }
}

fn is_ulid(s: &str) -> bool {
    IdRef::classify(s).is_ok()
}

impl Journal {
    /// Opens the journal in `dir` (`<server-data>/events`) under `consent`, already resolved
    /// from the settings and the environment. Never fails: an unusable directory suspends
    /// the journal. The directory is created only by the first write.
    pub fn open(dir: &Path, consent: Consent, opts: JournalOptions) -> Journal {
        let now = opts.clock.now_ms();
        let session = Ulid::from_parts(now, Ulid::new().random()).to_string();
        let (data_tx, data_rx) = mpsc::sync_channel(opts.channel.max(1));
        let (ctl_tx, ctl_rx) = mpsc::channel();
        let sh = Arc::new(Shared {
            word: AtomicU64::new(pack(Stamp {
                purposes: consent.purposes,
                rev: consent.rev,
                basis: consent.basis,
            })),
            inflight: AtomicUsize::new(0),
            drop_count: AtomicU64::new(0),
            drop_bytes: AtomicU64::new(0),
            dropped_total: AtomicU64::new(0),
            status: Mutex::new(Snapshot::default()),
            session,
            opts,
            faults: Faults::default(),
        });
        let w = writer::Writer::new(sh.clone(), dir.to_path_buf(), data_rx, ctl_rx);
        let Ok(thread) = std::thread::Builder::new()
            .name("fin-journal".into())
            .spawn(move || w.run())
        else {
            return Journal::disabled();
        };
        Journal {
            h: Some(Arc::new(Handle {
                sh,
                dir: dir.to_path_buf(),
                data: data_tx,
                ctl: ctl_tx,
                thread: Mutex::new(Some(thread)),
            })),
        }
    }

    /// A journal that records nothing and touches nothing.
    pub fn disabled() -> Journal {
        Journal { h: None }
    }

    /// The purposes in force, or `None` when off: one `Relaxed` load. Call sites check it
    /// before they build a payload.
    #[inline]
    pub fn gate(&self) -> Option<Purposes> {
        let p = gate_of(self.h.as_ref()?.sh.word.load(Relaxed));
        (!p.is_empty()).then_some(p)
    }

    /// The process run this journal stamps its entries with.
    pub fn session(&self) -> Option<&str> {
        self.h.as_ref().map(|h| h.sh.session.as_str())
    }

    /// Queues an entry. Never blocks: a full channel drops it (and counts the drop).
    pub fn record(&self, source: Source, ctx: Option<Ctx>, p: AnyPayload) {
        let Some(h) = &self.h else { return };
        let word = h.sh.word.load(Relaxed);
        if gate_of(word).is_empty() {
            return;
        }
        let pending = h.pending(source, ctx, p, word);
        let bytes = pending.bytes;
        h.send(Data::Entry(Box::new(pending)), bytes);
    }

    /// Decodes a received payload ([`validate_payload`]) and queues it under `session` (the
    /// sender's run, set by the server from the connection). `EVENT_KIND_UNKNOWN`,
    /// `EVENT_INVALID` (also for a kind `source` does not emit, or a session that is not a
    /// ULID) or `EVENT_TOO_LARGE`. Off, it decodes nothing and returns `Ok`.
    pub fn record_value(
        &self,
        source: Source,
        session: &str,
        kind: &str,
        kv: u16,
        ctx: Option<Ctx>,
        payload: Value,
    ) -> Result<(), CoreError> {
        let Some(h) = &self.h else { return Ok(()) };
        let word = h.sh.word.load(Relaxed);
        if gate_of(word).is_empty() {
            return Ok(());
        }
        let spec = require_kind(kind, kv)?;
        let invalid = |m: String| {
            CoreError::new("EVENT_INVALID", m).with_details(json!({ "kind": kind, "kv": kv }))
        };
        if !spec.sources.contains(&source) {
            return Err(invalid(format!("{kind} is not emitted by {source:?}")));
        }
        if !is_ulid(session) {
            return Err(invalid("the session is not a ULID".into()));
        }
        let p = validate_payload(spec, &payload)?;
        let mut pending = h.pending(source, ctx, p, word);
        if pending.bytes > MAX_ENTRY_BYTES {
            return Err(CoreError::new(
                "EVENT_TOO_LARGE",
                format!("the {kind} entry is over {MAX_ENTRY_BYTES} bytes"),
            )
            .with_details(json!({ "kind": kind, "bytes": pending.bytes })));
        }
        pending.session = Some(session.into());
        let bytes = pending.bytes;
        h.send(Data::Entry(Box::new(pending)), bytes);
        Ok(())
    }

    /// Adds one timing to the current window of `(metric, key)` (Decision 19).
    pub fn observe(&self, metric: Metric, key: Name, micros: u64, code: Option<Code>) {
        let Some(h) = &self.h else { return };
        if gate_of(h.sh.word.load(Relaxed)).is_empty() {
            return;
        }
        let s = Sample {
            ts: h.sh.opts.clock.now_ms(),
            metric,
            key,
            micros,
            code,
        };
        h.send(Data::Sample(Box::new(s)), SAMPLE_BYTES);
    }

    /// Writes and flushes an entry before returning (`error.panic`, `session.end`), waiting
    /// at most `timeout`. False when it is not known to be on disk: off, the writer stalled
    /// (counted as a drop), suspended, or called on the writer thread (it would wait on
    /// itself).
    pub fn record_sync(
        &self,
        source: Source,
        ctx: Option<Ctx>,
        p: AnyPayload,
        timeout: Duration,
    ) -> bool {
        let Some(h) = &self.h else { return false };
        let word = h.sh.word.load(Relaxed);
        if gate_of(word).is_empty() || Journal::on_writer_thread() {
            return false;
        }
        let entry = Box::new(h.pending(source, ctx, p, word));
        let bytes = entry.bytes;
        let slot = Arc::new(SyncSlot::default());
        let (ack, rx) = mpsc::sync_channel(1);
        let msg = Control::Sync {
            entry,
            slot: slot.clone(),
            ack,
        };
        if !h.control(msg) {
            h.sh.dropped(bytes);
            return false;
        }
        match rx.recv_timeout(timeout) {
            Ok(on_disk) => on_disk,
            Err(_) => {
                if slot.cancel() {
                    h.sh.dropped(bytes);
                }
                false
            }
        }
    }

    /// True on the journal's writer thread: a panic hook there must not call
    /// [`Journal::record_sync`].
    pub fn on_writer_thread() -> bool {
        writer::ON_WRITER.with(|w| w.get())
    }

    /// Writes everything queued so far to the files (no fsync). `JOURNAL_UNAVAILABLE` while
    /// suspended or closed, `JOURNAL_TIMEOUT` when the writer does not answer in 10 s.
    pub fn flush(&self) -> Result<(), JournalError> {
        match &self.h {
            Some(h) => h.flush(Duration::from_secs(10)),
            None => Ok(()),
        }
    }

    /// The journal's state, after a short flush so the counts include what was queued.
    pub fn status(&self) -> JournalStatus {
        let Some(h) = &self.h else {
            return JournalStatus {
                state: JournalState::Off,
                code: None,
                files: 0,
                bytes: 0,
                dropped: 0,
                oldest: None,
                newest: None,
            };
        };
        let _ = h.flush(Duration::from_secs(1));
        let s = h.sh.lock_status();
        let state = match (self.gate(), s.suspended) {
            (None, _) => JournalState::Off,
            (Some(_), Some(_)) => JournalState::Suspended,
            (Some(_), None) => JournalState::On,
        };
        JournalStatus {
            state,
            code: s.suspended.map(str::to_owned),
            files: s.files,
            bytes: s.bytes,
            dropped: h.sh.dropped_total.load(Relaxed),
            oldest: s.oldest,
            newest: s.newest,
        }
    }

    /// The entries matching `filter`, oldest first, after a flush. Lines that do not parse
    /// are skipped and counted ([`Entries::skipped`]).
    pub fn read(&self, filter: &Filter) -> Entries {
        let Some(h) = &self.h else {
            return Entries::empty();
        };
        let _ = h.flush(Duration::from_secs(10));
        Entries::new(files::paths(&h.dir), filter)
    }

    /// Drains the queue, flushes, fsyncs and stops the writer (waiting at most 5 s for it).
    /// Other clones record nothing afterwards.
    pub fn close(self) {
        let Some(h) = self.h else { return };
        let (ack, rx) = mpsc::sync_channel(1);
        if h.control(Control::Close(ack)) && rx.recv_timeout(Duration::from_secs(5)).is_ok() {
            let t = h.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(t) = t {
                let _ = t.join();
            }
        }
    }

    /// The fault hooks (feature `test-hooks`); `None` when disabled.
    #[cfg(feature = "test-hooks")]
    pub fn faults(&self) -> Option<&Faults> {
        self.h.as_ref().map(|h| &h.sh.faults)
    }

    /// Parks the writer before its next message (feature `test-hooks`); true once it is.
    #[cfg(feature = "test-hooks")]
    pub fn pause_writer(&self) -> bool {
        let Some(h) = &self.h else { return false };
        h.sh.faults.request_pause();
        let _ = h.data.try_send(Data::Wake);
        h.sh.faults.wait_parked(Duration::from_secs(5))
    }

    #[cfg(feature = "test-hooks")]
    pub fn resume_writer(&self) {
        if let Some(h) = &self.h {
            h.sh.faults.resume();
        }
    }
}
