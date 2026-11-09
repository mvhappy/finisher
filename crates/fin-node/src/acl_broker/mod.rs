//! The ACL broker (plan 7C Decision 18, Task 10): one per server, the only code that changes a
//! DACL for a sandboxed process. It grants and revokes `{path, sid, mask, inherit, owner}` as
//! allow-only ACEs (`fin_sandbox::windows::acl` on Windows; [`apply`]), each operation first
//! appended to `<server-data>/run/acl-broker.wal` and fsynced, then applied, then marked done
//! ([`wal`]). So a crash between grant and revoke never leaves a stale grant:
//!
//! - **Replay at open** ([`Broker::open`]): an incomplete revoke is completed; an incomplete
//!   grant (its caller never saw it succeed) is completed when its owner is live, else revoked.
//!   The log is then rewritten with only what is granted, so a torn last line goes too.
//! - **Sweep** ([`sweep`]): every grant whose owner is not live is revoked, and a revoke that
//!   failed earlier is retried. `finisherd` sweeps before any listener starts, with no owner
//!   live: no session, job or call outlives the server that started it. The owner
//!   [`STATIC_OWNER`] (the toolchain, a project's folder, a sign-in store: granted once, kept)
//!   is always live.
//! - **Protected roots** are refused before anything is written ([`protect`]).
//! - **One path at a time**: a DACL change is a read-modify-write, so two grants on one
//!   directory at once would lose one (the S8 lost-grant race). An operation waits while
//!   another runs on the same path, an ancestor or a descendant (inheritance rewrites the
//!   descendants' DACLs too).
//! - **Bounded**: the log is rewritten (temp file, fsync, rename) once it holds more than
//!   twice the live grants plus the operations not done plus [`Options::compact_slack`] lines.
//!
//! One owner per `(path, sid)`: a second owner's grant of the same pair is refused, since a
//! revoke removes every explicit ACE of that SID on that path. The same owner's repeated grant
//! adds its mask to the record and is skipped when the record already covers it.
//!
//! Napi-free (`crate::sandbox_acl` is the binding), so `tests/acl_broker.rs` includes it.

#![allow(clippy::disallowed_methods)] // the log and the protected-root checks are server data

pub mod apply;
pub mod protect;
pub mod sweep;
pub mod wal;

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use apply::Apply;
use wal::{Record, Wal};

/// The owner of the grants every launch of a kind shares; never revoked by a sweep.
pub const STATIC_OWNER: &str = "static";
/// The log, under `<server-data>/run/`.
pub const WAL_FILE: &str = "acl-broker.wal";
/// Extra lines the log may hold before it is rewritten (the default).
pub const COMPACT_SLACK: usize = 256;

/// One allow ACE and whose it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub path: PathBuf,
    pub sid: String,
    pub mask: u32,
    pub inherit: bool,
    /// `session:<id>`, `job:<id>`, `call:<id>` or [`STATIC_OWNER`].
    pub owner: String,
}

/// Why an operation failed: `code()` is the `CoreError` code the binding answers.
#[derive(Debug)]
pub enum BrokerError {
    /// Malformed: a relative path, a mask beyond modify, a SID or owner that is not one.
    Invalid(String),
    /// A protected root (refused before the log).
    Protected { path: PathBuf, why: &'static str },
    /// The path does not exist (refused before the log).
    NotFound(PathBuf),
    /// Another owner holds this `(path, sid)`.
    Owned { path: PathBuf, owner: String },
    /// The log or the DACL could not be read or written.
    Io(String),
}

impl BrokerError {
    pub fn code(&self) -> &'static str {
        match self {
            BrokerError::Invalid(_) => "INVALID_ARGS",
            BrokerError::Protected { .. } => "PATH_NOT_ALLOWED",
            BrokerError::NotFound(_) => "NOT_FOUND",
            BrokerError::Owned { .. } => "INVALID_OP",
            BrokerError::Io(_) => "IO_ERROR",
        }
    }

    pub fn details(&self) -> Value {
        match self {
            BrokerError::Invalid(m) => json!({ "reason": "aclInvalid", "problem": m }),
            BrokerError::Protected { path, why } => {
                json!({ "reason": "aclProtected", "path": path, "why": why })
            }
            BrokerError::NotFound(p) => json!({ "path": p }),
            BrokerError::Owned { path, owner } => {
                json!({ "reason": "aclOwned", "path": path, "owner": owner })
            }
            BrokerError::Io(m) => json!({ "reason": "aclBroker", "error": m }),
        }
    }
}

impl std::fmt::Display for BrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrokerError::Invalid(m) => write!(f, "not an ACL grant: {m}"),
            BrokerError::Protected { path, why } => {
                write!(f, "{} is protected ({why}): never granted", path.display())
            }
            BrokerError::NotFound(p) => write!(f, "{} does not exist", p.display()),
            BrokerError::Owned { path, owner } => {
                write!(f, "{} is granted to that SID for {owner}", path.display())
            }
            BrokerError::Io(m) => write!(f, "the ACL broker failed: {m}"),
        }
    }
}

fn io_err(what: &str, e: io::Error) -> BrokerError {
    BrokerError::Io(format!("{what}: {e}"))
}

/// A named point where a `test-hooks` build may abort (`FIN_TEST_ACL_CRASH=<op>:<point>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    AfterAppend,
    AfterApply,
}

/// Aborts the process when `FIN_TEST_ACL_CRASH` names `op` and `point` (test-hooks only).
#[cfg(feature = "test-hooks")]
fn crash_point(op: &str, point: Point) {
    let name = match point {
        Point::AfterAppend => "afterAppend",
        Point::AfterApply => "afterApply",
    };
    if std::env::var("FIN_TEST_ACL_CRASH").ok().as_deref() == Some(&format!("{op}:{name}")) {
        std::process::abort();
    }
}

#[cfg(not(feature = "test-hooks"))]
fn crash_point(_op: &str, _point: Point) {}

/// Tunables (tests make the log compact sooner).
#[derive(Debug, Clone)]
pub struct Options {
    pub compact_slack: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            compact_slack: COMPACT_SLACK,
        }
    }
}

struct State {
    wal: Wal,
    next: u64,
    /// Done grants, by the id of the record that holds them.
    granted: BTreeMap<u64, Grant>,
    /// Appended, not done: in flight, or failed and left for the next sweep.
    pending: BTreeMap<u64, Record>,
}

/// The broker of one server's data directory.
pub struct Broker<A: Apply> {
    rules: protect::Rules,
    apply: A,
    opts: Options,
    state: Mutex<State>,
    busy: Mutex<Vec<PathBuf>>,
    freed: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Two keys of one path compare alike (Windows paths compare without case).
pub(crate) fn same_key(a: &Path, b: &Path) -> bool {
    protect::relative(a, b).is_some_and(|r| r.is_empty())
}

/// `a` is `b`, inside it, or contains it.
fn related(a: &Path, b: &Path) -> bool {
    protect::relative(a, b).is_some() || protect::relative(b, a).is_some()
}

/// Releases a path taken by [`Broker::take_path`].
struct PathGuard<'a, A: Apply> {
    broker: &'a Broker<A>,
    path: PathBuf,
}

impl<A: Apply> Drop for PathGuard<'_, A> {
    fn drop(&mut self) {
        let mut busy = lock(&self.broker.busy);
        if let Some(i) = busy.iter().position(|p| p == &self.path) {
            busy.swap_remove(i);
        }
        self.broker.freed.notify_all();
    }
}

impl<A: Apply> Broker<A> {
    /// Opens the broker of `server_data`: reads the log, completes or rolls back what a crash
    /// left (an incomplete grant is completed only when `live(owner)`), and rewrites the log.
    pub fn open(
        server_data: &Path,
        apply: A,
        opts: Options,
        live: &dyn Fn(&str) -> bool,
    ) -> Result<Broker<A>, BrokerError> {
        let run = server_data.join("run");
        std::fs::create_dir_all(&run).map_err(|e| io_err("run directory", e))?;
        let (wal, records) = Wal::open(&run.join(WAL_FILE)).map_err(|e| io_err("log", e))?;
        let replay = wal::replay(&records);
        let next = records.iter().map(Record::id).max().map_or(1, |m| m + 1);
        let broker = Broker {
            rules: protect::Rules::new(server_data),
            apply,
            opts,
            state: Mutex::new(State {
                wal,
                next,
                granted: replay.granted,
                pending: BTreeMap::new(),
            }),
            busy: Mutex::new(Vec::new()),
            freed: Condvar::new(),
        };
        broker.finish(replay.pending, live)?;
        broker.compact_now()?;
        Ok(broker)
    }

    /// Completes the operations a crash left: revokes always, grants of a live owner.
    fn finish(&self, pending: Vec<Record>, live: &dyn Fn(&str) -> bool) -> Result<(), BrokerError> {
        for r in pending {
            match r {
                Record::Grant {
                    id,
                    path,
                    sid,
                    mask,
                    inherit,
                    owner,
                } => {
                    let g = Grant {
                        path,
                        sid,
                        mask,
                        inherit,
                        owner,
                    };
                    lock(&self.state).pending.insert(id, Record::grant(id, &g));
                    if is_live(live, &g.owner)
                        && self.apply.grant(&g.path, &g.sid, g.mask, g.inherit).is_ok()
                    {
                        self.done(id, Some(g))?;
                    } else {
                        // Never marked done: the revoke logged next drops it at replay.
                        lock(&self.state).pending.remove(&id);
                        self.revoke_one(&g.path, &g.sid, &g.owner)?;
                    }
                }
                Record::Revoke {
                    id,
                    ref path,
                    ref sid,
                    ..
                } => {
                    lock(&self.state).pending.insert(id, r.clone());
                    self.apply_revoke(path, sid)?;
                    self.done(id, None)?;
                }
                Record::Done { .. } => {}
            }
        }
        Ok(())
    }

    /// Waits until no operation runs on `path`, an ancestor or a descendant, then takes it.
    fn take_path(&self, path: &Path) -> PathGuard<'_, A> {
        let mut busy = lock(&self.busy);
        while busy.iter().any(|p| related(p, path)) {
            busy = self.freed.wait(busy).unwrap_or_else(|p| p.into_inner());
        }
        busy.push(path.to_path_buf());
        PathGuard {
            broker: self,
            path: path.to_path_buf(),
        }
    }

    /// Appends `r`, fsynced, and keeps it pending until its done marker.
    fn append(&self, r: Record) -> Result<(), BrokerError> {
        let mut s = lock(&self.state);
        s.wal.append(&r).map_err(|e| io_err("log append", e))?;
        s.pending.insert(r.id(), r);
        Ok(())
    }

    fn next_id(&self) -> u64 {
        let mut s = lock(&self.state);
        let id = s.next;
        s.next += 1;
        id
    }

    /// Marks `id` done; a completed grant becomes `granted` (replacing its pair's record).
    fn done(&self, id: u64, granted: Option<Grant>) -> Result<(), BrokerError> {
        let mut s = lock(&self.state);
        s.wal
            .append(&Record::Done { id })
            .map_err(|e| io_err("log append", e))?;
        s.pending.remove(&id);
        if let Some(g) = granted {
            s.granted
                .retain(|_, o| !(same_key(&o.path, &g.path) && o.sid == g.sid));
            s.granted.insert(id, g);
        }
        let limit = 2 * s.granted.len() + s.pending.len() + self.opts.compact_slack;
        if s.wal.lines() > limit {
            Self::compact(&mut s)?;
        }
        Ok(())
    }

    fn compact(s: &mut State) -> Result<(), BrokerError> {
        let mut records = Vec::with_capacity(2 * s.granted.len() + s.pending.len());
        for (id, g) in &s.granted {
            records.push(Record::grant(*id, g));
            records.push(Record::Done { id: *id });
        }
        records.extend(s.pending.values().cloned());
        s.wal
            .rewrite(&records)
            .map_err(|e| io_err("log rewrite", e))
    }

    fn compact_now(&self) -> Result<(), BrokerError> {
        Self::compact(&mut lock(&self.state))
    }

    /// A revoke of a path that is gone has nothing left to do.
    fn apply_revoke(&self, path: &Path, sid: &str) -> Result<(), BrokerError> {
        match self.apply.revoke(path, sid) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err("revoke", e)),
        }
    }

    /// Grants `g`: refused before the log when protected; logged, applied, marked done.
    pub fn grant(&self, g: &Grant) -> Result<(), BrokerError> {
        self.rules.check(g)?;
        if std::fs::symlink_metadata(&g.path).is_err() {
            return Err(BrokerError::NotFound(g.path.clone()));
        }
        let _path = self.take_path(&g.path);
        let mut record = g.clone();
        {
            let s = lock(&self.state);
            let held = s
                .granted
                .values()
                .find(|o| same_key(&o.path, &g.path) && o.sid == g.sid);
            if let Some(o) = held {
                if o.owner != g.owner {
                    return Err(BrokerError::Owned {
                        path: g.path.clone(),
                        owner: o.owner.clone(),
                    });
                }
                if o.mask & g.mask == g.mask && (o.inherit || !g.inherit) {
                    return Ok(());
                }
                record.mask |= o.mask;
                record.inherit |= o.inherit;
            }
        }
        let id = self.next_id();
        self.append(Record::grant(id, &record))?;
        crash_point("grant", Point::AfterAppend);
        if let Err(e) = self.apply.grant(&g.path, &g.sid, g.mask, g.inherit) {
            // Rolled back, never marked done (the logged revoke drops it at replay): the DACL
            // may hold part of it (inheritance stops midway).
            lock(&self.state).pending.remove(&id);
            self.revoke_logged(&g.path, &g.sid, &g.owner)?;
            return Err(io_err("grant", e));
        }
        crash_point("grant", Point::AfterApply);
        self.done(id, Some(record))
    }

    /// Logs and applies the revoke of `(path, sid)`; the caller holds the path.
    fn revoke_logged(&self, path: &Path, sid: &str, owner: &str) -> Result<(), BrokerError> {
        let id = self.next_id();
        self.append(Record::Revoke {
            id,
            path: path.to_path_buf(),
            sid: sid.to_owned(),
            owner: owner.to_owned(),
        })?;
        lock(&self.state)
            .granted
            .retain(|_, o| !(same_key(&o.path, path) && o.sid == sid));
        crash_point("revoke", Point::AfterAppend);
        self.apply_revoke(path, sid)?;
        crash_point("revoke", Point::AfterApply);
        self.done(id, None)
    }

    /// [`Self::revoke_logged`] with the path taken.
    fn revoke_one(&self, path: &Path, sid: &str, owner: &str) -> Result<(), BrokerError> {
        let _path = self.take_path(path);
        self.revoke_logged(path, sid, owner)
    }

    /// Revokes `owner`'s grants (only those on `path` when given); how many.
    pub fn revoke(&self, owner: &str, path: Option<&Path>) -> Result<usize, BrokerError> {
        let mine: Vec<Grant> = lock(&self.state)
            .granted
            .values()
            .filter(|g| g.owner == owner && path.is_none_or(|p| same_key(&g.path, p)))
            .cloned()
            .collect();
        for g in &mine {
            self.revoke_one(&g.path, &g.sid, &g.owner)?;
        }
        Ok(mine.len())
    }

    /// The live grants, in the order they were made.
    pub fn grants(&self) -> Vec<Grant> {
        lock(&self.state).granted.values().cloned().collect()
    }

    /// The log's line count (tests: it stays bounded).
    #[allow(dead_code)] // tests/acl_broker.rs
    pub fn wal_lines(&self) -> usize {
        lock(&self.state).wal.lines()
    }
}

fn is_live(live: &dyn Fn(&str) -> bool, owner: &str) -> bool {
    owner == STATIC_OWNER || live(owner)
}
