//! Lease v3: one writer per project folder (spec §3.5; parent spec §27.9 "v3", the design
//! that passed 300 cross-host races with 0 double holders, 0/48 early takeovers and 0/50
//! stale writes, under +9.9 s clock skew).
//!
//! Files in `.finisher/` (the lease never creates that directory: a project folder that
//! vanished must not come back empty):
//! - `lease.json`: the 512-byte checksummed record ([`record`]), created once with
//!   `create_new`, then only ever rewritten in place;
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
//! | Sync folders: never automatic, only with confirmation | spec §3.5 table, "On sync folders" |
//! | Every acquisition takes a fresh epoch through a claim | spec §3.5 "Claims and epochs" |
//! | After acquiring, claims below the epoch are deleted | spec §3.5 "Claims and epochs" |
//! | Missing `lease.json` beside any claim = held | spec §3.5 "Record"; §27.9 (Windows not-found cache) |
//! | Unreadable or bad checksum: held when acquiring, lost when fencing | spec §3.5 "Record" |
//! | The epoch is checked before every write and after every heartbeat | spec §3.5 "Fencing" |
//! | Self-fence at last successful heartbeat + 2×TTL, monotonic | spec §3.5 "Fencing" |
//! | Never compare the local wall clock with a server mtime | §27.9 "Clock skew" |
//! | Lease reads are uncached, with delete sharing on Windows | §27.9 "Reads", "Windows SMB client" |
//!
//! # After every acquisition
//!
//! A new [`Lease`] (from [`Lease::acquire`], [`Contender::try_acquire`] or
//! [`Contender::takeover`]) only says no one else writes from now on. What the previous
//! holder exported up to the takeover is not yet in our database, so the owner must block
//! commits and exports until an import pass at the new epoch has resolved every branch
//! (plan Task 11). Until then a commit would extend a line the folder has already moved past.
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
mod heartbeat;
pub mod record;
mod registry;

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
use record::LeaseRecord;

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
            auto_takeover: !kind.is_sync(),
            clock: Arc::new(SystemClock),
            poll: Duration::from_millis(250),
        }
    }
}

/// The lease as one server sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

fn host_name() -> String {
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
    params: Params,
    /// All `lease.json` I/O of this lease, so a fence never reads half of its own heartbeat.
    io: Mutex<Io>,
    status: Mutex<Status>,
    stop: Mutex<bool>,
    wake: Condvar,
    exited: Mutex<bool>,
    exited_cv: Condvar,
}

struct Io {
    clock: ServerClock,
    /// What this lease last wrote and verified.
    last: LeaseRecord,
    /// A heartbeat whose write failed: it may or may not have landed.
    pending: Option<LeaseRecord>,
}

struct Status {
    lost: Option<String>,
    /// The monotonic instant the last successful heartbeat (or the acquisition) stamped.
    last_ok: Instant,
    heartbeats: u64,
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

    fn start(
        dir: &Path,
        key: registry::Key,
        params: Params,
        clock: ServerClock,
        record: LeaseRecord,
        stamped: Instant,
    ) -> Lease {
        registry::started(&key, &record);
        let inner = Arc::new(Inner {
            dir: dir.to_path_buf(),
            key,
            epoch: record.epoch,
            params,
            io: Mutex::new(Io {
                clock,
                last: record,
                pending: None,
            }),
            status: Mutex::new(Status {
                lost: None,
                last_ok: stamped,
                heartbeats: 0,
                failed: 0,
                fail_next: 0,
            }),
            stop: Mutex::new(false),
            wake: Condvar::new(),
            exited: Mutex::new(false),
            exited_cv: Condvar::new(),
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
    /// 2. The self-fence deadline (last successful heartbeat + 2×TTL, monotonic) passed →
    ///    lost.
    /// 3. `lease.json`, read uncached: missing, unreadable or a bad checksum → lost; a record
    ///    other than the one this lease last wrote (another server, another epoch, another
    ///    heartbeat) → lost.
    /// 4. The deadline again (the read may have waited on a slow share).
    pub fn check(&self) -> Result<u64, LeaseLost> {
        self.inner.check()
    }

    /// Successful heartbeats so far.
    pub fn heartbeats(&self) -> u64 {
        lock(&self.inner.status).heartbeats
    }

    /// Heartbeats that failed and were retried.
    pub fn failed_heartbeats(&self) -> u64 {
        lock(&self.inner.status).failed
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
    }
}

impl Inner {
    fn path(&self) -> PathBuf {
        self.dir.join(record::FILE)
    }

    fn now(&self) -> Instant {
        self.params.clock.now()
    }

    /// Steps 1–2 of [`Lease::check`].
    fn deadline(&self) -> Result<(), LeaseLost> {
        let now = self.now();
        let mut s = lock(&self.status);
        if s.lost.is_none() && now >= s.last_ok + self.params.self_fence {
            s.lost = Some(format!(
                "self-fenced: no successful heartbeat for {} s",
                self.params.self_fence.as_secs()
            ));
            self.wake.notify_all();
        }
        match &s.lost {
            Some(reason) => Err(LeaseLost {
                reason: reason.clone(),
            }),
            None => Ok(()),
        }
    }

    fn check(&self) -> Result<u64, LeaseLost> {
        self.deadline()?;
        let verdict = self.verify(&mut lock(&self.io));
        if let Err(reason) = verdict {
            return Err(self.set_lost(reason));
        }
        self.deadline()?;
        Ok(self.epoch)
    }

    /// Whether `lease.json` still holds what this lease last wrote (or the heartbeat whose
    /// write failed, which then counts as written).
    fn verify(&self, io: &mut Io) -> Result<(), String> {
        match record::read(&self.path()) {
            Err(e) => Err(format!("lease.json is unreadable: {e}")),
            Ok(record::Seen::Missing) => Err("lease.json is missing".to_string()),
            Ok(record::Seen::Corrupt(_)) => Err(
                "lease.json fails its checksum (damaged or being rewritten by another server)"
                    .to_string(),
            ),
            Ok(record::Seen::Valid(r)) if r == io.last => Ok(()),
            Ok(record::Seen::Valid(r)) if io.pending.as_ref() == Some(&r) => {
                io.pending = None;
                registry::wrote(&self.key, &r);
                io.last = r;
                Ok(())
            }
            Ok(record::Seen::Valid(r)) => Err(format!(
                "lease.json now names server {} at epoch {} (heartbeat {}); this lease was epoch {}",
                r.server_id, r.epoch, r.heartbeat, self.epoch
            )),
        }
    }

    /// Marks the lease lost (the first reason stays) and returns why it is lost.
    fn set_lost(&self, reason: String) -> LeaseLost {
        let mut s = lock(&self.status);
        let reason = s.lost.get_or_insert(reason).clone();
        self.wake.notify_all();
        LeaseLost { reason }
    }
}
