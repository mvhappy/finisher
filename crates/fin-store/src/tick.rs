//! `Store::tick`: following the folder, exporting, and driving the lease (plan Task 11).
//!
//! # One tick ([`Store::tick`])
//!
//! Folder work runs once per poll, at the storage kind's cadence ([`poll_every`]: local 2 s,
//! network 1 s, sync 10 s); calls in between only export (the export has its own delay) and
//! report status changes. A poll:
//! 1. **import** ([`Store::import`]); records taken out of a branch's history become
//!    [`StoreEvent::Rewound`], then applied records [`StoreEvent::Applied`]. Every import
//!    pass queues its events, also those run outside `tick` (open, a takeover, a rebuild,
//!    which also queues [`StoreEvent::Rebuilt`] first); each tick returns the queue;
//! 2. **export** if holding and writable;
//! 3. **fencing re-check** while holding ([`Lease::check`]): a lost lease is dropped (the
//!    store is `LEASE_LOST`); a check that only could not list the folder is reported in
//!    status and changes nothing;
//! 4. **lease retry** when not holding (right after a loss too): one
//!    [`Contender::try_acquire`], always at a fresh epoch (a confirmed-stale holder is taken
//!    over where `auto_takeover`; a sync folder needs [`Store::takeover`]). After any
//!    acquisition the ordering rule applies (below), and when it needs no wait an import pass
//!    runs at once;
//! 5. temp-file cleanup runs inside the export; then the daily backup of the database
//!    ([`Store::daily_backup`], on every server: it is a copy of local state; checked once per
//!    poll, taken once per local calendar day, a failure retried after
//!    [`BACKUP_RETRY_MS`]);
//! 6. compaction (`crate::compact`; holder only, writable and caught up), also under a test
//!    lease.
//!
//! Step 5's backup and step 6 can take seconds: with [`TickOptions`] off, the tick only notes
//! them as due ([`Store::due_work`]) for the caller to run outside its tick.
//!
//! (The plan lists the retry before the re-check; re-checking first lets a loss found by
//! this poll be retried by the same poll.)
//!
//! Every call ends with [`StoreEvent::Status`] if the status changed (lease ages ignored).
//!
//! # Locking
//!
//! [`Store::tick_phased`] runs a tick through a [`StoreAccess`] (fin-node: the engine's
//! mutex), taking the store only around database work and in-memory steps, never across
//! folder I/O ([`Store::tick_with`] is the same tick on a store the caller has):
//! - an import pass is planned with the store (`Store::import_plan` takes out the scanner and
//!   the verified-segment cache), reads the folder without it (project-file reread, scan,
//!   hydration, verification, the full lease check, the lease files) and is applied with it
//!   (`Store::import_apply`: the one transaction) unless something changed what it read
//!   against since it was planned (`Phase::io_gen`: another pass, a marked export,
//!   compaction, a rebuild, a recovery, the lease in force); then the next poll reads afresh;
//! - an export writes segment by segment: rows read with the store, the fence, encoding,
//!   write and read-back without it (fenced by the planned lease: one the store let go since
//!   fails), marked with it only if the rows are still exactly the next unexported ones; it
//!   exports what was pending when its delay passed;
//! - the re-check's folder read runs without the store, applied only to the lease it checked;
//!   acquisition, the backup and compaction run with it (compaction rereads the folder against
//!   the database step by step before it deletes; it is not phased);
//! - one tick at a time ([`Store::tick_phased`] returns `None` while another runs); a store
//!   that goes away between two steps ends the tick there.
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
//! highest epoch in our database belongs to another server, or `lease.json` was missing while
//! the folder holds a segment by another writer or above our epochs, that server may have
//! exported files our directory listing does not show yet. Then the pass that lifts the block must
//! start at least [`dir_cache_window`] after the acquisition (monotonic): 10 s
//! ([`DIR_CACHE_WINDOW`], the Windows SMB directory cache), or 65 s on a network share mounted
//! by macOS ([`MACOS_DIR_CACHE_WINDOW`]). Passes before that still import.
//!
//! On a sync folder the block also stays while any file under `segments/` has not arrived
//! (a placeholder, or incomplete). The window is a directory-cache bound, not a propagation
//! bound: after a confirmed takeover on a sync folder the other machine's last edits may
//! still be on their way; status lists what is arriving (`SYNC_ARRIVING`), and nothing
//! bounds what has not started arriving.

use std::time::{Duration, Instant};

use fin_core::CoreError;

use crate::export::{self, ExportReport, StoreLease};
use crate::import::ImportReport;
use crate::lease::{Contender, Lease, LeaseLost, LeaseState, Params, ServerLock, describe_holder};
use crate::status::StoreEvent;
use crate::storage_kind::StorageKind;
use crate::{Store, StoreError};

/// How long a directory listing may lag behind another server's writes: the Windows SMB
/// client's `DirectoryCacheLifetime` (10 s). Linux clients and local disks lag less.
pub const DIR_CACHE_WINDOW: Duration = Duration::from_secs(10);

/// [`DIR_CACHE_WINDOW`] on a network share mounted by macOS. smbfs caches a directory's
/// listing for `dir_cache_min` 30 s up to `dir_cache_max` 60 s (`man nsmb.conf` defaults), for
/// every process on the machine: a listing measured on the NAS missed another host's new
/// segments for 30–31 s, and a verifier missed 64 of them for over 27 s. 65 s is
/// `dir_cache_max` plus a margin for the poll that must start after it. A user's
/// `/etc/nsmb.conf` can raise `dir_cache_max` (or turn the cache off); a raised value is
/// not detected and puts the bound back at risk (spec §3.6, a documented limit). The
/// listing refresh (`crate::scan`) makes the follow fast, never this rule safe: it is best
/// effort and cannot run read-only.
pub const MACOS_DIR_CACHE_WINDOW: Duration = Duration::from_secs(65);

/// The ordering rule's wait on `kind` (module docs) on this OS.
pub fn dir_cache_window(kind: StorageKind) -> Duration {
    dir_cache_window_on(kind, cfg!(target_os = "macos"))
}

/// [`dir_cache_window`] as on macOS (`is_macos`) or elsewhere, apart from the `cfg` so both
/// answers are tested on every OS ([`crate::storage_kind::slow_dir_cache_on`]).
pub(crate) fn dir_cache_window_on(kind: StorageKind, is_macos: bool) -> Duration {
    if crate::storage_kind::slow_dir_cache_on(kind, is_macos) {
        MACOS_DIR_CACHE_WINDOW
    } else {
        DIR_CACHE_WINDOW
    }
}

/// The poll cadence for `kind` (spec §3.6, Decision 3). A share polls every second for
/// follow latency (an import pass is one listing per branch directory; verified segments are
/// cached, so an idle poll reads no file); sync folders poll slowly because their client
/// delivers files seconds apart anyway.
pub fn poll_every(kind: StorageKind) -> Duration {
    match kind {
        StorageKind::Local => Duration::from_secs(2),
        StorageKind::Network => Duration::from_secs(1),
        StorageKind::Sync(_) => Duration::from_secs(10),
    }
}

/// How long after a failed daily backup the next attempt waits.
pub(crate) const BACKUP_RETRY_MS: i64 = 10 * 60 * 1000;

/// What the daily backup last did (by unix ms, local date).
#[derive(Debug, Clone, Default)]
pub(crate) struct DailyState {
    /// The local date that has its backup.
    done: Option<(i64, i64, i64)>,
    /// When the last attempt failed.
    failed_at: Option<i64>,
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
    /// The server-data lock every acquisition takes (`lease::ServerLock`).
    pub(crate) server_lock: Option<ServerLock>,
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
            server_lock: None,
        }
    }

    /// Now, by the lease's monotonic clock.
    pub(crate) fn now(&self) -> Instant {
        self.params.clock.now()
    }
}

/// Which heavy steps a tick runs itself ([`Store::tick_with`]). Both default to `true`.
///
/// The daily backup (`VACUUM INTO`) and compaction can take seconds (minutes on a slow share).
/// A caller that must not hold the store that long inside its tick turns them off: the tick
/// then only notes that they are due ([`Store::due_work`]) and the caller runs them when it
/// can ([`Store::run_due_backup`], [`Store::run_due_compaction`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickOptions {
    pub backup: bool,
    pub compaction: bool,
}

impl Default for TickOptions {
    fn default() -> Self {
        TickOptions {
            backup: true,
            compaction: true,
        }
    }
}

/// Heavy work found due by a tick that did not run it ([`TickOptions`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DueWork {
    pub backup: bool,
    pub compaction: bool,
}

/// What one tick did ([`Store::tick_report`]).
#[derive(Debug, Clone, Default)]
pub struct TickReport {
    pub events: Vec<StoreEvent>,
    /// The import passes of this tick (none between polls; two after an acquisition).
    pub imports: Vec<ImportReport>,
    pub export: ExportReport,
    /// Compaction (on polls only; [`crate::CompactReport::skipped`] says why it did not run).
    pub compact: crate::CompactReport,
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
        self.tick_with(now, TickOptions::default())
    }

    /// [`Store::tick_report`], running the heavy steps only as `opts` says (else noting them
    /// in [`Store::due_work`]).
    pub fn tick_with(&mut self, now: Instant, opts: TickOptions) -> Result<TickReport, StoreError> {
        // `None`: a phased tick of this store is running (another thread's, between its steps).
        Ok(Store::tick_phased(&mut Direct(self), now, opts)?.unwrap_or_default())
    }

    /// Flushes the export without waiting for the delay (when writable), then closes the
    /// store. A holder with nothing left unexported releases the lease (`export::close`), so
    /// another server takes it at once; otherwise others wait out its staleness. Dropping a
    /// `Store` flushes and releases nothing: folder I/O can block for seconds on a network
    /// share, and a drop must not.
    pub fn close(mut self) -> Result<ExportReport, StoreError> {
        let now = self.ctl.now();
        export::close(&mut self, now)
    }

    /// Takes the lease over (`fin project takeover [--confirm]`, spec §3.5): see
    /// [`Contender::takeover`] for what `confirm` allows and the `LEASE_TAKEOVER_REFUSED`
    /// reasons. On success an import pass runs before this returns, and the ordering rule
    /// applies as after any acquisition. Holding already: the status, unchanged. A project
    /// that needs a newer Finisher is refused (`NEWER_FORMAT`, `crate::newer`).
    pub fn takeover(&mut self, confirm: bool) -> Result<crate::ProjectStatus, StoreError> {
        self.reread_project_file();
        if self.follow_if_newer() {
            return Err(self.newer_takeover_refused());
        }
        if !self.holding() {
            if !crate::reserved::dot_is_dir(&self.project_dir) {
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

    /// The heavy work ticks under [`TickOptions`] found due and left for the caller.
    pub fn due_work(&self) -> DueWork {
        self.due
    }

    /// Takes the daily backup a tick found due (failures are retried later, never raised).
    pub fn run_due_backup(&mut self) {
        self.due.backup = false;
        self.daily_tick(crate::commit::now_ms());
    }

    /// Runs the compaction a tick found due.
    pub fn run_due_compaction(&mut self) -> Result<crate::CompactReport, StoreError> {
        self.due.compaction = false;
        crate::compact::run(self, crate::commit::now_ms(), false)
    }

    /// Whether the daily backup of unix ms `now_ms` is still to take.
    pub(crate) fn backup_due(&self, now_ms: i64) -> bool {
        let today = crate::localdate::local_ymd(now_ms);
        self.daily.done != Some(today)
            && !self
                .daily
                .failed_at
                .is_some_and(|t| now_ms.saturating_sub(t) < BACKUP_RETRY_MS)
    }

    /// Queues [`StoreEvent::Rebuilt`] for the next tick.
    pub(crate) fn queue_rebuilt(&mut self) {
        let incarnation = self.incarnation();
        self.follow.queued.push(StoreEvent::Rebuilt { incarnation });
    }

    /// The daily backup of a poll (module docs, step 5); failures are retried later, never
    /// raised.
    fn daily_tick(&mut self, now_ms: i64) {
        if !self.backup_due(now_ms) {
            return;
        }
        match self.daily_backup(now_ms) {
            Ok(_) => self.backup_done(now_ms),
            Err(_) => self.backup_failed(now_ms),
        }
    }

    /// The local day of unix ms `now_ms` has its backup.
    pub(crate) fn backup_done(&mut self, now_ms: i64) {
        self.daily.done = Some(crate::localdate::local_ymd(now_ms));
        self.daily.failed_at = None;
    }

    /// A backup attempt at unix ms `now_ms` failed: retried after [`BACKUP_RETRY_MS`].
    pub(crate) fn backup_failed(&mut self, now_ms: i64) {
        self.daily.failed_at = Some(now_ms);
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
            server_lock,
            ..
        } = &mut self.ctl;
        contender.get_or_insert_with(|| {
            let c = Contender::new(&dir, &self.server_id, holder, params.clone());
            match server_lock {
                Some(l) => c.with_server_lock(l.clone()),
                None => c,
            }
        })
    }

    /// One acquisition attempt. Never creates `.finisher/`: a project folder that vanished
    /// must not come back empty.
    pub(crate) fn try_acquire(&mut self) -> bool {
        if self.follow_if_newer() {
            return false;
        }
        if !crate::reserved::dot_is_dir(&self.project_dir) {
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
        let previous = lease.previous();
        let missing = previous.record.is_none() && !previous.damaged;
        let wait = previous.other_writer_since(&self.server_id, known)
            || (missing && self.folder_has_other_writes(known));
        let now = self.ctl.now();
        let window = dir_cache_window(self.storage_kind());
        self.ctl.gate = Gate::Pending {
            wait_until: wait.then(|| now + window),
        };
        self.ctl.contender = None;
        self.ctl.check_error = None;
        self.ctl.seen = lease.state();
        self.export.lease = StoreLease::Lease(lease);
        self.phase.io_gen += 1;
    }

    /// `lease.json` was missing (deleted, or not synced yet): whether the folder still shows
    /// another server's writes (a segment by another writer, or above the epochs we know).
    /// A folder that cannot be listed counts as yes.
    fn folder_has_other_writes(&mut self, known: u64) -> bool {
        match self
            .import
            .scanner
            .scan_for(&self.project_dir, self.storage_kind())
        {
            Ok(entries) => entries.iter().any(|e| {
                matches!(e, crate::scan::Entry::Segment { seg, .. }
                    if seg.header.writer_id != self.server_id || seg.header.epoch > known)
            }),
            Err(_) => true,
        }
    }

    /// What the fencing re-check of a poll found (module docs, step 3).
    fn recheck_result(&mut self, checked: Result<u64, LeaseLost>) {
        match checked {
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
            self.phase.io_gen += 1;
        }
    }

    /// After an import pass that started at `started` (by the lease's clock): queues its
    /// events, notes what it found for status and lifts the ordering rule's block when it may
    /// (module docs). `copies`: the project file's conflict copies, listed with the pass.
    fn after_pass(&mut self, started: Instant, r: &ImportReport, copies: Vec<String>) {
        // A newer project file or segment lets go of the lease (`crate::newer`).
        self.follow_if_newer();
        self.follow
            .queued
            .extend(r.rewound.iter().map(|w| StoreEvent::Rewound {
                branch: w.branch.clone(),
                from_rev: w.from_rev,
                quarantined: w.quarantined,
                own: w.own,
                generation: w.generation,
            }));
        self.follow
            .queued
            .extend(r.applied.iter().cloned().map(StoreEvent::Applied));
        let kind = self.storage_kind();
        self.follow.observe(r, kind, &self.project_dir, copies);
        if let Gate::Pending { wait_until } = self.ctl.gate
            && wait_until.is_none_or(|t| started >= t)
            && r.waiting.is_empty()
            && r.error.is_none()
            && !(kind.is_sync() && self.follow.arriving())
            && self.holding()
        {
            self.ctl.gate = Gate::Open;
        }
    }

    /// What the project file says now: a newer `minReaderVersion` or `schemaVersion` (another
    /// machine upgraded) makes the project `NEWER_FORMAT` ([`ProjectFile::is_newer`]); a file
    /// that cannot be read changes nothing.
    ///
    /// [`ProjectFile::is_newer`]: crate::ProjectFile::is_newer
    fn reread_project_file(&mut self) {
        if let Ok(f) = crate::project_file::read_project_file_once(&self.project_dir) {
            self.observe_project_file(&f);
        }
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
        self.phase.io_gen += 1;
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

#[cfg(any(test, feature = "test-hooks"))]
pub mod hooks;
mod phased;
#[cfg(test)]
mod tests;

pub use phased::StoreAccess;
pub(crate) use phased::{Direct, Phase};
