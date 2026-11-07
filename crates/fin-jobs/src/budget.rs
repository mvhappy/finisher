//! The read budget (spec §6.2 "Bandwidth"): a token bucket over `limits.readBytesPerSec`,
//! shared by every clone, refilled per millisecond. The bucket holds at most a tenth of a
//! second of bytes; a `take` larger than what is left goes into debt and sleeps it off, so the
//! long-run rate is exact whatever the read sizes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Time for a [`ReadBudget`]; tests inject one whose `sleep` advances `now`.
pub trait BudgetClock: Send + Sync {
    fn now(&self) -> Duration;
    fn sleep(&self, d: Duration);
}

struct RealClock(Instant);

impl BudgetClock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

struct Bucket {
    /// Bytes available; negative is debt.
    tokens: f64,
    /// The last refill, in whole milliseconds of the clock.
    last_ms: u128,
}

struct Inner {
    rate_per_ms: f64,
    cap: f64,
    clock: Arc<dyn BudgetClock>,
    bucket: Mutex<Bucket>,
}

/// A token bucket over reads. `Clone` shares the bucket.
#[derive(Clone)]
pub struct ReadBudget(Arc<Inner>);

impl std::fmt::Debug for ReadBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadBudget")
            .field("bytes_per_sec", &(self.0.rate_per_ms * 1000.0))
            .finish()
    }
}

impl ReadBudget {
    pub fn new(bytes_per_sec: u64) -> Self {
        ReadBudget::with_clock(bytes_per_sec, Arc::new(RealClock(Instant::now())))
    }

    /// [`ReadBudget::new`] with an injected clock (tests).
    pub fn with_clock(bytes_per_sec: u64, clock: Arc<dyn BudgetClock>) -> Self {
        let rate = bytes_per_sec.max(1) as f64;
        let cap = (rate / 10.0).max(1.0);
        let last_ms = clock.now().as_millis();
        ReadBudget(Arc::new(Inner {
            rate_per_ms: rate / 1000.0,
            cap,
            clock,
            bucket: Mutex::new(Bucket {
                tokens: cap,
                last_ms,
            }),
        }))
    }

    /// Takes `n` bytes, blocking while the bucket is in debt.
    pub fn take(&self, n: u64) {
        let i = &self.0;
        let wait_ms = {
            let mut b = i.bucket.lock().unwrap_or_else(|e| e.into_inner());
            let now = i.clock.now().as_millis();
            let elapsed = now.saturating_sub(b.last_ms);
            b.last_ms = now;
            b.tokens = (b.tokens + elapsed as f64 * i.rate_per_ms).min(i.cap);
            b.tokens -= n as f64;
            if b.tokens < 0.0 {
                (-b.tokens / i.rate_per_ms).ceil()
            } else {
                0.0
            }
        };
        if wait_ms > 0.0 {
            i.clock.sleep(Duration::from_millis(wait_ms as u64));
        }
    }
}
