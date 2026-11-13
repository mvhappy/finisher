//! The phased tick ([`Store::tick_phased`]): a tick's folder I/O runs without the store, so
//! short calls (reads, commits) are not held up by a follower's import or a holder's export
//! (module docs, "Locking").

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use fin_core::CoreError;

use super::{TickOptions, TickReport, poll_every};
use crate::export::{self, StoreLease};
use crate::import::{ImportPlan, ImportRead, ImportReport};
use crate::lease::{LeaseFence, LeaseLost};
use crate::{Store, StoreError};

/// Where a phased tick finds the store between its steps ([`Store::tick_phased`]): typically
/// a mutex the caller's other threads share it through.
pub trait StoreAccess {
    /// Runs `f` on the store (under the caller's lock), or `None` when the store is not
    /// available now (closed, or taken by a long call): the tick then stops.
    fn with<R>(&mut self, f: impl FnOnce(&mut Store) -> R) -> Option<R>;
}

/// A store the caller already has: the unphased tick, `close`, open and the long calls.
pub(crate) struct Direct<'a>(pub(crate) &'a mut Store);

impl StoreAccess for Direct<'_> {
    fn with<R>(&mut self, f: impl FnOnce(&mut Store) -> R) -> Option<R> {
        Some(f(self.0))
    }
}

/// The phased tick's state in the store.
#[derive(Debug, Default)]
pub(crate) struct Phase {
    /// Bumped by everything that changes what an import pass reads the folder against: an
    /// applied pass, a marked export, compaction, a rebuild, a recovery, the lease in force
    /// changing. A pass whose read started at another value is not applied.
    pub(crate) io_gen: u64,
    /// A tick is running: one at a time ([`TickGuard`]).
    ticking: Arc<AtomicBool>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) hooks: super::hooks::TickHooks,
}

/// Held while a tick runs; dropping it lets the next one start, also when the store is gone.
struct TickGuard(Arc<AtomicBool>);

impl TickGuard {
    fn take(flag: &Arc<AtomicBool>) -> Option<TickGuard> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| TickGuard(flag.clone()))
    }
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// An import pass as a tick runs it: planned under the lock, read without it.
pub(crate) struct PassPlan {
    started: Instant,
    plan: ImportPlan,
}

fn overtaken() -> StoreError {
    CoreError::new("INTERNAL", "the import pass was overtaken by another").into()
}

impl Store {
    /// [`Store::tick_with`] through `access`, taking the store only around database work and
    /// in-memory steps: the import's scan, the export's writes and the lease re-check run
    /// without it, and what they found is checked again before it is applied (module docs,
    /// "Locking"). `None` when the tick did not run to its end: another tick of this store
    /// was running (one at a time), or `access` lost the store (closed, a long call); the
    /// next tick starts afresh.
    ///
    /// A caller that drains events itself ([`Store::take_events`] inside its `with`, so they
    /// reach its listeners before the lock is released) gets none in the report.
    pub fn tick_phased<A: StoreAccess>(
        access: &mut A,
        now: Instant,
        opts: TickOptions,
    ) -> Result<Option<TickReport>, StoreError> {
        let Some(Some(_guard)) = access.with(|s| TickGuard::take(&s.phase.ticking)) else {
            return Ok(None);
        };
        run(access, now, opts)
    }

    /// Events of import passes (and rebuilds) not yet taken, then a `Status` event queued at
    /// the end of a tick if the status changed.
    pub fn take_events(&mut self) -> Vec<crate::StoreEvent> {
        std::mem::take(&mut self.follow.queued)
    }

    pub(crate) fn pass_plan(&mut self) -> Result<PassPlan, StoreError> {
        Ok(PassPlan {
            started: self.ctl.now(),
            plan: self.import_plan()?,
        })
    }

    /// [`Store::import_pass`] for what `read` found; `None` when it was not applied
    /// ([`Store::import_apply`]).
    pub(crate) fn pass_apply(
        &mut self,
        started: Instant,
        mut read: ImportRead,
    ) -> Result<Option<ImportReport>, StoreError> {
        if let Some(f) = read.project_file.take() {
            self.observe_project_file(&f);
        }
        let copies = std::mem::take(&mut read.project_file_copies);
        let Some(r) = self.import_apply(read)? else {
            return Ok(None);
        };
        self.after_pass(started, &r, copies);
        Ok(Some(r))
    }

    /// An import pass that also lifts the ordering rule's block when it may (module docs).
    /// Its rewinds, then its applied records, are queued for the next `tick`'s events.
    pub(crate) fn import_pass(&mut self) -> Result<ImportReport, StoreError> {
        let p = self.pass_plan()?;
        let read = p.plan.read();
        self.pass_apply(p.started, read)?.ok_or_else(overtaken)
    }

    /// Test hook: this store's tick hooks.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn tick_hooks(&self) -> super::hooks::TickHooks {
        self.phase.hooks.clone()
    }
}

/// One import pass through `access`; `Some(None)` when it was not applied.
fn pass<A: StoreAccess>(access: &mut A) -> Result<Option<Option<ImportReport>>, StoreError> {
    let Some(p) = access.with(Store::pass_plan) else {
        return Ok(None);
    };
    let p = p?;
    let read = p.plan.read();
    access.with(|s| s.pass_apply(p.started, read)).transpose()
}

/// The tick (module docs, "One tick"), one step at a time through `access`.
fn run<A: StoreAccess>(
    access: &mut A,
    now: Instant,
    opts: TickOptions,
) -> Result<Option<TickReport>, StoreError> {
    let mut out = TickReport::default();
    let Some(due) = access.with(|s| s.poll_due(now)) else {
        return Ok(None);
    };
    if due {
        let Some(r) = pass(access)? else {
            return Ok(None);
        };
        out.imports.extend(r);
    }
    let Some(report) = export::export_with(access, now, false)? else {
        return Ok(None);
    };
    out.export = report;
    if out.export.segments_written > 0 {
        access.with(|s| s.follow.last_export_ms = Some(crate::commit::now_ms()));
    }
    if due {
        // The fencing re-check's folder I/O, of the lease in force now (if a real one).
        let Some(fence) = access.with(|s| s.recheck_fence()) else {
            return Ok(None);
        };
        let checked = fence.map(|f| {
            let r = f.check();
            (f, r)
        });
        let Some(again) = access.with(|s| s.lease_steps(checked)) else {
            return Ok(None);
        };
        if again {
            let Some(r) = pass(access)? else {
                return Ok(None);
            };
            out.imports.extend(r);
        }
        access.with(|s| s.backup_step(opts));
        // Holder only, writable, caught up (`compact`); after the export and lease steps.
        let Some(compact) = access.with(|s| s.compact_step(opts)) else {
            return Ok(None);
        };
        out.compact = compact?;
    }
    // Two steps: a caller that drains events in its `with` has them after the first.
    access.with(Store::queue_status_change);
    let Some(events) = access.with(Store::take_events) else {
        return Ok(None);
    };
    out.events = events;
    Ok(Some(out))
}

impl Store {
    /// Whether this tick polls the folder (the cadence, [`poll_every`]).
    fn poll_due(&mut self, now: Instant) -> bool {
        let every = poll_every(self.storage_kind());
        let due = self
            .ctl
            .last_poll
            .is_none_or(|t| now.saturating_duration_since(t) >= every);
        if due {
            self.ctl.last_poll = Some(now);
        }
        due
    }

    /// The lease a poll re-checks: the real one in force while holding (not a test lease).
    fn recheck_fence(&self) -> Option<LeaseFence> {
        match &self.export.lease {
            StoreLease::Lease(l) if !self.ctl.fixed && self.holding() => Some(l.fence()),
            _ => None,
        }
    }

    /// Steps 3–4 of a poll, under the lock: the re-check's verdict (`checked`, taken without
    /// it, applied only to the lease it checked), then the retry. Whether an import pass is to
    /// run at once (an acquisition whose block needs no wait).
    fn lease_steps(&mut self, checked: Option<(LeaseFence, Result<u64, LeaseLost>)>) -> bool {
        if self.ctl.fixed {
            return false;
        }
        if let Some((fence, r)) = checked
            && matches!(&self.export.lease, StoreLease::Lease(l) if fence.is(l))
        {
            self.recheck_result(r);
        }
        if self.holding() {
            return false;
        }
        self.release_lost();
        // Acquiring stays under the lock: a takeover (a long call) must not race it.
        self.try_acquire() && self.ctl.gate_needs_no_wait()
    }

    /// Step 5 of a poll: the daily backup, or noting it due.
    fn backup_step(&mut self, opts: TickOptions) {
        if self.ctl.fixed {
            return;
        }
        let now_ms = crate::commit::now_ms();
        if opts.backup {
            self.daily_tick(now_ms);
        } else if self.backup_due(now_ms) {
            self.due.backup = true;
        }
    }

    fn compact_step(&mut self, opts: TickOptions) -> Result<crate::CompactReport, StoreError> {
        let now_ms = crate::commit::now_ms();
        if opts.compaction {
            return crate::compact::run(self, now_ms, false);
        }
        if crate::compact::due(self)? {
            self.due.compaction = true;
        }
        Ok(crate::CompactReport::default())
    }

    /// Queues [`crate::StoreEvent::Status`] if the status changed (lease ages ignored).
    fn queue_status_change(&mut self) {
        if let Some(status) = self.status_if_changed() {
            let e = crate::StoreEvent::Status(Box::new(status));
            self.follow.queued.push(e);
        }
    }
}
