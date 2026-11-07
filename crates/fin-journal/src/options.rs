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
    /// An override of the 16-byte keyed hash (tests only). `None`, the default, is the
    /// per-install key in `events/key` (Decision 6), which [`crate::Journal::hash`],
    /// [`crate::Journal::id_ref`] and `record_value`'s wire kinds use; blobs always use the
    /// install key.
    pub key: Option<KeyFn>,
    /// What a received payload's free text is scrubbed with again (the server's home, data,
    /// temp, user and host; `fin-core` reads none): [`server_scrub`] in the server. The
    /// default scrubs absolute paths only.
    pub scrub: ScrubCtx,
}

/// The server's [`ScrubCtx`]: its home, `server_data`, temp dir, user and host names, from
/// the environment (`HOME`/`USERPROFILE`, `USER`/`USERNAME`, `HOSTNAME`/`COMPUTERNAME`, then
/// `/etc/hostname` on unix).
///
/// **Pass `host`** (from `finisherd`, Node's `os.hostname()`): macOS shells rarely export
/// `HOSTNAME` and have no `/etc/hostname`, so without it the host name is not scrubbed there.
/// `host` wins over the environment.
pub fn server_scrub(server_data: &std::path::Path, host: Option<String>) -> ScrubCtx {
    let var = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| std::env::var(n).ok().filter(|v| !v.trim().is_empty()))
    };
    let host = host.filter(|h| !h.trim().is_empty()).or_else(|| {
        var(&["HOSTNAME", "COMPUTERNAME"]).or_else(|| {
            ["/etc/hostname", "/proc/sys/kernel/hostname"]
                .iter()
                .find_map(|p| std::fs::read_to_string(p).ok())
                .map(|h| h.trim().to_owned())
                .filter(|h| !h.is_empty())
        })
    });
    ScrubCtx {
        home: var(&["HOME", "USERPROFILE"]),
        server_data: Some(server_data.to_string_lossy().into_owned()),
        tmp: Some(std::env::temp_dir().to_string_lossy().into_owned()),
        user: var(&["USER", "USERNAME", "LOGNAME"]),
        host,
    }
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
