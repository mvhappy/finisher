//! What an import pass reports: [`ImportReport`] and its parts, [`ReadOnlyReason`], [`Seed`].

use std::path::PathBuf;

use fin_core::{CommittedTx, CoreError};
use serde::Serialize;

use crate::chain::ForkInfo;
use crate::conflict_names::Provider;
use crate::scan::NotArrivedWhy;

/// What one import pass found and did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    /// Records applied, in order (each `CommittedTx`'s branch is in its `tx`).
    pub applied: Vec<CommittedTx>,
    /// Records currently set aside (quarantine rows that this pass's verdicts still report,
    /// plus this server's diverged tails, which nothing else holds).
    pub quarantined: u32,
    /// Missing predecessor segment hashes (gaps); retried on the next pass.
    pub waiting: Vec<String>,
    /// Hashes of segments neither imported nor set aside yet (beyond a fork, behind a gap).
    pub pending: Vec<String>,
    /// Files under `.finisher/` that have not arrived yet (placeholders, incomplete, temp).
    pub not_arrived: Vec<(PathBuf, NotArrivedWhy)>,
    /// The first branch that cannot continue without a person choosing (`CHAIN_FORK`).
    pub fork: Option<ForkInfo>,
    /// Files under `.finisher/` that are not ours, conflict copies among them.
    pub foreign: Vec<ForeignFile>,
    /// A file (or a record) needs a newer Finisher (`NEWER_FORMAT`).
    pub newer: bool,
    /// Placeholders read this pass to hydrate them; the next pass imports them.
    pub hydrated: Vec<PathBuf>,
    /// A record on the resolved chain that cannot be applied (`REPLAY_FAILED`,
    /// `INVALID_FORMAT`, `SCHEMA_VERSION`): its branch stops before it; retried every pass.
    pub error: Option<CoreError>,
    /// Records taken out of a branch's history this pass (a rewound applied line, a diverged
    /// tail), before `applied` was applied.
    pub rewound: Vec<Rewind>,
}

/// Records taken out of `branch`'s history by an import ([`crate::StoreEvent::Rewound`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewind {
    pub branch: String,
    /// The first rev that no longer holds what it held.
    pub from_rev: u64,
    /// Records set aside.
    pub quarantined: u32,
    /// Some of them were this server's own.
    pub own: bool,
    /// The branch's generation after the rewind.
    pub generation: u64,
}

/// A folder snapshot a rebuild offers to seed a fresh branch from (spec §3.4: it seeds only
/// if its `txHash` lies on the resolved chain).
#[derive(Debug, Clone)]
pub(crate) struct Seed {
    pub(crate) branch: String,
    pub(crate) seq: u64,
    pub(crate) tx_hash: String,
    pub(crate) project: fin_core::Project,
}

/// A foreign file (spec §3.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignFile {
    pub path: PathBuf,
    /// The conflict-copy pattern it matches; `Some(Unknown)` for a Finisher file under a name
    /// that is not its own.
    pub conflict_copy: Option<Provider>,
    /// Records it holds that the DB already has: harmless duplicates.
    pub known: u32,
    /// Records it holds that the DB does not have: set aside as `CONFLICT_COPY`.
    pub quarantined: u32,
}

/// Why the project is read-only (spec §3.7). Import sets `ChainFork`, `ConflictCopy`,
/// `NewerFormat` and `ImportError`; the lease sets `LeaseHeld`, `LeaseLost` and
/// `CatchingUp` (`Store::read_only` combines them).
///
/// Not in the spec's table (Task 20 adds them):
/// - `ImportError`: a record on the resolved chain cannot be applied
///   ([`ImportReport::error`]). Writing past it would build a line that servers able to
///   replay it see as a fork, so commits and export wait until a later pass no longer
///   reports it.
/// - `CatchingUp`: the lease was just acquired, and no import pass at the new epoch has yet
///   resolved every branch with nothing waiting (after waiting out the directory-cache
///   window when another server wrote before). Until then a commit or an export could extend
///   a line the folder has already moved past (plan Task 11, ordering rule).
///
/// Precedence, highest first: `ChainFork`, `ConflictCopy`, `NewerFormat`, `ImportError`
/// (the folder's content: a person or an update must act, whoever holds the lease), then
/// `LeaseLost`, `LeaseHeld`, `CatchingUp` (the lease: resolved by waiting or a takeover).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReadOnlyReason {
    LeaseHeld,
    LeaseLost,
    ChainFork,
    ConflictCopy,
    NewerFormat,
    ImportError,
    CatchingUp,
}

impl ReadOnlyReason {
    pub fn code(self) -> &'static str {
        match self {
            ReadOnlyReason::LeaseHeld => "LEASE_HELD",
            ReadOnlyReason::LeaseLost => "LEASE_LOST",
            ReadOnlyReason::ChainFork => "CHAIN_FORK",
            ReadOnlyReason::ConflictCopy => "CONFLICT_COPY",
            ReadOnlyReason::NewerFormat => "NEWER_FORMAT",
            ReadOnlyReason::ImportError => "IMPORT_ERROR",
            ReadOnlyReason::CatchingUp => "CATCHING_UP",
        }
    }

    pub fn fix(self) -> &'static str {
        match self {
            ReadOnlyReason::LeaseHeld => "wait for the other server, or take the project over",
            ReadOnlyReason::LeaseLost => "the lease is re-acquired automatically when possible",
            ReadOnlyReason::ChainFork => {
                "two histories exist in the project folder; keep one and move the other's segment files out of .finisher/segments (fin project status lists them)"
            }
            ReadOnlyReason::ConflictCopy => {
                "a sync client made a conflict copy of a Finisher file (fin project status lists it): another machine may have written at the same time; keep one history and move the copy out of .finisher"
            }
            ReadOnlyReason::CatchingUp => {
                "the lease was just taken: the project is writable once the folder has been read (about 10 s, or once missing segments arrive)"
            }
            ReadOnlyReason::NewerFormat => "update Finisher to edit this project",
            ReadOnlyReason::ImportError => {
                "a record in the project folder cannot be applied (details in fin project status): update Finisher, or move the damaged segment out of .finisher/segments"
            }
        }
    }
}
