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
use std::path::Path;
use std::sync::atomic::Ordering::{Acquire, Relaxed};
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use fin_core::CoreError;
use fin_core::events::Ulid as EventUlid;
use fin_core::events::{
    AnyPayload, Code, Ctx, Name, Purposes, Source, WIRE_KINDS, decode_payload, require_kind,
    validate_payload,
};
use serde_json::{Value, json};
use ulid::Ulid;

mod blobs;
mod clock;
mod consent;
mod control;
mod disk;
mod dispatch;
mod encode;
mod export;
mod faults;
mod files;
mod handle;
mod key;
mod options;
mod projects;
mod purge;
mod read;
mod report;
mod session;
mod state;
mod stats;
mod status;
mod writer;

pub use blobs::MAX_BLOB_BYTES;
pub use clock::{Clock, ManualClock, SystemClock};
pub use consent::{
    ConsentState, DEFAULTS, Defaults, EnvOverrides, PrivacySettings, Switch, TrainingState,
    resolve, resolve_state,
};
pub use export::Export;
#[cfg(feature = "test-hooks")]
pub use faults::Faults;
pub use fin_core::events::CollectPolicy;
pub use options::{JournalOptions, KeyFn, server_scrub};
pub use projects::{ProjectPolicy, ProjectScope};
pub use purge::{PurgeReport, PurgeSelector};
pub use read::{Entries, Filter};
pub use report::{DiagnosticsReport, ExportPage, MAX_PAGE};
pub use stats::MAX_KEYS;
pub use status::{Consent, JournalError, JournalState, JournalStatus, Metric};

#[cfg(not(feature = "test-hooks"))]
use faults::Faults;
use handle::{Handle, Shared};
use status::Snapshot;

use control::{Control, Data, SAMPLE_BYTES, Sample, Stamp, gate_of, pack};

/// A serialised entry larger than this is refused (`EVENT_TOO_LARGE`) and counted as dropped.
pub const MAX_ENTRY_BYTES: usize = 64 * 1024;

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

impl Journal {
    /// Opens the journal in `dir` (`<server-data>/events`) under `consent`, already resolved
    /// from the settings and the environment ([`Journal::open_with_settings`] does both).
    /// Never fails: an unusable directory suspends the journal. The directory is created
    /// only by the first write. The consent's `rev` is a floor: `state.json` numbers it.
    pub fn open(dir: &Path, consent: Consent, opts: JournalOptions) -> Journal {
        let ctl = handle::ConsentCtl {
            env: consent::EnvOverrides::default(),
            last_good: None,
            state: consent::ConsentState::of(consent),
            base_age_ms: opts.max_age.as_millis() as u64,
            base_total: opts.max_total_bytes,
        };
        Journal::open_ctl(dir, ctl, opts)
    }

    pub(crate) fn open_ctl(
        dir: &Path,
        mut ctl: handle::ConsentCtl,
        opts: JournalOptions,
    ) -> Journal {
        let now = opts.clock.now_ms();
        let session = EventUlid::from_u128(Ulid::from_parts(now, Ulid::new().random()).0);
        let (data_tx, data_rx) = mpsc::sync_channel(opts.channel.max(1));
        let (ctl_tx, ctl_rx) = mpsc::channel();
        // Reading only: the revision continues what `state.json` recorded.
        let stored = state::read(dir);
        ctl.state.rev = state::reconcile(
            stored.as_ref().map(|s| &s.consent),
            &ctl.state,
            ctl.state.rev,
        );
        let dirty = stored.as_ref().is_none_or(|s| s.consent != ctl.state);
        let init = writer::Init {
            state: ctl.state,
            dirty,
            other: stored.map(|s| s.other).unwrap_or_default(),
        };
        let state = ctl.state;
        let key = (!state.purposes().is_empty())
            .then(|| key::Key::load_or_generate(dir).map(Arc::new))
            .flatten();
        let (age, total) = (ctl.base_age_ms, ctl.base_total);
        let (age, total) = consent::caps_of(ctl.last_good.as_ref(), age, total);
        let sh = Arc::new(Shared {
            word: AtomicU64::new(pack(Stamp {
                purposes: state.purposes(),
                rev: state.rev,
                basis: state.basis,
            })),
            inflight: AtomicUsize::new(0),
            sent: AtomicU64::new(0),
            drop_count: AtomicU64::new(0),
            drop_bytes: AtomicU64::new(0),
            dropped_total: AtomicU64::new(0),
            skipped_unknown: AtomicU64::new(0),
            scopes: Mutex::new(Default::default()),
            status: Mutex::new(Snapshot::default()),
            session,
            opts,
            #[allow(clippy::default_constructed_unit_structs)] // a unit struct without test-hooks
            faults: Faults::default(),
            key: RwLock::new(key),
            consent: Mutex::new(ctl),
            caps_age: AtomicU64::new(age),
            caps_total: AtomicU64::new(total),
        });
        let w = writer::Writer::new(sh.clone(), dir.to_path_buf(), data_rx, ctl_rx, init);
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
                waker: thread.thread().clone(),
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
        let word = h.sh.word.load(Acquire);
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
        let Some((h, pending)) = self.prepare_value(source, session, kind, kv, ctx, payload)?
        else {
            return Ok(());
        };
        let bytes = pending.bytes;
        h.send(Data::Entry(Box::new(pending)), bytes);
        Ok(())
    }

    /// [`Journal::record_value`], written and flushed before it returns, waiting at most
    /// `timeout` ([`Journal::record_sync`]'s rules): a received or server-built
    /// `error.panic`. `Ok(false)` when it is not known to be on disk (off included).
    #[allow(clippy::too_many_arguments)] // `record_value`'s, plus the timeout
    pub fn record_value_sync(
        &self,
        source: Source,
        session: &str,
        kind: &str,
        kv: u16,
        ctx: Option<Ctx>,
        payload: Value,
        timeout: Duration,
    ) -> Result<bool, CoreError> {
        if Journal::on_writer_thread() {
            return Ok(false);
        }
        let Some((h, pending)) = self.prepare_value(source, session, kind, kv, ctx, payload)?
        else {
            return Ok(false);
        };
        Ok(h.sync(Box::new(pending), timeout))
    }

    /// The decoded, stamped entry of a received payload (`None` while off).
    fn prepare_value(
        &self,
        source: Source,
        session: &str,
        kind: &str,
        kv: u16,
        ctx: Option<Ctx>,
        payload: Value,
    ) -> Result<Option<(&Arc<Handle>, control::Pending)>, CoreError> {
        let Some(h) = &self.h else { return Ok(None) };
        let word = h.sh.word.load(Acquire);
        if gate_of(word).is_empty() {
            return Ok(None);
        }
        let spec = require_kind(kind, kv)?;
        let invalid = |m: String| {
            CoreError::new("EVENT_INVALID", m).with_details(json!({ "kind": kind, "kv": kv }))
        };
        if !spec.sources.contains(&source) {
            return Err(invalid(format!("{kind} is not emitted by {source:?}")));
        }
        let Some(session) = EventUlid::parse(session) else {
            return Err(invalid("the session is not a ULID".into()));
        };
        let opts = &h.sh.opts;
        let p = if WIRE_KINDS.contains(&spec.name) {
            // A wire form keys its ids and hashes with the install key (held whenever a
            // purpose is on; refused rather than keyed with a stand-in if it is not).
            let Some(key) = h.sh.hasher() else {
                return Err(invalid(format!("{kind} needs the install key")));
            };
            validate_payload(spec, &payload, &*key, &opts.scrub)?
        } else {
            // Not a wire kind: decoded as stored and scrubbed again; no key is involved.
            let mut p = decode_payload(spec, &payload)?;
            p.rescrub(&opts.scrub);
            p
        };
        let mut pending = h.pending(source, ctx, p, word);
        if pending.bytes > MAX_ENTRY_BYTES {
            return Err(CoreError::new(
                "EVENT_TOO_LARGE",
                format!("the {kind} entry is over {MAX_ENTRY_BYTES} bytes"),
            )
            .with_details(json!({ "kind": kind, "bytes": pending.bytes })));
        }
        pending.session = Some(session);
        Ok(Some((h, pending)))
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
        let word = h.sh.word.load(Acquire);
        if gate_of(word).is_empty() || Journal::on_writer_thread() {
            return false;
        }
        h.sync(Box::new(h.pending(source, ctx, p, word)), timeout)
    }

    /// True on the journal's writer thread: a panic hook there must not call
    /// [`Journal::record_sync`].
    pub fn on_writer_thread() -> bool {
        writer::ON_WRITER.with(|w| w.get())
    }

    /// Writes everything queued so far to the files (no fsync). `JOURNAL_UNAVAILABLE` while
    /// suspended or closed, `JOURNAL_TIMEOUT` when the writer does not answer in 10 s. Blocks
    /// until then: call it off the JS thread.
    pub fn flush(&self) -> Result<(), JournalError> {
        match &self.h {
            Some(h) => h.flush(Duration::from_secs(10)),
            None => Ok(()),
        }
    }

    /// The journal's state, after a flush so the counts include what was queued. Blocks for
    /// up to a second on a stalled writer: call it off the JS thread, or use
    /// [`Journal::status_now`].
    pub fn status(&self) -> JournalStatus {
        if let Some(h) = &self.h {
            let _ = h.flush(Duration::from_secs(1));
        }
        self.status_now()
    }

    /// The journal's state as the writer last published it, without waiting: entries still
    /// queued are not counted yet.
    pub fn status_now(&self) -> JournalStatus {
        let Some(h) = &self.h else {
            return JournalStatus {
                state: JournalState::Off,
                code: None,
                files: 0,
                bytes: 0,
                blobs: 0,
                blob_bytes: 0,
                dropped: 0,
                skipped_unknown_project: 0,
                oldest: None,
                newest: None,
            };
        };
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
            blobs: s.blobs,
            blob_bytes: s.blob_bytes,
            dropped: h.sh.dropped_total.load(Relaxed),
            skipped_unknown_project: h.sh.skipped_unknown.load(Relaxed),
            oldest: s.oldest,
            newest: s.newest,
        }
    }

    /// The entries matching `filter`, oldest first, after a flush. Lines that do not parse
    /// are skipped and counted ([`Entries::skipped`]). Blocks on the flush (up to 10 s with a
    /// stalled writer) and reads the files: call it off the JS thread.
    pub fn read(&self, filter: &Filter) -> Entries {
        let Some(h) = &self.h else {
            return Entries::empty();
        };
        let _ = h.flush(Duration::from_secs(10));
        Entries::new(files::paths(&h.dir), filter)
    }

    /// Drains the queue, flushes, fsyncs and stops the writer (waiting at most 5 s for it).
    /// Other clones record nothing afterwards. Blocks until then: call it off the JS thread.
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
        h.send_raw(Data::Wake);
        h.wake();
        h.sh.faults.wait_parked(Duration::from_secs(5))
    }

    /// Stores a new consent in the gate word only, as a consent change does first (feature
    /// `test-hooks`): the writer learns of it at its next write or flush.
    #[cfg(feature = "test-hooks")]
    pub fn set_gate_for_test(&self, c: Consent) {
        if let Some(h) = &self.h {
            let stamp = control::Stamp {
                purposes: c.purposes,
                rev: c.rev,
                basis: c.basis,
            };
            h.sh.word
                .store(pack(stamp), std::sync::atomic::Ordering::Release);
        }
    }

    #[cfg(feature = "test-hooks")]
    pub fn resume_writer(&self) {
        if let Some(h) = &self.h {
            h.sh.faults.resume();
        }
    }
}
