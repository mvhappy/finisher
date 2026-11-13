//! What a compaction run started against (module docs of `compact`, "Locking"): the store's
//! `io_gen` and the fence of the lease then in force, which its folder I/O runs under.

use super::{Fence, fence_at};
use crate::Store;
use crate::export;

/// Taken with the store when a run starts; checked again by every step with the store.
pub(super) struct Planned {
    io_gen: u64,
    fence: export::Fence,
    #[cfg(any(test, feature = "test-hooks"))]
    hooks: crate::tick::hooks::TickHooks,
}

impl Planned {
    pub(super) fn new(store: &Store) -> Planned {
        Planned {
            io_gen: store.phase.io_gen,
            fence: store.export.lease.fence(),
            #[cfg(any(test, feature = "test-hooks"))]
            hooks: store.phase.hooks.clone(),
        }
    }

    /// Whether nothing changed what the run's reads are checked against since it started:
    /// the same `io_gen` (no applied import, marked export, other compaction, rebuild,
    /// recovery or lease change) and the same lease in force.
    pub(super) fn fresh(&self, store: &Store) -> bool {
        self.io_gen == store.phase.io_gen && self.fence.is_current(&store.export.lease)
    }

    /// The full lease check before a folder write or delete (folder I/O, without the store),
    /// at `min_epoch` ([`fence_at`]).
    pub(super) fn check(&self, min_epoch: u64) -> Fence {
        fence_at(self.fence.check(), min_epoch)
    }

    /// With the store, right before a delete: the lease is still the one the run started
    /// with and in force at `min_epoch` (in memory; the full check ran just before).
    pub(super) fn held(&self, store: &Store, min_epoch: u64) -> Fence {
        if !self.fence.is_current(&store.export.lease) {
            return Fence::Lost("the lease in force changed".into());
        }
        fence_at(store.export.lease.quick_check(), min_epoch)
    }

    /// Test hook: a held or panicking test tick stops here (`crate::TickPoint`).
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn at(&self, point: Point) {
        self.hooks.at(point);
    }

    #[cfg(not(any(test, feature = "test-hooks")))]
    pub(super) fn at(&self, _point: Point) {}
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) use crate::TickPoint as Point;

/// The test points outside test builds (never armed).
#[cfg(not(any(test, feature = "test-hooks")))]
#[derive(Clone, Copy)]
pub(super) enum Point {
    CompactList,
    CompactRead,
    CompactWrite,
    CompactDelete,
}
