//! The ACL broker (plan 7C Decision 18, Task 10): one per server, the only code that changes a
//! DACL for a sandboxed process. It grants and revokes `{path, sid, mask, inherit, owner}` as
//! allow-only ACEs (`fin_sandbox::windows::acl` on Windows; [`apply`]), each operation first
//! appended to `<server-data>/run/acl-broker.wal` and fsynced, then applied, then marked done
//! ([`wal`]). So a crash between grant and revoke never leaves a stale grant:
//!
//! - **Replay at open** ([`Broker::open`]): an incomplete revoke is completed; an incomplete
//!   grant (its caller never saw it succeed) is completed when its owner is live, else revoked.
//!   A revoke that cannot be done stays pending (the sweep retries it) and the broker opens.
//!   The log is then rewritten with only what is granted, so a torn last line goes too.
//! - **Sweep** ([`sweep`]): every grant whose owner is not live is revoked, and a revoke that
//!   failed earlier is retried; one that fails again stays pending and the sweep goes on.
//!   `finisherd` sweeps before any listener starts, with no owner live: no session, job or call
//!   outlives the server that started it. The owner [`STATIC_OWNER`] (the toolchain, a
//!   project's folder, a sign-in store: shared by every launch of a kind) is never revoked, and
//!   its records are **not carried across a restart**: the next launch grants them again
//!   (granting merges, so this is idempotent), which heals a file replaced by rename since.
//! - **A pending revoke never outlives a newer grant of its pair**: a grant first completes
//!   any pending revoke of its `(path, sid)`, and completing a revoke whose pair was granted
//!   again later (the replay) re-applies that grant.
//! - **Protected roots** are refused before anything is written ([`protect`]); every path is
//!   resolved once on entry and that form is checked, locked, recorded and applied.
//! - **One path at a time**: a DACL change is a read-modify-write, so two grants on one
//!   directory at once would lose one (the S8 lost-grant race). An operation waits while
//!   another runs on the same path, an ancestor or a descendant (inheritance rewrites the
//!   descendants' DACLs too).
//! - **One broker per server data**: `run/acl-broker.lock` is held exclusively while it is open.
//! - **Bounded**: the log is rewritten (temp file, fsync, rename) once it holds more than
//!   twice the live grants plus the operations not done plus [`Options::compact_slack`] lines.
//!   A rewrite that fails is a warning: the operation that triggered it stands.
//!
//! One owner per `(path, sid)`: a second owner's grant of the same pair is refused, since a
//! revoke removes every explicit ACE of that SID on that path. The same owner's repeated grant
//! adds its mask to the record and is skipped when the record already covers it (a static one
//! only when the DACL still holds it, [`Apply::holds`]).
//!
//! Napi-free (`crate::sandbox_acl` is the binding), so `tests/acl_broker.rs` includes it.

#![allow(clippy::disallowed_methods)] // the log and the protected-root checks are server data

pub mod apply;
pub mod error;
pub mod protect;
pub mod sweep;
pub mod wal;

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

pub use apply::Apply;
pub use error::BrokerError;
use error::io_err;
use wal::{Record, Wal};

/// The owner of the grants every launch of a kind shares; never revoked by a sweep.
pub const STATIC_OWNER: &str = "static";
/// The log, under `<server-data>/run/`.
pub const WAL_FILE: &str = "acl-broker.wal";
/// Held exclusively by the open broker, under `<server-data>/run/`.
pub const LOCK_FILE: &str = "acl-broker.lock";
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

/// A named point where a `test-hooks` build may abort (`FIN_TEST_ACL_CRASH=<op>:<point>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    AfterAppend,
    AfterApply,
    /// The compacted log is written to its temp file, not yet renamed over the log.
    BeforeRename,
    AfterRename,
}

/// Aborts the process when `FIN_TEST_ACL_CRASH` names `op` and `point` (test-hooks only).
#[cfg(feature = "test-hooks")]
fn crash_point(op: &str, point: Point) {
    let name = match point {
        Point::AfterAppend => "afterAppend",
        Point::AfterApply => "afterApply",
        Point::BeforeRename => "beforeRename",
        Point::AfterRename => "afterRename",
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
    /// What went wrong without failing an operation (a failed rewrite of the log, a revoke
    /// left pending at open), oldest first.
    warnings: Vec<String>,
}

/// The broker of one server's data directory.
pub struct Broker<A: Apply> {
    rules: protect::Rules,
    apply: A,
    opts: Options,
    state: Mutex<State>,
    busy: Mutex<Vec<PathBuf>>,
    freed: Condvar,
    _lock: File,
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

fn pair_of(r: &Record) -> Option<(&Path, &str)> {
    match r {
        Record::Grant { path, sid, .. } | Record::Revoke { path, sid, .. } => {
            Some((path.as_path(), sid.as_str()))
        }
        Record::Done { .. } => None,
    }
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

/// Takes the exclusive lock of `run/` (one broker per server data, across processes).
fn take_lock(run: &Path) -> Result<File, BrokerError> {
    let path = run.join(LOCK_FILE);
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| io_err("lock file", e))?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => Err(BrokerError::Io(format!(
            "another ACL broker holds {}",
            path.display()
        ))),
        Err(std::fs::TryLockError::Error(e)) => Err(io_err("lock", e)),
    }
}

impl<A: Apply> Broker<A> {
    /// Opens the broker of `server_data`: takes its lock, reads the log, completes or rolls
    /// back what a crash left (an incomplete grant is completed only when `live(owner)`; a
    /// revoke that fails stays pending, see [`Broker::warnings`]), forgets the static records,
    /// and rewrites the log.
    pub fn open(
        server_data: &Path,
        apply: A,
        opts: Options,
        live: &dyn Fn(&str) -> bool,
    ) -> Result<Broker<A>, BrokerError> {
        let run = server_data.join("run");
        std::fs::create_dir_all(&run).map_err(|e| io_err("run directory", e))?;
        let lock_file = take_lock(&run)?;
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
                warnings: Vec::new(),
            }),
            busy: Mutex::new(Vec::new()),
            freed: Condvar::new(),
            _lock: lock_file,
        };
        for e in broker.finish(replay.pending, live) {
            broker.warn(format!("left pending at open: {e}"));
        }
        let mut s = lock(&broker.state);
        s.granted.retain(|_, g| g.owner != STATIC_OWNER);
        if let Err(e) = Self::compact(&mut s) {
            s.warnings.push(e.to_string());
        }
        drop(s);
        Ok(broker)
    }

    /// What went wrong without failing an operation since the broker opened (each also
    /// written to stderr when it happened).
    #[allow(dead_code)] // tests/acl_broker.rs
    pub fn warnings(&self) -> Vec<String> {
        lock(&self.state).warnings.clone()
    }

    fn warn(&self, w: String) {
        eprintln!("finisherd: ACL broker: {w}");
        lock(&self.state).warnings.push(w);
    }

    /// Completes the operations a crash left: revokes always, grants of a live owner. What
    /// fails stays pending; the failures are answered.
    fn finish(&self, pending: Vec<Record>, live: &dyn Fn(&str) -> bool) -> Vec<BrokerError> {
        let mut failed = Vec::new();
        for r in pending {
            let id = r.id();
            let outcome = match r {
                Record::Grant {
                    path,
                    sid,
                    mask,
                    inherit,
                    owner,
                    ..
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
                        self.done(id, Some(g))
                    } else {
                        // Never marked done: the revoke logged next drops it at replay.
                        lock(&self.state).pending.remove(&id);
                        self.revoke_one(&g.path, &g.sid, &g.owner)
                    }
                }
                Record::Revoke {
                    ref path, ref sid, ..
                } => {
                    lock(&self.state).pending.insert(id, r.clone());
                    self.complete_revoke(id, path, sid)
                }
                Record::Done { .. } => Ok(()),
            };
            if let Err(e) = outcome {
                failed.push(e);
            }
        }
        failed
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
    /// Rewrites the log when it has grown past its bound (a failure there is a warning).
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
        if s.wal.lines() > limit
            && let Err(e) = Self::compact(&mut s)
        {
            eprintln!("finisherd: ACL broker: {e}");
            s.warnings.push(e.to_string());
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
        let rewrite = |e| io_err("log rewrite", e);
        s.wal.write_temp(&records).map_err(rewrite)?;
        crash_point("compact", Point::BeforeRename);
        s.wal.replace_with_temp(records.len()).map_err(rewrite)?;
        crash_point("compact", Point::AfterRename);
        Ok(())
    }

    /// A revoke of a path that is gone has nothing left to do.
    fn apply_revoke(&self, path: &Path, sid: &str) -> Result<(), BrokerError> {
        match self.apply.revoke(path, sid) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err("revoke", e)),
        }
    }

    /// Completes the pending revoke `id` of `(path, sid)`; the caller holds the path. When the
    /// pair was granted again after it (a replayed log), that grant is applied again, so the
    /// record and the DACL agree.
    fn complete_revoke(&self, id: u64, path: &Path, sid: &str) -> Result<(), BrokerError> {
        self.apply_revoke(path, sid)?;
        let again = lock(&self.state)
            .granted
            .values()
            .find(|g| same_key(&g.path, path) && g.sid == sid)
            .cloned();
        self.done(id, None)?;
        if let Some(g) = again
            && let Err(e) = self.apply.grant(&g.path, &g.sid, g.mask, g.inherit)
        {
            self.revoke_logged(&g.path, &g.sid, &g.owner)?;
            return Err(io_err("grant again after a revoke", e));
        }
        Ok(())
    }

    /// Completes every pending revoke of `(path, sid)`; the caller holds the path.
    fn settle(&self, path: &Path, sid: &str) -> Result<(), BrokerError> {
        let ids: Vec<u64> = lock(&self.state)
            .pending
            .values()
            .filter(|r| matches!(r, Record::Revoke { .. }))
            .filter(|r| pair_of(r).is_some_and(|(p, s)| same_key(p, path) && s == sid))
            .map(Record::id)
            .collect();
        for id in ids {
            self.complete_revoke(id, path, sid)?;
        }
        Ok(())
    }

    /// Grants `g`: resolved and refused before the log when protected; logged, applied,
    /// marked done. A pending revoke of its pair is completed first.
    pub fn grant(&self, g: &Grant) -> Result<(), BrokerError> {
        let g = &self.rules.resolve(g)?;
        let _path = self.take_path(&g.path);
        self.settle(&g.path, &g.sid)?;
        let mut record = g.clone();
        let held = lock(&self.state)
            .granted
            .values()
            .find(|o| same_key(&o.path, &g.path) && o.sid == g.sid)
            .cloned();
        if let Some(o) = held {
            if o.owner != g.owner {
                return Err(BrokerError::Owned {
                    path: g.path.clone(),
                    owner: o.owner,
                });
            }
            let covered = o.mask & g.mask == g.mask && (o.inherit || !g.inherit);
            // A static grant lives as long as its file: one replaced since loses its ACE.
            let stands = g.owner != STATIC_OWNER
                || self
                    .apply
                    .holds(&g.path, &g.sid, g.mask, g.inherit)
                    .unwrap_or(false);
            if covered && stands {
                return Ok(());
            }
            record.mask |= o.mask;
            record.inherit |= o.inherit;
        }
        let id = self.next_id();
        self.append(Record::grant(id, &record))?;
        crash_point("grant", Point::AfterAppend);
        if let Err(e) = self
            .apply
            .grant(&record.path, &record.sid, record.mask, record.inherit)
        {
            // Rolled back, never marked done (the logged revoke drops it at replay): the DACL
            // may hold part of it (inheritance stops midway).
            lock(&self.state).pending.remove(&id);
            self.revoke_logged(&g.path, &g.sid, &g.owner)?;
            return Err(io_err("grant", e));
        }
        crash_point("grant", Point::AfterApply);
        self.done(id, Some(record))
    }

    /// Logs and applies the revoke of `(path, sid)`; the caller holds the path. A failure to
    /// apply leaves it pending (the sweep retries it).
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

    /// Revokes `owner`'s grants (only those on `path` when given); how many. Every grant is
    /// tried: one that fails stays pending for the sweep, and the first failure is answered
    /// after the others are done.
    pub fn revoke(&self, owner: &str, path: Option<&Path>) -> Result<usize, BrokerError> {
        let path = path.map(protect::resolved);
        let mine: Vec<Grant> = lock(&self.state)
            .granted
            .values()
            .filter(|g| g.owner == owner && path.as_ref().is_none_or(|p| same_key(&g.path, p)))
            .cloned()
            .collect();
        let mut first = None;
        for g in &mine {
            if let Err(e) = self.revoke_one(&g.path, &g.sid, &g.owner) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(mine.len()), Err)
    }

    /// The live grants, in the order they were made.
    pub fn grants(&self) -> Vec<Grant> {
        lock(&self.state).granted.values().cloned().collect()
    }

    /// The operations not done (tests: a failed revoke stays pending).
    #[allow(dead_code)] // tests/acl_broker.rs
    pub fn pending(&self) -> usize {
        lock(&self.state).pending.len()
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
