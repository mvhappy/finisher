//! [`LeaseFence`]: a lease's fence for folder I/O run without the store that holds the lease.

use std::sync::Arc;

use super::clock::lock;
use super::{Inner, Lease, LeaseLost};

/// [`Lease::check`] and [`Lease::quick_check`] of a lease, usable without the store that holds
/// it (a tick's folder I/O, `crate::tick`). A lease the store let go (dropped) fails both,
/// even before its record goes stale.
#[derive(Clone)]
pub(crate) struct LeaseFence(Arc<Inner>);

impl Lease {
    /// A handle on this lease's fence.
    pub(crate) fn fence(&self) -> LeaseFence {
        LeaseFence(self.inner.clone())
    }
}

impl LeaseFence {
    fn dropped(&self) -> Result<(), LeaseLost> {
        if *lock(&self.0.stop) {
            return Err(LeaseLost {
                transient: false,
                reason: "this server let the lease go".to_string(),
            });
        }
        Ok(())
    }

    pub(crate) fn check(&self) -> Result<u64, LeaseLost> {
        self.dropped()?;
        let epoch = self.0.check()?;
        // Let go while the check ran: refused as well.
        self.dropped().map(|()| epoch)
    }

    pub(crate) fn quick_check(&self) -> Result<u64, LeaseLost> {
        self.dropped()?;
        self.0.deadline().map(|()| self.0.epoch)
    }

    /// Whether `lease` is the lease this fence belongs to.
    pub(crate) fn is(&self, lease: &Lease) -> bool {
        Arc::ptr_eq(&self.0, &lease.inner)
    }
}
