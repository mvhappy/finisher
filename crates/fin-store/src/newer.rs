//! A project that needs a newer Finisher (`NEWER_FORMAT`) is followed, never led: this server
//! does not take, renew or take over the lease while the project file
//! ([`crate::ProjectFile::is_newer`]) or the last import pass (a newer segment or record)
//! says so. A lease held when that turns true is let go at once. Otherwise a machine running
//! the older Finisher would hold the lease it cannot write under and lock out the machine that
//! can (it would wait out the staleness, or need a confirmed takeover on a sync folder).
//!
//! Letting go drops the [`crate::lease::Lease`]: its heartbeat thread stops and is joined,
//! and the record is left as it is (dropping releases nothing; others take over once it is
//! stale, lease module docs). While newer, nothing of the lease touches `.finisher/`: no
//! claim, no `lease.json` write and no clock probe (`lease.probe-*`), since the probe exists
//! only to judge staleness for an acquisition, and reading the folder needs none.

use fin_core::CoreError;
use serde_json::json;

use crate::export::StoreLease;
use crate::lease::LeaseState;
use crate::tick::Gate;
use crate::{Store, StoreError};

const WHY: &str = "the project needs a newer Finisher: this server follows it without the lease";

impl Store {
    /// Whether the project file or the last import pass says the project needs a newer
    /// Finisher.
    pub(crate) fn newer_format(&self) -> bool {
        self.file_newer || self.import.newer()
    }

    /// While [`Store::newer_format`]: lets go of a held lease (module docs) and reports why
    /// there is none. Returns whether the project is newer.
    pub(crate) fn follow_if_newer(&mut self) -> bool {
        if !self.newer_format() {
            return false;
        }
        if matches!(self.export.lease, StoreLease::Lease(_)) {
            // Dropping the lease stops and joins its heartbeat thread.
            self.export.lease = StoreLease::NotHeld;
            self.phase.io_gen += 1;
            self.ctl.gate = Gate::Open;
            self.ctl.check_error = None;
        }
        if matches!(self.export.lease, StoreLease::NotHeld) {
            self.ctl.seen = LeaseState::Lost {
                reason: WHY.to_string(),
            };
        }
        true
    }

    /// `LEASE_TAKEOVER_REFUSED` (`NEWER_FORMAT`) for a takeover of a newer project.
    pub(crate) fn newer_takeover_refused(&self) -> StoreError {
        CoreError::new(
            "LEASE_TAKEOVER_REFUSED",
            "lease takeover refused: the project needs a newer Finisher",
        )
        .with_details(json!({ "reason": "NEWER_FORMAT", "projectId": self.project_id }))
        .with_fix("update Finisher to edit this project")
        .into()
    }
}
