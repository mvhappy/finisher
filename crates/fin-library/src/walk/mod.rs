//! The walker (plan 5D Task 4; Decisions 9, 13, 17, 18, 25, 27): reads a root through a
//! [`Source`] on parallel threads and hands [`ScanBatch`]es to a sink, **in a deterministic
//! order** (breadth first, by depth then raw chain), whatever order the reads finish in.
//!
//! **The first stat** ([`Source::stat_root`]) waits up to `Timing::first_stat` (60 s, spin-up
//! included) and is judged against the stored identity ([`crate::volume::classify_root`]):
//! anything but online, or an empty root where the catalog knows entries
//! (`ROOT_EMPTY_SUSPECT`), ends the walk with [`WalkError::Offline`] before any batch.
//!
//! **Nested mounts** are walked and catalogued: a directory that is a mount root has its
//! volume measured after its open ([`Source::volume`]) and reported in its `DirDone`; one the
//! catalog knows as a mount root is checked against the stored volume first.
//!
//! **Each directory** is opened from its parent's handle when one is kept (by its raw chain
//! from the root otherwise) and listed by [`Source::read_dir_with`]; [`crate::listing`]
//! classifies its entries. A read with no progress for `Timing::entry` (30 s) leaves the
//! directory `TIMEOUT` and the walk goes on (its thread is left behind). A
//! listing's entries precede its [`DirDone`](crate::types::DirDone); directories that are not descended get a
//! `DirDone {complete: false}` with their code:
//!
//! | code | why |
//! |---|---|
//! | `TIMEOUT`, `IO`, `PERMISSION_DENIED`, `NOT_FOUND`, … | the open or the listing failed ([`crate::volume::unreachable_code`]) |
//! | `CLOUD_ONLY` | a placeholder directory (Decision 13), never opened |
//! | `AUTOMOUNT` | an automount point (Decision 5), never opened |
//! | `ROOT_UNMOUNTED` | a known nested mount root that is no longer one (Decision 17) |
//! | `ROOT_VOLUME_CHANGED` | a known nested mount root that holds another volume now (Decision 17) |
//! | `DIR_EMPTY_SUSPECT` | a listing that dropped from N > 0 entries to 0 (Decision 25) |
//! | `ENTRY_UNREADABLE` | a partial listing ([`crate::source::Listing`]): its readable entries are reported, the rest are not marked missing |
//!
//! Excluded directories and links are reported as entries only. A failure of the root's own
//! listing is [`WalkError::Offline`]. What each mode lists: the `poll` module.
//!
//! **Read-only:** the walker opens directories only, never a file, and writes nothing.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

mod batch;
mod poll;
mod pool;
mod run;

pub use poll::KnownDirs;

use poll::Plan;

use crate::exclude::Excludes;
use crate::raw::RawRel;
use crate::source::Source;
use crate::timing::Timing;
use crate::types::{RootState, ScanBatch, VolumeIdentity};
use crate::volume::{classify_root, unreachable_code};

/// `DirDone` codes of directories the walker did not list ([`crate::volume::unreachable_code`]
/// gives those of failed opens and listings).
pub mod code {
    pub const TIMEOUT: &str = "TIMEOUT";
    pub const CLOUD_ONLY: &str = "CLOUD_ONLY";
    pub const AUTOMOUNT: &str = crate::exclude::code::AUTOMOUNT;
    pub const ROOT_UNMOUNTED: &str = "ROOT_UNMOUNTED";
    pub const ROOT_VOLUME_CHANGED: &str = "ROOT_VOLUME_CHANGED";
    pub const DIR_EMPTY_SUSPECT: &str = "DIR_EMPTY_SUSPECT";
    pub const ENTRY_UNREADABLE: &str = "ENTRY_UNREADABLE";
}

/// Read threads for local roots (Decision 9).
pub const THREADS_LOCAL: usize = 64;
/// Read threads for network and sync roots.
pub const THREADS_REMOTE: usize = 32;
/// Entries (plus `DirDone`s) per batch (Decision 26).
pub const BATCH_ENTRIES: usize = 4096;
/// Bytes of NDJSON per batch, estimated from above.
pub const BATCH_BYTES: usize = 8 << 20;

/// What to walk.
#[derive(Debug, Clone)]
pub enum WalkMode {
    /// Every directory. `known` (the catalog's directories, `in/dirs.ndjson`) enables the
    /// mount and empty-suspect rules, and skips listings already done in this `generation`
    /// (a resumed job, Decision 27).
    Full {
        generation: u64,
        known: Option<KnownDirs>,
    },
    /// The given directories; with `known`, also the subdirectories they report that the
    /// catalog does not know.
    Dirs {
        dirs: Vec<RawRel>,
        known: Option<KnownDirs>,
    },
    /// Re-list the known directories whose mtime changed, and new ones (Decision 28).
    Poll { known: KnownDirs },
}

/// How to walk.
#[derive(Debug, Clone)]
pub struct WalkOpts {
    /// Read threads ([`THREADS_LOCAL`], [`THREADS_REMOTE`]).
    pub threads: usize,
    pub excludes: Excludes,
    pub timing: Timing,
    pub batch_entries: usize,
    pub batch_bytes: usize,
    /// The library's stored identity, which the first stat is judged against; `None` for the
    /// first scan of a new library.
    pub stored: Option<VolumeIdentity>,
}

impl WalkOpts {
    pub fn new(excludes: Excludes) -> Self {
        Self {
            threads: THREADS_LOCAL,
            excludes,
            timing: Timing::default(),
            batch_entries: BATCH_ENTRIES,
            batch_bytes: BATCH_BYTES,
            stored: None,
        }
    }
}

/// What a walk did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkSummary {
    /// The identity the first stat measured (refreshed into the library after a full scan).
    pub identity: VolumeIdentity,
    /// Complete listings.
    pub listed: u64,
    /// `DirDone {complete: false}`s, timeouts included.
    pub incomplete: u64,
    pub timeouts: u64,
    /// Poll: known directories not re-listed.
    pub unchanged: u64,
    /// Resumed full scan: listings already done in this generation.
    pub skipped: u64,
    pub entries: u64,
    pub batches: u64,
    /// Read threads still stuck in an abandoned read when the walk ended.
    pub stuck: u64,
}

/// The sink's refusal (the host call that takes a batch failed); the walk stops at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkStop(pub String);

impl fmt::Display for WalkStop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WalkStop {}

#[derive(Debug, thiserror::Error)]
pub enum WalkError {
    /// The root is not usable (Decision 18); the caller answers `LIBRARY_OFFLINE`. `Empty`
    /// stands for `ROOT_EMPTY_SUSPECT`.
    #[error("the library root is offline")]
    Offline(Box<RootState>),
    #[error("the walk was cancelled")]
    Cancelled,
    #[error("the walk was stopped: {0}")]
    Stopped(WalkStop),
    #[error("the walk could not start a thread: {0}")]
    Thread(io::Error),
}

impl WalkError {
    /// The root's state, when the walk found it offline.
    pub fn offline(&self) -> Option<&RootState> {
        match self {
            Self::Offline(s) => Some(s),
            _ => None,
        }
    }

    fn off(state: RootState) -> Self {
        Self::Offline(Box::new(state))
    }
}

/// A cancel flag shared with the caller (5C-1's job token sets it).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// A token over an existing flag.
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Walks `src` (see the module docs). Batches go to `sink` in order, `seq` from 0; a cancel
/// stops the walk within one batch ([`WalkError::Cancelled`]).
pub fn walk(
    src: Arc<dyn Source>,
    mode: WalkMode,
    opts: &WalkOpts,
    sink: &mut dyn FnMut(ScanBatch) -> Result<(), WalkStop>,
    cancel: &CancelToken,
) -> Result<WalkSummary, WalkError> {
    if cancel.is_cancelled() {
        return Err(WalkError::Cancelled);
    }
    let stat = match pool::first_stat(&src, opts.timing.first_stat, cancel) {
        None if cancel.is_cancelled() => return Err(WalkError::Cancelled),
        None => return Err(WalkError::off(RootState::Timeout)),
        Some(Err(e)) => {
            return Err(WalkError::off(RootState::Unreachable {
                code: unreachable_code(&e).into(),
            }));
        }
        Some(Ok(s)) => s,
    };
    let state = match &opts.stored {
        Some(stored) => classify_root(stored, &stat),
        None => RootState::Online(stat.identity()),
    };
    let RootState::Online(identity) = state else {
        return Err(WalkError::off(state));
    };
    let (plan, seeds) = Plan::new(mode, &opts.excludes);
    if stat.entries == 0 && plan.known_entries(&RawRel::root()) > 0 {
        return Err(WalkError::off(RootState::Empty));
    }
    run::Walker::new(plan, src, opts, sink, cancel, identity).run(seeds)
}
