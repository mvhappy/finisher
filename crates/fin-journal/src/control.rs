//! What the callers hand the writer (plan D, Decision 18): data messages (entries and timing
//! samples) on the bounded channel, which drop when full; control messages (synchronous
//! records, flushes, close) on the unbounded one, which never drop. The packed consent word
//! the gate is read from lives here too.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::SyncSender;

use fin_core::events::{AnyPayload, Basis, Code, CollectPolicy, Ctx, Name, Purposes, Source, Ulid};

use crate::blobs::BlobData;
use crate::consent::ConsentState;
use crate::purge::{PurgeReport, PurgeSelector};
use crate::{JournalError, Metric};

/// The consent an entry is stamped with when it is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub purposes: Purposes,
    pub rev: u64,
    pub basis: Basis,
}

/// The gate word: purposes in bits 0–1, basis in bits 2–3, `rev` above. One `Relaxed` load
/// gives the gate ([`gate_of`]) and a consistent stamp ([`unpack`]).
pub(crate) fn pack(s: Stamp) -> u64 {
    debug_assert!(
        s.rev < 1 << 60,
        "consent rev {} does not fit the gate word",
        s.rev
    );
    let basis = match s.basis {
        Basis::Default => 0,
        Basis::Env => 1,
        Basis::User => 2,
    };
    u64::from(s.purposes.bits()) | (basis << 2) | (s.rev << 4)
}

#[inline]
pub(crate) fn gate_of(word: u64) -> Purposes {
    Purposes::from_bits((word & 3) as u8)
}

pub(crate) fn unpack(word: u64) -> Stamp {
    let basis = match (word >> 2) & 3 {
        1 => Basis::Env,
        2 => Basis::User,
        _ => Basis::Default,
    };
    Stamp {
        purposes: gate_of(word),
        rev: word >> 4,
        basis,
    }
}

/// An entry on its way to the writer: stamped in the caller, serialised on the writer.
#[derive(Debug)]
pub(crate) struct Pending {
    pub ts: u64,
    /// The envelope's `session` when it is not the journal's own run (`record_value`).
    pub session: Option<Ulid>,
    pub source: Source,
    pub ctx: Option<Ctx>,
    pub payload: AnyPayload,
    pub stamp: Stamp,
    /// What the entry counts against the byte bound.
    pub bytes: usize,
}

/// One timing for the stats windows (Decision 19).
#[derive(Debug)]
pub(crate) struct Sample {
    /// When it was observed: it counts in that window.
    pub ts: u64,
    pub metric: Metric,
    pub key: Name,
    pub micros: u64,
    pub code: Option<Code>,
}

/// What a sample counts against the byte bound.
pub(crate) const SAMPLE_BYTES: usize = 48;

pub(crate) enum Data {
    Entry(Box<Pending>),
    Sample(Box<Sample>),
    /// A blob's bytes, held by the writer until an entry references it.
    Blob(Box<BlobData>),
    /// Wakes the writer to drain the control channel; carries nothing.
    Wake,
}

/// A synchronous record's claim: the writer takes it, or the caller cancels it on timeout,
/// never both.
#[derive(Debug, Default)]
pub(crate) struct SyncSlot(AtomicU8);

const PENDING: u8 = 0;
const TAKEN: u8 = 1;
const CANCELLED: u8 = 2;

impl SyncSlot {
    /// The writer's claim; false when the caller already gave up.
    pub fn take(&self) -> bool {
        self.0
            .compare_exchange(PENDING, TAKEN, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// The caller's withdrawal; false when the writer already took the entry.
    pub fn cancel(&self) -> bool {
        self.0
            .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

pub(crate) enum Control {
    /// `record_sync`: written and flushed, then acknowledged (`true` when on disk).
    Sync {
        entry: Box<Pending>,
        slot: Arc<SyncSlot>,
        ack: SyncSender<bool>,
    },
    Flush(SyncSender<Result<(), JournalError>>),
    /// Flush, then answer the files that are closed (every one but the active file).
    Closed(SyncSender<Vec<String>>),
    /// The effective consent changed (the gate word already holds `to`): journal
    /// `consent.changed` under `from`, persist `to`, re-check the buffered lines.
    Consent {
        from: ConsentState,
        to: ConsentState,
    },
    /// The caps changed (settings): apply them now.
    Caps,
    /// A project's policy (`None`: unknown).
    Project {
        id: String,
        policy: Option<CollectPolicy>,
    },
    Purge {
        sel: PurgeSelector,
        ack: SyncSender<Result<PurgeReport, JournalError>>,
    },
    /// The server's run: `session.start` and `session.end` (`session.rs`).
    Session(crate::session::SessionMsg),
    /// Drain, flush, fsync, stop.
    Close(SyncSender<()>),
}
