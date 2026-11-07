//! What the callers and the writer share ([`Shared`]), and the callers' side of the channels
//! ([`Handle`]): the gate word, the bounds and drop counters, the published status, the
//! install key and the consent the settings resolved to.

use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use fin_core::events::Ulid as EventUlid;
use fin_core::events::{AnyPayload, Ctx, Source};

use crate::consent::{ConsentState, EnvOverrides, PrivacySettings};
use crate::control::{Control, Data, Pending, unpack};
use crate::faults::Faults;
use crate::key::{self, Key};
use crate::options::KeyFn;
use crate::status::Snapshot;
use crate::{Journal, JournalError, JournalOptions, encode};

/// What `apply_settings` resolves against: the environment read at open, the last good
/// `privacy` settings, the consent in force, and the caps the options set.
#[derive(Debug)]
pub(crate) struct ConsentCtl {
    pub env: EnvOverrides,
    pub last_good: Option<PrivacySettings>,
    pub state: ConsentState,
    pub base_age_ms: u64,
    pub base_total: u64,
}

/// What the callers and the writer share.
pub(crate) struct Shared {
    /// The gate and the stamp, packed (`control::pack`).
    pub(crate) word: AtomicU64,
    pub(crate) inflight: AtomicUsize,
    /// Data messages queued so far (the writer counts those it handled).
    pub(crate) sent: AtomicU64,
    /// Drops not yet reported, and all drops.
    pub(crate) drop_count: AtomicU64,
    pub(crate) drop_bytes: AtomicU64,
    pub(crate) dropped_total: AtomicU64,
    /// Entries not written because their project was unknown (not a drop).
    pub(crate) skipped_unknown: AtomicU64,
    /// Open [`crate::ProjectScope`]s per project id.
    pub(crate) scopes: Mutex<std::collections::HashMap<String, usize>>,
    pub(crate) status: Mutex<Snapshot>,
    /// The process run (a ULID): every entry's `session` unless `record_value` names another.
    pub(crate) session: EventUlid,
    pub(crate) opts: JournalOptions,
    pub(crate) faults: Faults,
    /// The install key: loaded or made before a purpose turns on, replaced by `purge all`.
    pub(crate) key: RwLock<Option<Arc<Key>>>,
    pub(crate) consent: Mutex<ConsentCtl>,
    /// The caps in force (ms, bytes): the options, or the settings' `retentionDays`/`maxMiB`.
    pub(crate) caps_age: AtomicU64,
    pub(crate) caps_total: AtomicU64,
}

impl Shared {
    pub(crate) fn dropped(&self, bytes: usize) {
        self.drop_count.fetch_add(1, Relaxed);
        self.drop_bytes.fetch_add(bytes as u64, Relaxed);
        self.dropped_total.fetch_add(1, Relaxed);
    }

    pub(crate) fn lock_status(&self) -> MutexGuard<'_, Snapshot> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn lock_consent(&self) -> MutexGuard<'_, ConsentCtl> {
        self.consent.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn key(&self) -> Option<Arc<Key>> {
        self.key.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The 16-byte keyed hash in use: the options' override, else the install key.
    pub(crate) fn hasher(&self) -> Option<KeyFn> {
        key::hasher(self.opts.key.as_ref(), self.key())
    }

    /// Loads the key in `dir`, or makes one in memory (the writer persists it before the
    /// first line), unless one is held already.
    pub(crate) fn ensure_key(&self, dir: &std::path::Path) {
        let mut k = self.key.write().unwrap_or_else(|e| e.into_inner());
        if k.is_none() {
            *k = Key::load_or_generate(dir).map(Arc::new);
        }
    }

    /// After `purge all`: a new key in memory while on, none while off.
    pub(crate) fn reset_key(&self, on: bool) {
        let mut k = self.key.write().unwrap_or_else(|e| e.into_inner());
        *k = if on {
            Key::generate().map(Arc::new)
        } else {
            None
        };
    }
}

pub(crate) struct Handle {
    pub(crate) sh: Arc<Shared>,
    pub(crate) dir: PathBuf,
    pub(crate) data: SyncSender<Data>,
    /// Each control message with the count of data messages queued before it.
    pub(crate) ctl: Sender<(u64, Control)>,
    pub(crate) thread: Mutex<Option<JoinHandle<()>>>,
}

impl Handle {
    /// Queues a data message unless either bound is reached.
    pub(crate) fn send(&self, d: Data, bytes: usize) {
        let limit = self.sh.opts.channel_bytes;
        if self.sh.inflight.fetch_add(bytes, Relaxed) + bytes > limit {
            self.sh.inflight.fetch_sub(bytes, Relaxed);
            self.sh.dropped(bytes);
            return;
        }
        if !self.send_raw(d) {
            self.sh.inflight.fetch_sub(bytes, Relaxed);
            self.sh.dropped(bytes);
        }
    }

    /// `try_send`, counting what is queued so a control message knows what came before it.
    pub(crate) fn send_raw(&self, d: Data) -> bool {
        let ok = self.data.try_send(d).is_ok();
        if ok {
            self.sh.sent.fetch_add(1, Relaxed);
        }
        ok
    }

    /// Sends a control message and wakes the writer if it is waiting for data. A full data
    /// channel means the writer is busy and drains control before its next batch. The
    /// writer handles the data queued before it first, and only that.
    pub(crate) fn control(&self, c: Control) -> bool {
        self.control_after(self.seen(), c)
    }

    /// The data messages queued so far: a control message sent with this count is handled
    /// after them and before anything queued later.
    pub(crate) fn seen(&self) -> u64 {
        self.sh.sent.load(Relaxed)
    }

    /// [`Handle::control`] with a count read earlier (a consent change reads it before it
    /// stores the gate, so the entries stamped under the new gate come after it).
    pub(crate) fn control_after(&self, seen: u64, c: Control) -> bool {
        let sent = self.ctl.send((seen, c)).is_ok();
        self.send_raw(Data::Wake);
        sent
    }

    pub(crate) fn pending(
        &self,
        source: Source,
        ctx: Option<Ctx>,
        p: AnyPayload,
        word: u64,
    ) -> Pending {
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

    pub(crate) fn flush(&self, timeout: Duration) -> Result<(), JournalError> {
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
