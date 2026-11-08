//! The read threads (Decisions 9, 18). Directory opens, listings and stats run on up to
//! `threads` worker threads that own an `Arc` of the source, so a thread stuck in a read (a
//! sleeping disk, a dead hard mount) can be **left behind**: the walk answers without it and
//! the thread exits when its call returns (or with the process). Each task carries a progress
//! clock the source advances per step ([`crate::source::Source::read_dir_with`]); the walker
//! abandons a task whose clock has not moved for `Timing::entry`.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::code;
use super::poll::{MountCheck, Work};
use crate::exclude::Excludes;
use crate::listing;
use crate::raw::RawRel;
use crate::source::{DirHandle, RawEntry, RootStat, Source};
use crate::types::{EntryType, ScanEntry, VolumeIdentity};
use crate::volume::{same_volume, unreachable_code};

/// An open directory kept for its children's opens. At most [`HELD_MAX`] are kept at once,
/// counting those of finished listings that wait in the walker's window; past the cap the
/// children are opened by their chain from the root.
///
/// **The descriptor budget** (macOS starts a process with 256): kept directories, plus
/// [`TASK_FDS`] for every task whose read has not returned, **abandoned ones included** (a
/// stuck read still holds its descriptors), at most [`TASKS_MAX`] of them, plus the source's
/// root (one, two while a first stat replaces it, and one transient for its listing):
/// `64 + 64 × 2 + 3 = 195`, below 256 with room for the host's own descriptors.
pub(crate) struct Held {
    pub(crate) dir: DirHandle,
    count: Arc<AtomicUsize>,
}

pub(crate) const HELD_MAX: usize = 64;
/// Descriptors one task may hold at once: its directory and a fresh one for the listing's
/// own offset (`reopen_dir`); a chain open from the root holds the previous component and
/// the next one.
pub(crate) const TASK_FDS: usize = 2;
/// Tasks whose reads have not returned, running or abandoned; the walker's thread count is
/// capped to it.
pub(crate) const TASKS_MAX: usize = 64;
/// The root's descriptors: the source's own, a first stat's replacing it, and one listing.
const ROOT_FDS: usize = 3;
const _: () = assert!(HELD_MAX + TASK_FDS * TASKS_MAX + ROOT_FDS < 256);

impl Held {
    /// Keeps `dir` when fewer than [`HELD_MAX`] are kept.
    pub(crate) fn keep(dir: DirHandle, count: &Arc<AtomicUsize>) -> Option<Arc<Held>> {
        let n = count.fetch_add(1, Ordering::Relaxed);
        if n >= HELD_MAX {
            count.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        Some(Arc::new(Held {
            dir,
            count: Arc::clone(count),
        }))
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A task's progress clock (nanoseconds since the pool started, [`Pool::now`]) and whether
/// its worker has finished it. A finished task is never overdue: its answer may sit in the
/// channel while the coordinator is busy in the sink.
#[derive(Default)]
pub(crate) struct Progress {
    stamp: AtomicU64,
    finished: AtomicBool,
}

impl Progress {
    fn tick(&self, now: u64) {
        self.stamp.store(now, Ordering::Relaxed);
    }

    pub(crate) fn stamp(&self) -> u64 {
        self.stamp.load(Ordering::Relaxed)
    }

    pub(crate) fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

/// One directory's I/O.
pub(crate) struct Task {
    pub(crate) id: u64,
    pub(crate) rel: RawRel,
    pub(crate) parent: Option<Arc<Held>>,
    pub(crate) work: Work,
    pub(crate) mount: Option<MountCheck>,
    pub(crate) progress: Arc<Progress>,
}

/// What a task found.
pub(crate) enum Outcome {
    /// A listing: its entries (sorted, classified, [`listing::entries`]) and the directory's
    /// mtime read before it; `partial` when some entry could not be read
    /// ([`crate::source::Listing`]).
    Listed {
        mtime: Option<i64>,
        entries: Vec<ScanEntry>,
        handle: Option<Arc<Held>>,
        partial: bool,
        /// A nested mount root's volume ([`MountCheck`]).
        volume: Option<Box<VolumeIdentity>>,
    },
    /// Poll, unchanged: each known subdirectory's stat.
    Unchanged {
        stats: Vec<RawEntry>,
        handle: Option<Arc<Held>>,
    },
    /// Resumed full scan: not listed.
    Skipped,
    Failed {
        code: String,
    },
    TimedOut,
}

struct Shared {
    src: Arc<dyn Source>,
    excludes: Excludes,
    tasks: Mutex<Receiver<Task>>,
    done: Sender<(u64, Outcome)>,
    start: Instant,
    held: Arc<AtomicUsize>,
    /// Tasks handed out whose `run` has not returned (stuck ones included); the worker
    /// releases its task's slot when the read returns, whether or not it was abandoned.
    inflight: AtomicUsize,
}

/// The worker threads and their channels.
pub(crate) struct Pool {
    shared: Arc<Shared>,
    tasks: Sender<Task>,
    done: Receiver<(u64, Outcome)>,
    max: usize,
    /// Workers not stuck in an abandoned task.
    live: usize,
    /// Tasks handed out and not yet answered or abandoned.
    busy: usize,
}

impl Pool {
    pub(crate) fn new(src: Arc<dyn Source>, excludes: Excludes, threads: usize) -> Pool {
        let (tasks, rx) = mpsc::channel();
        let (done, done_rx) = mpsc::channel();
        Pool {
            shared: Arc::new(Shared {
                src,
                excludes,
                tasks: Mutex::new(rx),
                done,
                start: Instant::now(),
                held: Arc::new(AtomicUsize::new(0)),
                inflight: AtomicUsize::new(0),
            }),
            tasks,
            done: done_rx,
            max: threads.clamp(1, TASKS_MAX),
            live: 0,
            busy: 0,
        }
    }

    /// The walk's clock (progress stamps are nanoseconds since the pool started).
    pub(crate) fn now(&self) -> u64 {
        stamp(self.shared.start)
    }

    pub(crate) fn since(&self, stamp: u64) -> Duration {
        Duration::from_nanos(self.now().saturating_sub(stamp))
    }

    /// A worker is free and the descriptor budget has a task's room ([`TASKS_MAX`]).
    pub(crate) fn has_room(&self) -> bool {
        self.busy < self.max && self.shared.inflight.load(Ordering::Acquire) < TASKS_MAX
    }

    /// Hands a task to an idle worker, starting one when none is idle.
    pub(crate) fn send(&mut self, t: Task) -> io::Result<()> {
        if self.busy >= self.live {
            let shared = Arc::clone(&self.shared);
            std::thread::Builder::new()
                .name("fin-library-walk".into())
                .stack_size(512 * 1024)
                .spawn(move || worker(&shared))?;
            self.live += 1;
        }
        t.progress.tick(self.now());
        self.busy += 1;
        self.shared.inflight.fetch_add(1, Ordering::AcqRel);
        self.tasks
            .send(t)
            .map_err(|_| io::Error::other("walk workers are gone"))
    }

    pub(crate) fn recv(&self, wait: Duration) -> Option<(u64, Outcome)> {
        self.done.recv_timeout(wait).ok()
    }

    /// An answer already waiting, without blocking.
    pub(crate) fn try_recv(&self) -> Option<(u64, Outcome)> {
        self.done.try_recv().ok()
    }

    /// An answer to a live task arrived.
    pub(crate) fn answered(&mut self) {
        self.busy -= 1;
    }

    /// A live task is abandoned: its worker counts as stuck until it answers.
    pub(crate) fn abandon(&mut self) {
        self.busy -= 1;
        self.live -= 1;
    }

    /// An abandoned task answered after all: its worker is idle again.
    pub(crate) fn returned(&mut self) {
        self.live += 1;
    }
}

fn stamp(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn worker(shared: &Shared) {
    loop {
        let task = {
            let rx = shared.tasks.lock().unwrap_or_else(|p| p.into_inner());
            match rx.recv() {
                Ok(t) => t,
                Err(_) => return,
            }
        };
        let out = run(shared, &task);
        // The read returned and its descriptors are closed or kept (`Held`).
        shared.inflight.fetch_sub(1, Ordering::AcqRel);
        task.progress.finished.store(true, Ordering::Release);
        if shared.done.send((task.id, out)).is_err() {
            return;
        }
    }
}

fn failed(e: &io::Error) -> Outcome {
    Outcome::Failed {
        code: unreachable_code(e).to_owned(),
    }
}

fn run(sh: &Shared, t: &Task) -> Outcome {
    let tick = || t.progress.tick(stamp(sh.start));
    let src = &*sh.src;
    let opened = match (&t.parent, t.rel.name()) {
        (Some(p), Some(name)) => src.open_child(&p.dir, name),
        _ => src.open_dir(&t.rel),
    };
    let dir = match opened {
        Ok(d) => d,
        Err(e) => return failed(&e),
    };
    tick();
    let volume = match &t.mount {
        None => None,
        Some(m) => match src.volume(&dir) {
            Err(e) => return failed(&e),
            Ok(now) if m.was_mount && !now.mount_root => return code_of(code::ROOT_UNMOUNTED),
            Ok(now) if m.stored.as_ref().is_some_and(|s| !same_volume(s, &now)) => {
                return code_of(code::ROOT_VOLUME_CHANGED);
            }
            Ok(now) => {
                tick();
                Some(Box::new(now))
            }
        },
    };
    let list = |dir, mtime| list(sh, t, dir, mtime, volume.clone(), &tick);
    let (mtime, children) = match &t.work {
        Work::List { mtime } => return list(dir, *mtime),
        Work::Check { mtime, children } => (*mtime, children),
        Work::Probe {
            known_mtime,
            children,
        } => {
            let now = match src.dir_mtime(&dir) {
                Ok(m) => m,
                Err(e) => return failed(&e),
            };
            tick();
            if now.is_none() || now != *known_mtime {
                return list(dir, now);
            }
            (now, children)
        }
        Work::Skip => return Outcome::Skipped,
    };
    let mut stats = Vec::with_capacity(children.len());
    for name in children {
        match src.stat_child(&dir, name) {
            Ok(Some(e)) if e.entry_type == EntryType::Dir => stats.push(e),
            // Gone, no longer a directory, or unreadable: the listing tells.
            _ => return list(dir, mtime),
        }
        tick();
    }
    Outcome::Unchanged {
        handle: if stats.is_empty() {
            None
        } else {
            Held::keep(dir, &sh.held)
        },
        stats,
    }
}

fn code_of(code: &str) -> Outcome {
    Outcome::Failed {
        code: code.to_owned(),
    }
}

fn list(
    sh: &Shared,
    t: &Task,
    dir: DirHandle,
    mtime: Option<i64>,
    volume: Option<Box<VolumeIdentity>>,
    tick: &dyn Fn(),
) -> Outcome {
    let mtime = match mtime {
        Some(m) => Some(m),
        None => match sh.src.dir_mtime(&dir) {
            Ok(m) => m,
            Err(e) => return failed(&e),
        },
    };
    tick();
    match sh.src.read_dir_with(&dir, tick) {
        Ok(raw) => {
            let partial = raw.unreadable.is_some();
            let entries = listing::entries(&t.rel, raw.entries, &sh.excludes);
            let subdirs = entries
                .iter()
                .any(|e| e.entry_type == EntryType::Dir && e.excluded.is_none());
            Outcome::Listed {
                mtime,
                entries,
                handle: if subdirs {
                    Held::keep(dir, &sh.held)
                } else {
                    None
                },
                partial,
                volume,
            }
        }
        Err(e) => failed(&e),
    }
}

/// The root's first stat (Decision 18), on its own thread: `None` past `timeout` or on a
/// cancel (the thread is left to finish).
pub(crate) fn first_stat(
    src: &Arc<dyn Source>,
    timeout: Duration,
    cancel: &super::CancelToken,
) -> Option<io::Result<RootStat>> {
    let (tx, rx) = mpsc::channel();
    let src = Arc::clone(src);
    let spawned = std::thread::Builder::new()
        .name("fin-library-first-stat".into())
        .spawn(move || {
            let _ = tx.send(src.stat_root());
        });
    if let Err(e) = spawned {
        return Some(Err(e));
    }
    let until = Instant::now() + timeout;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || cancel.is_cancelled() {
            return None;
        }
        if let Ok(r) = rx.recv_timeout(left.min(Duration::from_millis(100))) {
            return Some(r);
        }
    }
}
