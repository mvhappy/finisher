//! Lease v3: one writer per project folder (spec §3.5; parent spec §27.9 "v3", the design
//! that passed 300 cross-host races with 0 double holders, 0/48 early takeovers and 0/50
//! stale writes, under +9.9 s clock skew).
//!
//! Files in `.finisher/` (the lease never creates that directory: a project folder that
//! vanished must not come back empty):
//! - `lease.json`: the 512-byte checksummed record ([`record`]), created once (staged in a temp
//!   file, then published without replacing: `acquire::take`), then only ever rewritten in
//!   place;
//! - `lease.claim-<epoch:08>`: one per acquisition, created with `create_new` ([`claim`]);
//! - `lease.probe-<serverId>`: each server's probe file, whose mtime is server time
//!   ([`clock`]).
//!
//! # Rules, and where they come from
//!
//! | Rule | Source |
//! |---|---|
//! | TTL 10 s, heartbeat 5 s, stale after 45 s of server time, on every kind | spec §3.5 table |
//! | Takeover after two stale probes ≥ 5 s apart, each with round trip ≤ TTL/4, record unchanged between them | spec §3.5 table; plan Task 9 |
//! | …and the record unchanged for ≥ self-fence + heartbeat (25 s) by the observer's monotonic clock | review of Task 9 (spec amendment pending) |
//! | Staleness evidence expires after 2 × stale-after without a look | review of Task 9 |
//! | A probe is used only if it moved forward and lands ≤ 1 s (+ the previous round trip) below the extrapolated server time; a sample older than 60 s is ignored | spec §3.5 "Probe file"; review of Task 9 |
//! | Sync folders: never automatic, only with confirmation | spec §3.5 table, "On sync folders" |
//! | Every acquisition takes a fresh epoch through a claim | spec §3.5 "Claims and epochs" |
//! | After acquiring, claims below the epoch are deleted | spec §3.5 "Claims and epochs" |
//! | Missing `lease.json` beside any claim = held | spec §3.5 "Record"; §27.9 (Windows not-found cache) |
//! | Unreadable or bad checksum: held when acquiring; when fencing, the write is refused and the lease lost at the self-fence deadline (`fence`) | spec §3.5 "Record", "nothing is lost until the self-fence deadline"; spike comparison (§3.13 item 3) |
//! | A record naming a lower epoch (or another server) over ours: lost, never rewritten (the spike rewrote) | spec §3.5 "Heartbeat and fencing" |
//! | The epoch is checked before every write and after every heartbeat | spec §3.5 "Fencing" |
//! | Self-fence at last successful heartbeat + 2×TTL, monotonic | spec §3.5 "Fencing" |
//! | The monotonic clock counts time asleep ([`SystemClock`]) | review of Task 9 (a sleeping Mac must wake fenced) |
//! | A heartbeat re-checks stop, lost and the deadline right before writing, and a beat finishing past the deadline revives nothing | spec §3.5 "Fencing"; review of Task 9 |
//! | A claim above our epoch, younger than 45 s of server time = lost (claims are the authority on epochs) | §27.9 fenced design ("no newer epoch claim"); second review of Task 9 (bounded lockout) |
//! | One claim per acquisition (`base + 1`); taken → back off creating nothing | second review of Task 9 |
//! | A new epoch is above every claim, segment epoch and our own last epoch | second review of Task 9 |
//! | Never compare the local wall clock with a server mtime | §27.9 "Clock skew" |
//! | Lease reads are uncached, with delete sharing on Windows | §27.9 "Reads", "Windows SMB client" |
//!
//! # After every acquisition
//!
//! A new [`Lease`] (from [`Lease::acquire`], [`Contender::try_acquire`] or
//! [`Contender::takeover`]) only says no one else writes from now on. What the previous
//! holder exported up to the takeover is not yet in our database, so the owner must block
//! commits and exports until an import pass at the new epoch has resolved every branch.
//! Until then a commit would extend a line the folder has already moved past. `Store` does
//! this (its `tick` module, "The ordering rule"), using [`Lease::previous`] to decide whether
//! it must first wait out the directory-cache window.
//!
//! # Holding
//!
//! A [`Lease`] owns a heartbeat thread (stopped and joined on drop; dropping releases
//! nothing, so others wait out staleness). [`Lease::check`] is the fence: it reads the record
//! and returns the epoch only while the record is still exactly what this lease last wrote and
//! the self-fence deadline has not passed. Once lost, a lease stays lost; re-acquiring makes a
//! new one at a new epoch.

mod acquire;
pub mod claim;
pub mod clock;
mod fence;
mod heartbeat;
#[cfg(any(test, feature = "test-hooks"))]
mod hooks;
mod previous;
pub mod record;
mod refusal;
mod registry;
mod server_lock;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fin_core::CoreError;
use serde::Serialize;
use serde_json::json;

use crate::storage_kind::StorageKind;
pub use acquire::Contender;
#[cfg(any(test, feature = "test-hooks"))]
pub use clock::FakeClock;
use clock::lock;
pub use clock::{MonoClock, ServerClock, SystemClock};
use fence::Fence;
#[cfg(any(test, feature = "test-hooks"))]
pub use hooks::{
    HangPoint, LeaseHooks, ReadFault, crash_before_publishing_record_for_test,
    deny_claim_creates_for_test, fault_record_reads_for_test, no_hard_links_for_test,
};
pub use previous::Previous;
use record::LeaseRecord;
pub use server_lock::ServerLock;

#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub use registry::forget_process_for_test;
/// A heartbeat that failed is tried again this soon (monotonic).
pub const RETRY_AFTER: Duration = Duration::from_secs(1);
/// Dropping a lease waits this long for its heartbeat thread (blocked in folder I/O on a
/// hung share, it is left to finish on its own).
const JOIN_WAIT: Duration = Duration::from_secs(2);

/// Lease timing. [`Params::for_kind`] gives the spec's values.
#[derive(Debug, Clone)]
pub struct Params {
    pub ttl: Duration,
    pub heartbeat: Duration,
    /// Server time since the holder's last stamp after which it looks stopped.
    pub stale_after: Duration,
    /// The second stale probe comes at least this long (monotonic) after the first.
    pub confirm_gap: Duration,
    /// A probe slower than this does not count towards staleness (TTL/4).
    pub max_rtt: Duration,
    /// The holder self-fences this long (monotonic) after its last successful heartbeat
    /// (2×TTL).
    pub self_fence: Duration,
    /// Staleness is confirmed only once the record has also been seen unchanged this long by
    /// the observer's own monotonic clock (self-fence + heartbeat). A holder that is running
    /// rewrites its record within that time or has already fenced itself, so it is never
    /// taken over, whatever either clock says.
    pub min_unchanged: Duration,
    /// Whether a confirmed-stale lease is taken over without asking (not on sync folders).
    pub auto_takeover: bool,
    pub clock: Arc<dyn MonoClock>,
    /// How often the heartbeat thread looks at the clock.
    pub poll: Duration,
}

impl Params {
    pub fn for_kind(kind: StorageKind) -> Params {
        let ttl = Duration::from_secs(10);
        Params {
            ttl,
            heartbeat: Duration::from_secs(5),
            stale_after: Duration::from_secs(45),
            confirm_gap: Duration::from_secs(5),
            max_rtt: ttl / 4,
            self_fence: ttl * 2,
            min_unchanged: ttl * 2 + Duration::from_secs(5),
            auto_takeover: !kind.is_sync(),
            clock: Arc::new(SystemClock),
            poll: Duration::from_millis(250),
        }
    }
}

/// The lease as one server sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LeaseState {
    /// This server writes, at `epoch`.
    Held { epoch: u64 },
    /// Another server holds it. `age_server_ms`: server time since its last stamp, when this
    /// round's probe could tell.
    HeldElsewhere {
        holder: String,
        epoch: u64,
        age_server_ms: Option<i64>,
    },
    /// The holder is confirmed stopped, but this storage kind needs confirmation to take over.
    Stale { holder: String, epoch: u64 },
    /// This server's lease is lost (or could not be acquired: the folder is unreachable).
    Lost { reason: String },
}

/// Why [`Lease::check`] refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseLost {
    pub reason: String,
    /// Only this check failed (the folder could not be listed, or `lease.json` could not be
    /// read as one valid record): the write is refused but the lease is not lost.
    pub transient: bool,
}

impl From<LeaseLost> for CoreError {
    fn from(l: LeaseLost) -> Self {
        CoreError::new("LEASE_LOST", format!("the lease is lost: {}", l.reason))
            .with_details(json!({ "cause": l.reason }))
            .with_fix("the lease is re-acquired automatically when possible; otherwise take the project over")
    }
}

/// A server id usable in the record and in a probe file name: 1–64 lowercase ASCII letters
/// and digits ([`crate::server_id()`] gives a lowercase ULID).
pub fn valid_server_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// "`<machine>` (finisher `<app_version>`)", for [`LeaseRecord::holder`].
pub fn describe_holder(app_version: &str) -> String {
    format!("{} (finisher {app_version})", host_name())
}

pub(crate) fn host_name() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer outlives the call and its length is passed.
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
            let n = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            if let Ok(s) = std::str::from_utf8(&buf[..n])
                && !s.is_empty()
            {
                return s.to_string();
            }
        }
    }
    #[cfg(windows)]
    if let Ok(s) = std::env::var("COMPUTERNAME") {
        return s;
    }
    "unknown host".to_string()
}

/// A held lease. `Send + Sync`: the store fences from its own thread while the heartbeat
/// thread runs.
pub struct Lease {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

pub(crate) struct Inner {
    dir: PathBuf,
    key: registry::Key,
    epoch: u64,
    previous: Previous,
    /// The server-data lock, held as long as anything of this lease (its heartbeat thread
    /// included) lives.
    _server_lock: Option<server_lock::Held>,
    params: Params,
    /// All `lease.json` I/O of this lease, so a fence never reads half of its own heartbeat.
    io: Mutex<Io>,
    status: Mutex<Status>,
    stop: Mutex<bool>,
    wake: Condvar,
    exited: Mutex<bool>,
    exited_cv: Condvar,
    #[cfg(any(test, feature = "test-hooks"))]
    hang: hooks::Hang,
}

struct Io {
    clock: ServerClock,
    /// What this lease last wrote and verified (or adopted, see [`Inner::verify`]).
    last: LeaseRecord,
}

struct Status {
    lost: Option<String>,
    /// The monotonic instant the last successful heartbeat (or the acquisition) stamped.
    last_ok: Instant,
    heartbeats: u64,
    /// When the last full check that passed started (monotonic).
    last_checked: Option<Instant>,
    /// Looks that could not tell so far ([`Inner::doubt`]): a check stores `last_checked`
    /// only if none happened while it ran.
    doubts: u64,
    failed: u64,
    fail_next: u32,
}

impl Lease {
    /// One acquisition attempt: takes a free lease, or our own stopped one. A stale holder
    /// is never taken over here, since that needs two probes ≥ 5 s apart; keep a [`Contender`]
    /// across attempts for that. See the module docs for what the owner must do after every
    /// acquisition (import before writing).
    pub fn acquire(
        dir: &Path,
        server_id: &str,
        holder: &str,
        params: Params,
    ) -> Result<Lease, LeaseState> {
        Contender::new(dir, server_id, holder, params).try_acquire()
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        dir: &Path,
        key: registry::Key,
        params: Params,
        clock: ServerClock,
        record: LeaseRecord,
        stamped: Instant,
        previous: Previous,
        server_lock: Option<server_lock::Held>,
    ) -> Lease {
        registry::started(&key, &record);
        let inner = Arc::new(Inner {
            dir: dir.to_path_buf(),
            key,
            epoch: record.epoch,
            previous,
            _server_lock: server_lock,
            params,
            io: Mutex::new(Io {
                clock,
                last: record,
            }),
            status: Mutex::new(Status {
                lost: None,
                last_ok: stamped,
                heartbeats: 0,
                last_checked: None,
                doubts: 0,
                failed: 0,
                fail_next: 0,
            }),
            stop: Mutex::new(false),
            wake: Condvar::new(),
            exited: Mutex::new(false),
            exited_cv: Condvar::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            hang: hooks::Hang::default(),
        });
        let for_thread = inner.clone();
        let thread = std::thread::Builder::new()
            .name("fin-lease-heartbeat".into())
            .spawn(move || heartbeat::run(&for_thread));
        let thread = match thread {
            Ok(t) => Some(t),
            Err(e) => {
                inner.set_lost(format!("cannot start the heartbeat thread: {e}"));
                heartbeat::exited(&inner);
                None
            }
        };
        Lease { inner, thread }
    }

    pub fn epoch(&self) -> u64 {
        self.inner.epoch
    }

    /// What this acquisition replaced ([`Previous`]).
    pub fn previous(&self) -> &Previous {
        &self.inner.previous
    }

    /// The monotonic clock this lease measures its deadlines on.
    pub fn clock(&self) -> &Arc<dyn MonoClock> {
        &self.inner.params.clock
    }

    /// `Held` or `Lost`, without folder I/O (the self-fence deadline is applied).
    pub fn state(&self) -> LeaseState {
        match self.inner.deadline() {
            Ok(()) => LeaseState::Held {
                epoch: self.inner.epoch,
            },
            Err(l) => LeaseState::Lost { reason: l.reason },
        }
    }

    /// The fence, called before every write to the folder (and every commit): the epoch, if
    /// this lease is held right now.
    ///
    /// 1. Lost before → lost.
    /// 2. The self-fence deadline (last successful heartbeat + 2×TTL, monotonic, counting
    ///    sleep) passed → lost.
    /// 3. `lease.json`, read uncached: missing, or a record naming another server or another
    ///    epoch → lost. Unreadable, or a bad checksum after re-reading → this write is refused
    ///    with [`LeaseLost::transient`] set, and the lease is **not** lost (`fence` module).
    /// 4. A claim file above our epoch, younger than 45 s of server time → lost. If `.finisher`
    ///    cannot be listed (after retrying transient errors), this write is refused with
    ///    [`LeaseLost::transient`] set, and the lease is **not** lost.
    /// 5. The deadline again (the read may have waited on a slow share).
    ///
    /// It does folder I/O and can wait on a heartbeat in progress: never call it under a
    /// database lock ([`Lease::quick_check`] is the in-memory part).
    pub fn check(&self) -> Result<u64, LeaseLost> {
        self.inner.check()
    }

    /// The in-memory part of [`Lease::check`] (steps 1–2), for use under a database lock:
    /// lost (by the heartbeat or an earlier check, including a newer claim it saw) or past
    /// the self-fence deadline. No folder I/O, never blocks on the heartbeat.
    pub fn quick_check(&self) -> Result<u64, LeaseLost> {
        self.inner.deadline().map(|()| self.inner.epoch)
    }

    /// [`Lease::check`], unless a full check passed within `max_age` (monotonic): then only
    /// [`Lease::quick_check`]. For commits only, which write the local database, never the
    /// folder (see `Store::commit`); every folder write takes a full check.
    pub fn check_recent(&self, max_age: Duration) -> Result<u64, LeaseLost> {
        let recent = lock(&self.inner.status)
            .last_checked
            .is_some_and(|t| self.inner.now().saturating_duration_since(t) <= max_age);
        if recent {
            self.quick_check()
        } else {
            self.check()
        }
    }

    /// Successful heartbeats so far.
    pub fn heartbeats(&self) -> u64 {
        lock(&self.inner.status).heartbeats
    }

    /// Heartbeats that failed and were retried.
    pub fn failed_heartbeats(&self) -> u64 {
        lock(&self.inner.status).failed
    }

    /// Test hooks that outlive the lease (hang points, the heartbeat thread's end).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn hooks(&self) -> LeaseHooks {
        LeaseHooks(self.inner.clone())
    }

    /// Test hook: the next `n` heartbeats fail before touching the folder (`u32::MAX`: all).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn fail_heartbeats(&self, n: u32) {
        lock(&self.inner.status).fail_next = n;
    }
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("dir", &self.inner.dir)
            .field("epoch", &self.inner.epoch)
            .field("state", &self.state())
            .finish()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        *lock(&self.inner.stop) = true;
        self.inner.wake.notify_all();
        let exited = lock(&self.inner.exited);
        let (exited, _) = self
            .inner
            .exited_cv
            .wait_timeout_while(exited, JOIN_WAIT, |e| !*e)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let done = *exited;
        drop(exited);
        if done && let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // A thread still stuck in I/O checks `stop` before any write.
        registry::stopped(&self.inner.key, self.inner.epoch);
    }
}

impl Inner {
    fn path(&self) -> PathBuf {
        self.dir.join(record::FILE)
    }

    fn now(&self) -> Instant {
        self.params.clock.now()
    }

    /// Steps 1–2 of [`Lease::check`]: lost before, or past the self-fence deadline. In
    /// memory only.
    fn deadline(&self) -> Result<(), LeaseLost> {
        let now = self.now();
        let mut s = lock(&self.status);
        if s.lost.is_none() && now >= s.last_ok + self.params.self_fence {
            let reason = format!(
                "self-fenced: no successful heartbeat for {} s",
                self.params.self_fence.as_secs()
            );
            self.lose(&mut s, reason);
        }
        match &s.lost {
            Some(reason) => Err(LeaseLost {
                transient: false,
                reason: reason.clone(),
            }),
            None => Ok(()),
        }
    }

    fn check(&self) -> Result<u64, LeaseLost> {
        let started = self.now();
        let doubts = lock(&self.status).doubts;
        self.deadline()?;
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.hang
                .full_checks
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.hang.at(HangPoint::CheckBeforeRead);
        }
        let looked = {
            let mut io = lock(&self.io);
            self.verify(&mut io)
                .map(|()| io.clock.now_server_ms(self.now()))
        };
        #[cfg(any(test, feature = "test-hooks"))]
        self.hang.at(HangPoint::CheckAfterVerify);
        match looked.and_then(|server_now| self.no_newer_claim(server_now)) {
            Ok(()) => {}
            Err(Fence::Lost(reason)) => return Err(self.set_lost(reason)),
            // This write is refused; the lease stands (the next check looks again).
            Err(Fence::Unsure(reason)) => {
                self.doubt();
                return Err(LeaseLost {
                    reason,
                    transient: true,
                });
            }
        }
        self.deadline()?;
        // A beat that doubted while this check ran (after its record read) outranks it: a
        // commit must not trust this check then.
        let mut s = lock(&self.status);
        if s.doubts == doubts {
            s.last_checked = Some(started);
        }
        Ok(self.epoch)
    }

    /// Marks the lease lost (the first reason stays) and returns why it is lost.
    fn set_lost(&self, reason: String) -> LeaseLost {
        let mut s = lock(&self.status);
        self.lose(&mut s, reason);
        LeaseLost {
            transient: false,
            reason: s.lost.clone().unwrap_or_default(),
        }
    }

    /// A lost lease never writes again (every write re-checks `lost`), so this process may
    /// re-acquire at once even if its heartbeat thread is still stuck in I/O.
    fn lose(&self, s: &mut Status, reason: String) {
        if s.lost.is_none() {
            s.lost = Some(reason);
            registry::stopped(&self.key, self.epoch);
        }
        self.wake.notify_all();
    }
}
