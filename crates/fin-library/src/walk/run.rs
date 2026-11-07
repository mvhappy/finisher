//! The walker's coordinator: dispatches directories to the read threads in walk order, keeps
//! the finished ones in a bounded window until every directory before them is emitted, and
//! emits each one's entries and `DirDone`s ([`super`] has the rules).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::batch::Batcher;
use super::poll::{Plan, Work};
use super::pool::{Held, Outcome, Pool, Task};
use super::{CancelToken, WalkError, WalkOpts, WalkStop, WalkSummary, code};
use crate::placeholder;
use crate::raw::RawRel;
use crate::source::Source;
use crate::types::{DirDone, EntryType, RootState, ScanBatch, VolumeIdentity};

enum State {
    Pending,
    Running,
    Ready(Outcome),
}

struct Slot {
    rel: RawRel,
    work: Work,
    parent: Option<Arc<Held>>,
    state: State,
}

pub(super) struct Walker<'a> {
    plan: Plan,
    pool: Pool,
    batch: Batcher<'a>,
    cancel: &'a CancelToken,
    entry: Duration,
    threads: usize,
    /// At most this many directories between the next to emit and the last dispatched, so
    /// a stuck directory at the front buffers a bounded number of listings.
    window: usize,
    /// In walk order; `slots[0]` (id `base`) is the next to emit, `slots[cursor]` the next to
    /// dispatch.
    slots: VecDeque<Slot>,
    base: u64,
    cursor: usize,
    running: HashMap<u64, Arc<AtomicU64>>,
    abandoned: HashSet<u64>,
    sum: WalkSummary,
}

impl<'a> Walker<'a> {
    pub(super) fn new(
        plan: Plan,
        src: Arc<dyn Source>,
        opts: &WalkOpts,
        sink: &'a mut dyn FnMut(ScanBatch) -> Result<(), WalkStop>,
        cancel: &'a CancelToken,
        identity: VolumeIdentity,
    ) -> Self {
        let threads = opts.threads.max(1);
        Walker {
            plan,
            pool: Pool::new(src, opts.excludes.clone(), threads),
            batch: Batcher::new(
                sink,
                cancel,
                opts.batch_entries,
                opts.batch_bytes,
                opts.timing.flush,
            ),
            cancel,
            entry: opts.timing.entry,
            threads,
            window: (threads * 4).max(16),
            slots: VecDeque::new(),
            base: 0,
            cursor: 0,
            running: HashMap::new(),
            abandoned: HashSet::new(),
            sum: WalkSummary {
                identity,
                listed: 0,
                incomplete: 0,
                timeouts: 0,
                unchanged: 0,
                skipped: 0,
                entries: 0,
                batches: 0,
                stuck: 0,
            },
        }
    }

    pub(super) fn run(mut self, seeds: Vec<(RawRel, Work)>) -> Result<WalkSummary, WalkError> {
        for (rel, work) in seeds {
            self.push(rel, work, None);
        }
        loop {
            if self.cancel.is_cancelled() {
                return Err(WalkError::Cancelled);
            }
            loop {
                self.dispatch()?;
                if !self.emit()? {
                    break;
                }
            }
            if self.slots.is_empty() {
                break;
            }
            let wait = self.wait();
            if let Some((id, out)) = self.pool.recv(wait) {
                self.take(id, out);
            }
            self.expire()?;
            self.batch.tick(Instant::now())?;
        }
        self.batch.flush()?;
        self.sum.batches = self.batch.batches;
        self.sum.stuck = self.abandoned.len() as u64;
        Ok(self.sum)
    }

    fn push(&mut self, rel: RawRel, work: Work, parent: Option<Arc<Held>>) {
        self.slots.push_back(Slot {
            rel,
            work,
            parent,
            state: State::Pending,
        });
    }

    fn dispatch(&mut self) -> Result<(), WalkError> {
        while self.cursor < self.slots.len() && self.cursor < self.window {
            let id = self.base + self.cursor as u64;
            let slot = &mut self.slots[self.cursor];
            match &slot.work {
                Work::Skip => slot.state = State::Ready(Outcome::Skipped),
                Work::Check { children, .. } if children.is_empty() => {
                    slot.state = State::Ready(Outcome::Unchanged {
                        stats: Vec::new(),
                        handle: None,
                    });
                }
                _ => {
                    if !self.pool.has_room() {
                        break;
                    }
                    let progress = Arc::new(AtomicU64::new(0));
                    let task = Task {
                        id,
                        rel: slot.rel.clone(),
                        parent: slot.parent.take(),
                        work: slot.work.clone(),
                        progress: Arc::clone(&progress),
                    };
                    slot.state = State::Running;
                    self.pool.send(task).map_err(WalkError::Thread)?;
                    self.running.insert(id, progress);
                }
            }
            self.cursor += 1;
        }
        Ok(())
    }

    /// Emits every ready directory at the front; whether any was.
    fn emit(&mut self) -> Result<bool, WalkError> {
        let mut any = false;
        while matches!(
            self.slots.front(),
            Some(Slot {
                state: State::Ready(_),
                ..
            })
        ) {
            let slot = self.slots.pop_front().expect("front");
            self.base += 1;
            self.cursor -= 1;
            let State::Ready(out) = slot.state else {
                unreachable!()
            };
            self.process(slot.rel, out)?;
            any = true;
        }
        Ok(any)
    }

    fn wait(&self) -> Duration {
        let mut wait = Duration::from_millis(100);
        for p in self.running.values() {
            let left = self
                .entry
                .saturating_sub(self.pool.since(p.load(Ordering::Relaxed)));
            wait = wait.min(left);
        }
        if let Some(due) = self.batch.due() {
            wait = wait.min(due.saturating_duration_since(Instant::now()));
        }
        wait.max(Duration::from_millis(1))
    }

    fn take(&mut self, id: u64, out: Outcome) {
        if self.running.remove(&id).is_some() {
            self.pool.answered();
            let i = (id - self.base) as usize;
            self.slots[i].state = State::Ready(out);
        } else if self.abandoned.remove(&id) {
            self.pool.returned();
        }
    }

    /// Abandons the reads whose progress clock stopped for `Timing::entry`. As many stuck
    /// threads as the walk has read threads means the volume no longer answers: offline.
    fn expire(&mut self) -> Result<(), WalkError> {
        let overdue: Vec<u64> = self
            .running
            .iter()
            .filter(|(_, p)| self.pool.since(p.load(Ordering::Relaxed)) >= self.entry)
            .map(|(id, _)| *id)
            .collect();
        for id in overdue {
            self.running.remove(&id);
            self.abandoned.insert(id);
            self.pool.abandon();
            self.sum.timeouts += 1;
            let i = (id - self.base) as usize;
            self.slots[i].state = State::Ready(Outcome::TimedOut);
        }
        if self.abandoned.len() >= self.threads {
            return Err(WalkError::off(RootState::Timeout));
        }
        Ok(())
    }

    fn incomplete(&mut self, dir: RawRel, mtime: Option<i64>, code: &str) -> Result<(), WalkError> {
        self.sum.incomplete += 1;
        self.batch.done(DirDone {
            dir,
            mtime_ns: mtime,
            complete: false,
            code: Some(code.to_owned()),
            entries: 0,
        })
    }

    /// A subdirectory found by a listing or a stat: closed with its code when it is not to be
    /// descended, else queued per the mode (into `kids`).
    fn gate(
        &mut self,
        child: RawRel,
        e: (bool, bool, bool, i64),
        kids: &mut Vec<(RawRel, Work)>,
        dones: &mut Vec<(RawRel, i64, &'static str)>,
    ) {
        let (cloud, automount, mount_root, mtime) = e;
        let code = if automount {
            Some(code::AUTOMOUNT)
        } else if cloud {
            Some(code::CLOUD_ONLY)
        } else if self.plan.was_mount(&child) && !mount_root {
            Some(code::ROOT_UNMOUNTED)
        } else {
            None
        };
        match code {
            Some(c) => dones.push((child, mtime, c)),
            None => kids.extend(self.plan.child(child, mtime)),
        }
    }

    fn process(&mut self, rel: RawRel, out: Outcome) -> Result<(), WalkError> {
        let root = rel.is_root();
        let mut kids = Vec::new();
        let mut dones = Vec::new();
        let handle = match out {
            Outcome::Skipped => {
                self.sum.skipped += 1;
                kids = self.plan.after_skip(&rel);
                None
            }
            Outcome::TimedOut if root => return Err(WalkError::off(RootState::Timeout)),
            Outcome::TimedOut => return self.incomplete(rel, None, code::TIMEOUT),
            Outcome::Failed { code } if root => {
                return Err(WalkError::off(RootState::Unreachable { code }));
            }
            Outcome::Failed { code } => return self.incomplete(rel, None, &code),
            Outcome::Listed {
                mtime,
                entries,
                handle,
            } => {
                let n = entries.len();
                if n == 0 && self.plan.known_entries(&rel) > 0 {
                    if root {
                        return Err(WalkError::off(RootState::Empty));
                    }
                    return self.incomplete(rel, mtime, code::DIR_EMPTY_SUSPECT);
                }
                for e in &entries {
                    let descend = e.excluded.is_none() || e.automount;
                    if e.entry_type == EntryType::Dir && descend {
                        let flags = (
                            e.placeholder.is_some(),
                            e.automount,
                            e.mount_root,
                            e.mtime_ns,
                        );
                        self.gate(rel.join(&e.name), flags, &mut kids, &mut dones);
                    }
                }
                self.sum.listed += 1;
                self.sum.entries += n as u64;
                self.batch.entries(entries)?;
                self.batch.done(DirDone {
                    dir: rel.clone(),
                    mtime_ns: mtime,
                    complete: true,
                    code: None,
                    entries: u32::try_from(n).unwrap_or(u32::MAX),
                })?;
                handle
            }
            Outcome::Unchanged { stats, handle } => {
                self.sum.unchanged += 1;
                for e in stats {
                    let cloud = placeholder::from_attrs(e.attrs).is_some();
                    let flags = (cloud, e.automount, e.mount_root, e.mtime_ns);
                    self.gate(rel.join(&e.name), flags, &mut kids, &mut dones);
                }
                handle
            }
        };
        for (dir, mtime, c) in dones {
            self.incomplete(dir, Some(mtime), c)?;
        }
        if kids.is_empty() {
            return Ok(());
        }
        for (c, w) in kids {
            self.push(c, w, handle.clone());
        }
        Ok(())
    }
}
