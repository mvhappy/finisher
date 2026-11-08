//! The read budget and cancel flag of a probe or hash (plan 5D Decision 36): a reader wrapper
//! over [`fin_jobs::ReadBudget`], so every byte read from a library file is paid for. The
//! worker splits `readBps` over its pool's slots and gives each slot one [`Throttle`];
//! enumeration is metadata only and is not budgeted.

use std::io::{self, Read, Seek, SeekFrom};

use fin_jobs::ReadBudget;

use crate::walk::CancelToken;

/// What bounds a read: an optional bandwidth budget (shared by every clone) and an optional
/// cancel flag. The default bounds nothing.
#[derive(Debug, Clone, Default)]
pub struct Throttle {
    pub budget: Option<ReadBudget>,
    pub cancel: Option<CancelToken>,
}

impl Throttle {
    pub fn new(budget: Option<ReadBudget>, cancel: Option<CancelToken>) -> Self {
        Throttle { budget, cancel }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }

    /// `inner` read through this throttle.
    pub fn reader<R>(&self, inner: R) -> BudgetedReader<R> {
        BudgetedReader {
            inner,
            throttle: self.clone(),
        }
    }
}

/// A `Read + Seek` that takes every byte it reads from the budget (sleeping while the bucket
/// is in debt) and fails the next read once the cancel flag is set. A seek moves nothing and
/// costs nothing.
#[derive(Debug)]
pub struct BudgetedReader<R> {
    inner: R,
    throttle: Throttle,
}

impl<R: Read> Read for BudgetedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.throttle.is_cancelled() {
            // Not `Interrupted`: `read_to_end` and `read_exact` retry that.
            return Err(io::Error::other("cancelled"));
        }
        let n = self.inner.read(buf)?;
        if let (Some(budget), true) = (&self.throttle.budget, n > 0) {
            budget.take(n as u64);
        }
        Ok(n)
    }
}

impl<R: Seek> Seek for BudgetedReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}
