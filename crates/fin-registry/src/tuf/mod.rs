//! TUF registries (sub-project 8 spec §6; 8A Tasks 4a–4b): [`TrustedRepo`] runs the TUF 1.0
//! client workflow over `tough` 0.24 (8A Decision 8) and keeps what it trusts in a
//! [`TrustStore`] per root. `tough` is wrapped where the 8A Task 1 gate found it short: no
//! write of its datastore is trusted, roots are kept as fetched, a root-walk transport error
//! fails the refresh, caps apply per file, and the clock is injected ([`Clock`]): expiry
//! (`strict`, or `relaxed` with `maxStaleDays`) and the clock check (`CLOCK_SUSPECT`) are
//! ours, never `tough`'s.

mod adapter;
mod checks;
pub mod clock;
pub mod custom;
mod error;
mod freshness;
mod store;
pub(crate) mod trusted;

pub(crate) use checks::publisher_name_ok as checks_publisher_name;
pub use clock::{CLOCK_SLACK_SECS, Clock, FixedClock, SystemClock};
pub use custom::{DEFAULT_MAX_STALE_DAYS, MAX_STALE_DAYS_CAP, permissions_digest};
pub use error::TufError;
pub use store::{TrustState, TrustStore};
pub use trusted::{TargetInfo, TrustedRepo, sha256_hex};

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};

/// How expiry is enforced (spec §6.3, 8A Decision 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Freshness {
    /// An expired role is `TUF_EXPIRED` (future HTTPS registries, tests).
    Strict,
    /// Local folders: expired by more than `maxStaleDays` is `TUF_EXPIRED`; within it,
    /// `REGISTRY_STALE` is a warning.
    #[default]
    Relaxed,
}

/// Which repository layout a refresh requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// Finisher registries: one terminating `<publisher>/*` delegation per publisher.
    #[default]
    Finisher,
    /// Any TUF 1.0 repository with consistent snapshots (the `tuf-conformance` client):
    /// delegations are only checked by `tough` and the "listed but not loaded" rule.
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Policy {
    pub freshness: Freshness,
    pub layout: Layout,
}

/// A warning `{code, details}`, e.g. `REGISTRY_STALE {days, maxStaleDays, role}`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Warning {
    pub code: &'static str,
    pub details: Map<String, Value>,
}

/// What a refresh found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RefreshReport {
    pub root_version: u64,
    pub timestamp_version: u64,
    pub snapshot_version: u64,
    pub targets_version: u64,
    /// Roots newly trusted by this refresh.
    pub roots_added: u64,
    /// Whether the trusted set changed.
    pub changed: bool,
    /// Whole days the metadata is past its earliest expiry, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_days: Option<u16>,
    pub warnings: Vec<Warning>,
}
