//! Clocks for the lease (spec §3.5 "Probe file"; parent spec §27.9 "Clock skew" and "Time").
//!
//! The lease never compares this machine's wall clock with a file server's mtime: a client
//! 9.7 s ahead of the NAS split-brained the hardened lease in 105/105 races. Two clocks are
//! used instead:
//!
//! - the **monotonic** clock ([`MonoClock`]) for everything measured on this machine: the
//!   self-fence deadline, the heartbeat schedule, the gap between two staleness probes and a
//!   probe's round trip;
//! - **server time**, read from the mtime of our own probe file right after rewriting it
//!   ([`ServerClock::probe`]), then carried forward by monotonic elapsed time
//!   ([`ServerClock::now_server_ms`]). Every server time in the lease (the holder's stamp in
//!   the record, an observer's "now", a claim file's age) comes from the same file server.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::fsio;

/// A sample older than this (by the monotonic clock) no longer gives server time…
pub const SAMPLE_MAX_AGE: Duration = Duration::from_secs(60);
/// …and the holder re-probes once its sample is this old (a synced host still drifted about
/// 29 ms/h against the NAS, parent spec §27.9).
pub const REPROBE_EVERY: Duration = Duration::from_secs(30);
/// A probe whose mtime lands further than this below the server time extrapolated from the
/// previous sample is rejected.
pub const MAX_BEHIND: Duration = Duration::from_secs(1);

/// The probe file's content length. The content changes every probe (a counter), so no
/// filesystem can skip the write.
const PROBE_LEN: usize = 32;

/// A monotonic clock that keeps counting while the machine sleeps. [`SystemClock`] in
/// production; tests inject a [`FakeClock`] through [`super::Params::clock`]. Every lease
/// deadline (self-fence, heartbeat, sample age, confirm gap, unchanged time) is measured on
/// it, so all instants a lease compares must come from the same `MonoClock`.
pub trait MonoClock: Send + Sync + fmt::Debug {
    fn now(&self) -> Instant;

    /// Milliseconds added to the mtime of a probe file just written, i.e. to the file
    /// server's *current* time. Always 0 for a real clock. A [`FakeClock`] moves the server's
    /// clock along with itself, so tests can let 45 s of server time pass without waiting;
    /// files written before an advance keep their real mtimes and so look as much older.
    #[doc(hidden)]
    fn server_shift_ms(&self) -> i64 {
        0
    }
}

/// The system's monotonic clock **including time asleep**, as `Instant`s.
///
/// `Instant` itself stops while the machine sleeps on Linux (`CLOCK_MONOTONIC`) and macOS
/// (`CLOCK_UPTIME_RAW`). A holder that slept 10 minutes would then wake believing its last
/// heartbeat was a moment ago and write as holder, after another server took over. So:
/// - Linux/Android: `clock_gettime(CLOCK_BOOTTIME)`;
/// - macOS/iOS: `clock_gettime(CLOCK_MONOTONIC)`, which Apple documents as continuing to
///   increment while the system is asleep (unlike `CLOCK_UPTIME_RAW`);
/// - Windows: `QueryInterruptTimePrecise`. Interrupt time includes time in sleep and
///   hibernation (the "unbiased" variant excludes it). `Instant` uses QPC there, whose
///   behaviour across sleep Microsoft does not state clearly, so it is not relied on;
/// - other Unix: `CLOCK_MONOTONIC`; elsewhere `Instant` itself.
///
/// The value is mapped onto `Instant` as `start + (clock now − clock at start)`, with one
/// process-wide start, so every `SystemClock` shares one timeline.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl MonoClock for SystemClock {
    fn now(&self) -> Instant {
        static START: std::sync::OnceLock<(Instant, Option<Duration>)> = std::sync::OnceLock::new();
        let (start, at_start) = *START.get_or_init(|| (Instant::now(), awake_or_asleep()));
        match (at_start, awake_or_asleep()) {
            (Some(a), Some(b)) => start + b.saturating_sub(a),
            _ => Instant::now(),
        }
    }
}

/// Time since an arbitrary point, counting suspend (see [`SystemClock`]).
#[cfg(unix)]
fn awake_or_asleep() -> Option<Duration> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let id = libc::CLOCK_BOOTTIME;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let id = libc::CLOCK_MONOTONIC;
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    if unsafe { libc::clock_gettime(id, &mut ts) } != 0 {
        return None;
    }
    let secs = u64::try_from(ts.tv_sec).ok()?;
    let nanos = u32::try_from(ts.tv_nsec).ok()?;
    Some(Duration::new(secs, nanos))
}

#[cfg(windows)]
fn awake_or_asleep() -> Option<Duration> {
    let mut t = 0u64;
    // SAFETY: `t` is a valid, writable u64 for the duration of the call.
    unsafe { windows_sys::Win32::System::WindowsProgramming::QueryInterruptTimePrecise(&mut t) };
    // 100 ns units.
    Some(Duration::from_nanos(t.saturating_mul(100)))
}

#[cfg(not(any(unix, windows)))]
fn awake_or_asleep() -> Option<Duration> {
    None
}

/// Test clock: stands still until [`FakeClock::advance`]d. The folder's mtimes are real, but
/// [`MonoClock::server_shift_ms`] shifts them by how far this clock has run ahead of real
/// time, so server time follows the fake clock.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug)]
pub struct FakeClock {
    base: Instant,
    state: Mutex<(Duration, Duration)>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl FakeClock {
    pub fn new() -> Arc<FakeClock> {
        Arc::new(FakeClock {
            base: Instant::now(),
            state: Mutex::new((Duration::ZERO, Duration::ZERO)),
        })
    }

    pub fn advance(&self, by: Duration) {
        lock(&self.state).0 += by;
    }

    /// Every [`MonoClock::now`] call also advances the clock by `step` (a slow file server:
    /// each probe's round trip is at least `step`).
    pub fn set_step(&self, step: Duration) {
        lock(&self.state).1 = step;
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl MonoClock for FakeClock {
    fn now(&self) -> Instant {
        let mut s = lock(&self.state);
        let step = s.1;
        s.0 += step;
        self.base + s.0
    }

    fn server_shift_ms(&self) -> i64 {
        let ahead = lock(&self.state).0;
        let real = self.base.elapsed();
        millis(ahead) - millis(real)
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn millis(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// An mtime (server time) in Unix milliseconds, negative before 1970.
pub(crate) fn unix_ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => millis(d),
        Err(e) => -millis(e.duration()),
    }
}

/// One probe: the file server's time when it applied our write, how long the probe took, and
/// the monotonic instant it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeSample {
    pub server_ms: i64,
    pub rtt: Duration,
    /// When the probe started. The server applied the write between `at` and `at + rtt`, so
    /// `server_ms + (t - at)` never underestimates server time at `t`: a holder's stamp errs
    /// towards "fresh", never towards "stale".
    pub at: Instant,
}

/// A probe and whether it moved forward from the previous accepted one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub sample: ProbeSample,
    /// The mtime moved forward from the last accepted sample (or there was none). A probe
    /// that did not is not accepted: the stat was probably served from a cache (spec §3.5:
    /// "if it doesn't, staleness is not decided that round").
    pub advanced: bool,
}

/// Server time from `lease.probe-<serverId>`. Each server only ever writes its own probe file.
#[derive(Debug, Clone)]
pub struct ServerClock {
    path: PathBuf,
    clock: Arc<dyn MonoClock>,
    last: Option<ProbeSample>,
    counter: u64,
    /// The last probe failed or did not advance: probe again at the next chance.
    retry: bool,
}

impl ServerClock {
    pub fn new(dir: &Path, server_id: &str, clock: Arc<dyn MonoClock>) -> ServerClock {
        ServerClock {
            path: dir.join(format!("lease.probe-{server_id}")),
            clock,
            last: None,
            counter: 0,
            retry: false,
        }
    }

    pub fn mono(&self) -> Instant {
        self.clock.now()
    }

    pub fn mono_clock(&self) -> &dyn MonoClock {
        &*self.clock
    }

    /// Rewrites the probe file in place (created on first use), syncs it, then reads its mtime
    /// through a freshly opened handle. The sample is accepted (used by
    /// [`ServerClock::now_server_ms`]) only if it moved forward.
    pub fn probe(&mut self) -> io::Result<Probe> {
        self.retry = true;
        let at = self.clock.now();
        self.counter += 1;
        let mut content = format!("finisher-probe {:016}\n", self.counter).into_bytes();
        content.resize(PROBE_LEN, b' ');
        fsio::write_in_place_or_create(&self.path, &content)?;
        let mtime = fresh_mtime(&self.path)?;
        let server_ms = unix_ms(mtime).saturating_add(self.clock.server_shift_ms());
        let end = self.clock.now();
        let sample = ProbeSample {
            server_ms,
            rtt: end.saturating_duration_since(at),
            at,
        };
        let advanced = self.accepts(&sample);
        if advanced {
            self.last = Some(sample);
            self.retry = false;
        }
        Ok(Probe { sample, advanced })
    }

    /// A sample is accepted when there is no usable previous one (none, or older than
    /// [`SAMPLE_MAX_AGE`], so a NAS clock stepped backwards cannot wedge us), or when it moved
    /// forward from the previous one **and** is not more than [`MAX_BEHIND`] (plus the previous
    /// probe's round trip, by which its extrapolation overestimates) below the time
    /// extrapolated from it: a stale, delayed or client-set mtime is not server time.
    fn accepts(&self, sample: &ProbeSample) -> bool {
        let Some(l) = self.last.filter(|l| {
            sample
                .at
                .checked_duration_since(l.at)
                .is_some_and(|d| d <= SAMPLE_MAX_AGE)
        }) else {
            return true;
        };
        let expected = l
            .server_ms
            .saturating_add(millis(sample.at.saturating_duration_since(l.at)));
        let slack = millis(MAX_BEHIND + l.rtt);
        sample.server_ms > l.server_ms && sample.server_ms >= expected.saturating_sub(slack)
    }

    /// Server time at `at`: the last accepted sample plus monotonic time since it started.
    /// `None` without a sample, or once it is older than [`SAMPLE_MAX_AGE`].
    pub fn now_server_ms(&self, at: Instant) -> Option<i64> {
        let l = self.last?;
        let since = at.checked_duration_since(l.at)?;
        (since <= SAMPLE_MAX_AGE).then(|| l.server_ms.saturating_add(millis(since)))
    }

    /// Whether the holder should probe before stamping at `at`: no sample, one older than
    /// [`REPROBE_EVERY`], or the last probe failed or did not advance.
    pub fn needs_probe(&self, at: Instant) -> bool {
        self.retry
            || self
                .last
                .is_none_or(|l| at.saturating_duration_since(l.at) >= REPROBE_EVERY)
    }
}

/// The mtime of `path` from a newly opened handle (`fstat` / `GetFileInformationByHandle`),
/// not a path lookup, which Windows answers from its 10 s file-info cache. Opened with full
/// sharing on Windows so writers are never blocked.
fn fresh_mtime(path: &Path) -> io::Result<SystemTime> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let file: File = options.open(path)?;
    file.metadata()?.modified()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_must_advance_and_expire() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeClock::new();
        let mut c = ServerClock::new(tmp.path(), "s1", fake.clone());
        assert_eq!(c.now_server_ms(fake.now()), None);
        let p = c.probe().unwrap();
        assert!(p.advanced);
        assert_eq!(std::fs::metadata(&c.path).unwrap().len(), PROBE_LEN as u64);
        let t = fake.now();
        assert_eq!(c.now_server_ms(t), Some(p.sample.server_ms));
        fake.advance(Duration::from_secs(10));
        assert_eq!(
            c.now_server_ms(fake.now()),
            Some(p.sample.server_ms + 10_000)
        );
        assert!(!c.needs_probe(fake.now()));
        fake.advance(Duration::from_secs(20));
        assert!(c.needs_probe(fake.now()));
        fake.advance(Duration::from_secs(31));
        assert_eq!(c.now_server_ms(fake.now()), None, "older than 60 s");
        // Server time moves with the fake clock, so the next probe advances.
        let q = c.probe().unwrap();
        assert!(q.advanced);
        assert!(q.sample.server_ms - p.sample.server_ms >= 60_000);
    }

    #[test]
    fn a_probe_that_does_not_advance_is_not_used() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeClock::new();
        let mut c = ServerClock::new(tmp.path(), "s1", fake.clone());
        let p = c.probe().unwrap();
        // Pretend the last sample came from the future: the next one cannot advance past it.
        c.last = Some(ProbeSample {
            server_ms: p.sample.server_ms + 1_000_000,
            ..p.sample
        });
        let q = c.probe().unwrap();
        assert!(!q.advanced);
        assert_eq!(c.last.unwrap().server_ms, p.sample.server_ms + 1_000_000);
        assert!(c.needs_probe(fake.now()), "retried at the next chance");
    }

    /// An mtime newer than the last sample but well below where server time should be by
    /// now (a stale, delayed or client-set mtime) is not used; extrapolation continues.
    #[test]
    fn a_sample_far_below_the_extrapolation_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeClock::new();
        let mut c = ServerClock::new(tmp.path(), "s1", fake.clone());
        let p = c.probe().unwrap();
        // The last sample says server time was 10 s behind, 12 s ago: about 2 s ahead of
        // what the file shows now.
        let last = ProbeSample {
            server_ms: p.sample.server_ms - 10_000,
            rtt: Duration::ZERO,
            at: p.sample.at - Duration::from_secs(12),
        };
        c.last = Some(last);
        let q = c.probe().unwrap();
        assert!(!q.advanced, "{q:?}");
        assert_eq!(c.last, Some(last));
        assert_eq!(
            c.now_server_ms(q.sample.at),
            Some(last.server_ms + 12_000),
            "still extrapolated from the last good sample"
        );
        // Within the 1 s tolerance it is fine.
        c.last = Some(ProbeSample {
            at: p.sample.at - Duration::from_millis(10_500),
            ..last
        });
        assert!(c.probe().unwrap().advanced);
    }

    /// The NAS clock stepped back an hour: once the old sample is past its 60 s age it no
    /// longer blocks new ones.
    #[test]
    fn an_expired_sample_does_not_block_a_clock_stepped_back() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeClock::new();
        let mut c = ServerClock::new(tmp.path(), "s1", fake.clone());
        let p = c.probe().unwrap();
        c.last = Some(ProbeSample {
            server_ms: p.sample.server_ms + 3_600_000,
            ..p.sample
        });
        assert!(!c.probe().unwrap().advanced, "younger than 60 s: refused");
        fake.advance(Duration::from_secs(61));
        let q = c.probe().unwrap();
        assert!(q.advanced, "{q:?}");
        assert_eq!(c.now_server_ms(q.sample.at), Some(q.sample.server_ms));
    }

    #[test]
    fn the_system_clock_is_monotonic_and_keeps_time() {
        let (a, ia) = (SystemClock.now(), Instant::now());
        std::thread::sleep(Duration::from_millis(30));
        let (b, ib) = (SystemClock.now(), Instant::now());
        let (d, di) = (b - a, ib - ia);
        assert!(d >= Duration::from_millis(29), "{d:?}");
        // Awake, it runs at the rate of `Instant`.
        assert!(
            d.abs_diff(di) < Duration::from_millis(20),
            "{d:?} vs {di:?}"
        );
        let mut last = SystemClock.now();
        for _ in 0..1000 {
            let t = SystemClock.now();
            assert!(t >= last);
            last = t;
        }
    }
}
