//! `Store::tick`: following the folder, exporting, and driving the lease (plan Task 11).
//!
//! # One tick ([`Store::tick`])
//!
//! Folder work runs once per poll, at the storage kind's cadence (plan Decision 3: local 2 s,
//! network 3 s, sync 10 s); calls in between only export (the export has its own delay) and
//! report status changes. A poll:
//! 1. **import** ([`Store::import`]); applied records become [`StoreEvent::Applied`];
//! 2. **export** if holding and writable;
//! 3. **fencing re-check** while holding ([`Lease::check`]): a lost lease is dropped (the
//!    store is `LEASE_LOST`); a check that only could not list the folder is reported in
//!    status and changes nothing;
//! 4. **lease retry** when not holding (right after a loss too): one
//!    [`Contender::try_acquire`], always at a fresh epoch (a confirmed-stale holder is taken
//!    over where `auto_takeover`; a sync folder needs [`Store::takeover`]). After any
//!    acquisition the ordering rule applies (below), and when it needs no wait an import pass
//!    runs at once;
//! 5. temp-file cleanup runs inside the export; daily backups (Task 12) and compaction
//!    (Task 13) go at the marked call point.
//!
//! (The plan lists the retry before the re-check; re-checking first lets a loss found by
//! this poll be retried by the same poll.)
//!
//! Every call ends with [`StoreEvent::Status`] if the status changed (lease ages ignored).
//!
//! # The ordering rule
//!
//! After every acquisition (at open, an automatic re-acquisition or takeover, a manual
//! takeover) commits and exports are blocked (`CATCHING_UP`) until an import pass at the new
//! epoch has resolved every branch with nothing `waiting` and no import error. Otherwise a
//! stale local tail, or a commit on top of it, would be written at the new epoch and beat
//! records other servers exported meanwhile (it would launder: the export labels segments with
//! the commit epoch, but a new commit raises the tail's epoch).
//!
//! When the replaced `lease.json` names another server (or was damaged), or a claim above the
//! highest epoch in our database belongs to another server, that server may have exported
//! files our directory listing does not show yet. Then the pass that lifts the block must
//! start at least [`DIR_CACHE_WINDOW`] (10 s, monotonic: the Windows SMB directory cache)
//! after the acquisition. Passes before that still import.

use std::time::{Duration, Instant};

use fin_core::CoreError;

use crate::export::{self, ExportReport, StoreLease};
use crate::import::ImportReport;
use crate::lease::{Contender, Lease, LeaseState, Params, describe_holder};
use crate::status::StoreEvent;
use crate::storage_kind::StorageKind;
use crate::{Store, StoreError};

/// How long a directory listing may lag behind another server's writes.
pub const DIR_CACHE_WINDOW: Duration = Duration::from_secs(10);

/// The poll cadence for `kind` (plan Decision 3).
pub fn poll_every(kind: StorageKind) -> Duration {
    match kind {
        StorageKind::Local => Duration::from_secs(2),
        StorageKind::Network => Duration::from_secs(3),
        StorageKind::Sync(_) => Duration::from_secs(10),
    }
}

/// Whether commits and exports wait for an import (module docs, "The ordering rule").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    Open,
    /// Blocked until a clean import pass that started at or after `wait_until`.
    Pending {
        wait_until: Option<Instant>,
    },
}

/// The lease as the store drives it. The lease in force (the fence) is
/// `ExportState::lease`.
pub(crate) struct LeaseCtl {
    pub(crate) params: Params,
    holder: String,
    /// Kept between attempts while not holding: staleness needs observations over time.
    contender: Option<Contender>,
    /// The last attempt's answer while not holding.
    pub(crate) seen: LeaseState,
    pub(crate) gate: Gate,
    /// The last fencing check that failed only because the folder could not be listed.
    pub(crate) check_error: Option<String>,
    last_poll: Option<Instant>,
    /// A test lease is in force: no acquisition, no re-check.
    fixed: bool,
}

impl LeaseCtl {
    pub(crate) fn new(params: Params) -> LeaseCtl {
        LeaseCtl {
            params,
            holder: describe_holder(env!("CARGO_PKG_VERSION")),
            contender: None,
            seen: LeaseState::Lost {
                reason: "not acquired yet".to_string(),
            },
            gate: Gate::Open,
            check_error: None,
            last_poll: None,
            fixed: false,
        }
    }

    /// Now, by the lease's monotonic clock.
    pub(crate) fn now(&self) -> Instant {
        self.params.clock.now()
    }
}

/// What one tick did ([`Store::tick_report`]).
#[derive(Debug, Clone, Default)]
pub struct TickReport {
    pub events: Vec<StoreEvent>,
    /// The import passes of this tick (none between polls; two after an acquisition).
    pub imports: Vec<ImportReport>,
    pub export: ExportReport,
}

impl Store {
    /// Periodic folder work (module docs); call it often (the maintenance thread: every
    /// 500 ms). `now` drives the poll cadence and the export delay. A folder failure is
    /// reported in [`Store::status`], never as `Err`, which is a database failure.
    pub fn tick(&mut self, now: Instant) -> Result<Vec<StoreEvent>, StoreError> {
        Ok(self.tick_report(now)?.events)
    }

    /// [`Store::tick`], with what each step did.
    pub fn tick_report(&mut self, now: Instant) -> Result<TickReport, StoreError> {
        let mut out = TickReport::default();
        let every = poll_every(self.storage_kind());
        let due = self
            .ctl
            .last_poll
            .is_none_or(|t| now.saturating_duration_since(t) >= every);
        if due {
            self.ctl.last_poll = Some(now);
            let r = self.import_pass()?;
            out.events
                .extend(r.applied.iter().cloned().map(StoreEvent::Applied));
            out.imports.push(r);
        }
        out.export = export::export_pending(self, now, false)?;
        if out.export.segments_written > 0 {
            self.follow.last_export_ms = Some(crate::commit::now_ms());
        }
        if due && !self.ctl.fixed {
            if self.holding() {
                self.recheck();
            }
            if !self.holding() {
                self.release_lost();
                if self.try_acquire() && self.ctl.gate_needs_no_wait() {
                    let r = self.import_pass()?;
                    out.events
                        .extend(r.applied.iter().cloned().map(StoreEvent::Applied));
                    out.imports.push(r);
                }
            }
            // Task 12: daily backup. Task 13: compaction when triggered (holder only).
        }
        if let Some(status) = self.status_if_changed() {
            out.events.push(StoreEvent::Status(Box::new(status)));
        }
        Ok(out)
    }

    /// Flushes the export without waiting for the delay (when writable), then closes the
    /// store. The lease is not released: it stops heartbeating, and other servers wait out
    /// its staleness (45 s plus confirmation) before taking over; this process may re-take
    /// its own stopped lease at once. Dropping a `Store` does not flush: folder I/O can block
    /// for seconds on a network share, and a drop must not.
    pub fn close(mut self) -> Result<ExportReport, StoreError> {
        let now = self.ctl.now();
        export::export_pending(&mut self, now, true)
    }

    /// Takes the lease over (`fin project takeover [--confirm]`, spec §3.5): see
    /// [`Contender::takeover`] for what `confirm` allows and the `LEASE_TAKEOVER_REFUSED`
    /// reasons. On success an import pass runs before this returns, and the ordering rule
    /// applies as after any acquisition. Holding already: the status, unchanged.
    pub fn takeover(&mut self, confirm: bool) -> Result<crate::ProjectStatus, StoreError> {
        if !self.holding() {
            if !self.project_dir.join(".finisher").is_dir() {
                return Err(CoreError::new(
                    "LEASE_TAKEOVER_REFUSED",
                    "the project folder cannot be reached",
                )
                .with_details(serde_json::json!({ "reason": "UNAVAILABLE" }))
                .into());
            }
            self.ctl.fixed = false;
            let lease = self.contender().takeover(confirm)?;
            self.acquired(lease);
            self.import_pass()?;
        }
        Ok(self.status())
    }

    /// Whether this store holds the lease now (in memory, no folder I/O).
    pub(crate) fn holding(&self) -> bool {
        self.export.lease.quick_check().is_ok()
    }

    fn contender(&mut self) -> &mut Contender {
        let dir = self.project_dir.join(".finisher");
        let LeaseCtl {
            contender,
            params,
            holder,
            ..
        } = &mut self.ctl;
        contender
            .get_or_insert_with(|| Contender::new(&dir, &self.server_id, holder, params.clone()))
    }

    /// One acquisition attempt. Never creates `.finisher/`: a project folder that vanished
    /// must not come back empty.
    pub(crate) fn try_acquire(&mut self) -> bool {
        if !self.project_dir.join(".finisher").is_dir() {
            self.ctl.seen = LeaseState::Lost {
                reason: format!("{} cannot be reached", self.project_dir.display()),
            };
            return false;
        }
        match self.contender().try_acquire() {
            Ok(lease) => {
                self.acquired(lease);
                true
            }
            Err(state) => {
                self.ctl.seen = state;
                false
            }
        }
    }

    /// A new lease is in force: block until caught up (module docs).
    fn acquired(&mut self, lease: Lease) {
        // A database that cannot say counts as knowing nothing: wait.
        let known = crate::db::max_epoch(&self.conn).unwrap_or(0);
        let wait = lease.previous().other_writer_since(&self.server_id, known);
        let now = self.ctl.now();
        self.ctl.gate = Gate::Pending {
            wait_until: wait.then(|| now + DIR_CACHE_WINDOW),
        };
        self.ctl.contender = None;
        self.ctl.check_error = None;
        self.ctl.seen = lease.state();
        self.export.lease = StoreLease::Lease(lease);
    }

    /// The fencing re-check of a poll (module docs, step 3).
    fn recheck(&mut self) {
        let StoreLease::Lease(lease) = &self.export.lease else {
            return;
        };
        match lease.check() {
            Ok(_) => self.ctl.check_error = None,
            Err(l) if l.transient => self.ctl.check_error = Some(l.reason),
            Err(_) => self.release_lost(),
        }
    }

    /// A lease that is no longer held (lost, self-fenced) is dropped: `LEASE_LOST` until the
    /// next acquisition.
    fn release_lost(&mut self) {
        if let StoreLease::Lease(lease) = &self.export.lease {
            self.ctl.seen = lease.state();
            self.ctl.check_error = None;
            self.ctl.gate = Gate::Open;
            self.export.lease = StoreLease::NotHeld;
        }
    }

    /// An import pass that also lifts the ordering rule's block when it may (module docs).
    pub(crate) fn import_pass(&mut self) -> Result<ImportReport, StoreError> {
        let started = self.ctl.now();
        let r = self.import()?;
        self.follow.observe(&r, self.storage_kind());
        if let Gate::Pending { wait_until } = self.ctl.gate
            && wait_until.is_none_or(|t| started >= t)
            && r.waiting.is_empty()
            && r.error.is_none()
            && self.holding()
        {
            self.ctl.gate = Gate::Open;
        }
        Ok(r)
    }

    /// The lease state as status reports it.
    pub fn lease_state(&self) -> LeaseState {
        match &self.export.lease {
            StoreLease::Lease(l) => l.state(),
            StoreLease::NotHeld => self.ctl.seen.clone(),
            #[cfg(any(test, feature = "test-hooks"))]
            StoreLease::Fixed { epoch, held: true } => LeaseState::Held { epoch: *epoch },
            #[cfg(any(test, feature = "test-hooks"))]
            StoreLease::Fixed { held: false, .. } => LeaseState::Lost {
                reason: "the test lease is lost".to_string(),
            },
        }
    }

    /// The last export failure, cleared by the next successful export.
    pub fn export_error(&self) -> Option<&CoreError> {
        self.export.error.as_ref()
    }

    /// The real lease, while one is in force.
    pub fn lease(&self) -> Option<&Lease> {
        match &self.export.lease {
            StoreLease::Lease(l) => Some(l),
            _ => None,
        }
    }

    /// Test hook: the next export stops at `point` with `TEST_FAULT`, as if the process died
    /// there.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn fail_next_export_at(&mut self, point: crate::FailPoint) {
        self.export.fail_at = Some(point);
    }

    /// Test hook: this store holds (or has lost) a lease at `epoch`, replacing the real one.
    /// Commits and export write only while `held`, at `epoch`; `tick` no longer acquires or
    /// re-checks, and the ordering rule does not apply.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn set_test_lease(&mut self, epoch: u64, held: bool) {
        self.export.lease = StoreLease::Fixed { epoch, held };
        self.ctl.seen = self.lease_state();
        self.ctl.fixed = true;
        self.ctl.gate = Gate::Open;
        self.ctl.contender = None;
    }

    /// Test hook: `lease` becomes the fence of every commit and export, replacing the one in
    /// force. Unlike an acquisition, commits are allowed at once (no ordering rule).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn set_lease_for_test(&mut self, lease: Lease) {
        self.ctl.fixed = false;
        self.acquired(lease);
        self.ctl.gate = Gate::Open;
    }

    /// Test hook: forgets when the last poll ran, so the next `tick` polls.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn poll_now_for_test(&mut self) {
        self.ctl.last_poll = None;
    }
}

impl LeaseCtl {
    /// The block can lift on the very next pass (no directory-cache wait).
    fn gate_needs_no_wait(&self) -> bool {
        matches!(self.gate, Gate::Pending { wait_until: None })
    }
}
