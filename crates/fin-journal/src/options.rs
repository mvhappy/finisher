//! The journal's limits and its configured inputs ([`JournalOptions`]).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use fin_core::events::ScrubCtx;

use crate::{Clock, SystemClock};

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
    /// The install's keyed hash for ids and argument hashes in a received wire payload
    /// (`record_value`). `None` (the default) refuses every wire kind (`EVENT_INVALID`): no
    /// key, no keyed value reaches the disk.
    // D3: replace with the install key. D3 owns the per-install key; until it exists the
    // default is `None` and only a caller (a test) that supplies one writes wire kinds.
    pub key: Option<KeyFn>,
    /// What a received payload's free text is scrubbed with again (the server's home, data,
    /// temp, user and host; `fin-core` reads none). The default scrubs absolute paths only.
    pub scrub: ScrubCtx,
}

/// A keyed hash of a byte string, 16 bytes ([`JournalOptions::key`]).
pub type KeyFn = Arc<dyn Fn(&[u8]) -> [u8; 16] + Send + Sync>;

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
            key: None,
            scrub: ScrubCtx::default(),
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
            .field("key", &self.key.as_ref().map(|_| "<set>"))
            .finish_non_exhaustive()
    }
}
