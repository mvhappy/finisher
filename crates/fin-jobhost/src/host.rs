//! The host: group and process tables, the public API, the timers' checks and exit
//! bookkeeping. Spawns, kills and closes run on the `jobhost-spawn` thread
//! ([`crate::commands`]); `send`, `watch` and `close_stdin` only touch a process's queue.

use std::collections::{BTreeSet, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fin_jobs::HostMsg;

use crate::commands::{Cmd, Done, SpawnDone};
use crate::domain::{GroupHandle, KillDomain, LeafHandle};
use crate::exit::ExitReason;
use crate::proc::Proc;
use crate::spawner::SpawnRequest;
use crate::timers::Timers;
use crate::{
    EventSink, GroupId, GroupSpec, HostCaps, HostError, HostEvent, HostOptions, KillReason, ProcId,
    SendError, Watch,
};

/// How often a busy reader's stall is re-checked.
const RECHECK: Duration = Duration::from_millis(50);

pub(crate) struct Group {
    pub spec: GroupSpec,
    /// Made on the spawn thread at the group's first spawn (Decision 11a).
    pub handle: Option<Arc<GroupHandle>>,
    /// Processes whose `Exit` has not gone out yet.
    pub live: BTreeSet<ProcId>,
    /// Spawns in progress on the spawn thread.
    pub pending: u32,
    pub spawned: u32,
    pub closing: bool,
    /// `kill_group` callbacks, with the kill wait's end.
    pub kill_done: Vec<(Instant, Done)>,
    /// Dropping it lets the leader's waiter reap the leader.
    pub reap: Option<mpsc::Sender<()>>,
    /// The last kill sent to its domain (the kill wait's start, Decision 18).
    pub killed_at: Option<Instant>,
    /// Its processes' leaves, released (`close_leaf`) with the group.
    pub leaves: Vec<LeafHandle>,
    /// A Unix leader whose waiter never started: reaped when the group is released.
    #[cfg(unix)]
    pub orphan_leader: Option<u32>,
}

#[derive(Default)]
pub(crate) struct Tables {
    pub groups: HashMap<GroupId, Group>,
    pub procs: HashMap<ProcId, Arc<Proc>>,
    pub next_group: u64,
    pub next_proc: u64,
    pub shutting: bool,
    pub shutdown_done: Vec<Done>,
}

pub(crate) struct Inner {
    pub opts: HostOptions,
    pub domain: Box<dyn KillDomain>,
    pub sink: EventSink,
    pub timers: Timers,
    tables: Mutex<Tables>,
    cmd: mpsc::Sender<Cmd>,
    pub host_pid: u32,
}

/// The job host. Dropping it kills every group it still has.
pub struct Host {
    pub(crate) inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

impl Host {
    /// Starts the host with this OS's kill domain.
    pub fn start(opts: HostOptions, events: EventSink) -> Result<Host, HostError> {
        #[cfg(feature = "test-hooks")]
        if opts.panic_on_start_for_test {
            panic!("test: Host::start panics");
        }
        let domain = crate::os::default_domain(&opts)?;
        Host::start_with(opts, events, domain)
    }

    pub(crate) fn start_with(
        opts: HostOptions,
        sink: EventSink,
        domain: Box<dyn KillDomain>,
    ) -> Result<Host, HostError> {
        let (tx, rx) = mpsc::channel();
        let inner = Arc::new(Inner {
            opts,
            domain,
            sink,
            timers: Timers::start().map_err(HostError::Spawn)?,
            tables: Mutex::default(),
            cmd: tx,
            host_pid: std::process::id(),
        });
        let i = inner.clone();
        let thread = std::thread::Builder::new()
            .name("jobhost-spawn".into())
            .spawn(move || crate::commands::run(&i, rx))
            .map_err(HostError::Spawn)?;
        crate::memory::start_sampler(&inner);
        Ok(Host {
            inner,
            thread: Some(thread),
        })
    }

    pub fn caps(&self) -> HostCaps {
        let i = &self.inner;
        HostCaps {
            kill_tree: i.domain.kill_tree(),
            #[cfg(unix)]
            watchdog: i.domain.watchdog().is_some(),
            #[cfg(not(unix))]
            watchdog: false,
            cgroup_memory: i.domain.kernel_memory(),
            leaf_limits: i.domain.leaf_limits(),
            sandbox: i.opts.spawner.name(),
            sandboxed: i.opts.spawner.sandboxed(),
            profiles: i.opts.spawner.profiles(),
            per_job_grants: i.opts.spawner.per_job_grants(),
        }
    }

    /// Allocates a group; its OS objects are made at its first spawn.
    pub fn open_group(&self, spec: GroupSpec) -> GroupId {
        let mut t = self.inner.tables();
        t.next_group += 1;
        let id = GroupId(t.next_group);
        t.groups.insert(
            id,
            Group {
                spec,
                handle: None,
                live: BTreeSet::new(),
                pending: 0,
                spawned: 0,
                closing: false,
                kill_done: Vec::new(),
                reap: None,
                killed_at: None,
                leaves: Vec::new(),
                #[cfg(unix)]
                orphan_leader: None,
            },
        );
        id
    }

    /// Spawns on the spawn thread; `done` runs there with the process id.
    pub fn spawn(&self, group: GroupId, req: SpawnRequest, done: SpawnDone) {
        self.inner.post(Cmd::Spawn(group, Box::new(req), done));
    }

    /// Enqueues `msg` (never blocks). A full queue kills the process as hung.
    pub fn send(&self, proc: ProcId, msg: &HostMsg) -> Result<(), SendError> {
        let p = self.inner.proc(proc).ok_or(SendError::Gone)?;
        // A `start` names the running job before it is queued (the worker may answer before
        // `send` returns); one that was not queued puts the previous job back.
        // Only while it still names this job: whatever named another since is newer.
        let named = match msg {
            HostMsg::Start(s) => Some((s.job.id.clone(), p.state().job.replace(s.job.id.clone()))),
            _ => None,
        };
        let r = p.writer.send(msg);
        #[cfg(feature = "test-hooks")]
        if r.is_ok()
            && let Some(hook) = &self.inner.opts.after_send_for_test
        {
            hook(proc, msg);
        }
        if r.is_err()
            && let Some((ours, prev)) = named
        {
            let mut st = p.state();
            if st.job.as_deref() == Some(ours.as_str()) {
                st.job = prev;
            }
        }
        if let Err(SendError::QueueFull) = r {
            self.inner.kill_for(proc, ExitReason::Hung);
        }
        r
    }

    /// Sets (or clears) the running job's stall and timeout timers.
    pub fn watch(&self, proc: ProcId, w: Option<Watch>) {
        let Some(p) = self.inner.proc(proc) else {
            return;
        };
        let now = Instant::now();
        let (gen_, stall, timeout) = {
            let mut st = p.state();
            st.watch_gen += 1;
            st.last_progress = now;
            let (s, t) = w
                .as_ref()
                .map_or((None, None), |w| (w.stall_ms, w.timeout_ms));
            st.watch = w;
            (st.watch_gen, s, t)
        };
        if let Some(ms) = stall {
            self.inner
                .arm(now + ms_(ms), proc, move |i, id| i.check_stall(id, gen_));
        }
        if let Some(ms) = timeout {
            self.inner
                .arm(now + ms_(ms), proc, move |i, id| i.check_timeout(id, gen_));
        }
    }

    /// Closes the process's stdin once its queue is written.
    pub fn close_stdin(&self, proc: ProcId) {
        if let Some(p) = self.inner.proc(proc) {
            p.writer.close();
        }
    }

    /// Kills every process of the group; `done` runs once they have all exited or the kill
    /// wait passed. A no-op (`done` at once) on a closed or empty group.
    pub fn kill_group(&self, group: GroupId, reason: KillReason, done: Done) {
        self.inner.post(Cmd::Kill(group, reason, Some(done)));
    }

    /// Closes the group: later spawns fail, live processes are killed, and once it is empty
    /// its domain is released and its leader reaped (Decision 17).
    pub fn close_group(&self, group: GroupId) {
        self.inner.post(Cmd::Close(group));
    }

    /// Closes every stdin, kills what is left after `grace_ms`, and runs `done` once every
    /// process has exited (or the kill wait passed). Later spawns fail.
    pub fn shutdown(&self, grace_ms: u32, done: Done) {
        self.inner.post(Cmd::Shutdown(grace_ms, done));
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.inner.post(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub(crate) fn ms_(ms: u32) -> Duration {
    Duration::from_millis(u64::from(ms))
}

impl Inner {
    pub fn tables(&self) -> MutexGuard<'_, Tables> {
        self.tables.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn post(&self, cmd: Cmd) {
        if let Err(mpsc::SendError(cmd)) = self.cmd.send(cmd) {
            crate::commands::refuse(cmd);
        }
    }

    pub fn log(&self, line: &str) {
        (self.opts.log)(line)
    }

    pub fn proc(&self, id: ProcId) -> Option<Arc<Proc>> {
        self.tables().procs.get(&id).cloned()
    }

    /// Kills `proc`'s group, reporting `proc` with `reason` and the others with its
    /// [`ExitReason::for_others`].
    pub fn kill_for(&self, proc: ProcId, reason: ExitReason) {
        self.post(Cmd::KillFor(proc, reason));
    }

    /// Runs `f` on the timer thread at `when`, if the host and the process are still there.
    pub fn arm(
        self: &Arc<Self>,
        when: Instant,
        proc: ProcId,
        f: impl FnOnce(&Arc<Inner>, ProcId) + Send + 'static,
    ) {
        let w: Weak<Inner> = Arc::downgrade(self);
        self.timers.at(when, move || {
            if let Some(i) = w.upgrade() {
                f(&i, proc);
            }
        });
    }

    /// A live process that is not being killed.
    fn watched(&self, id: ProcId) -> Option<Arc<Proc>> {
        let p = self.proc(id)?;
        let st = p.state();
        if st.exited || st.kill.is_some() {
            return None;
        }
        drop(st);
        Some(p)
    }

    pub fn arm_hello(self: &Arc<Self>, p: &Proc) {
        let at = p.started + ms_(self.opts.timing.hello_ms);
        self.arm(at, p.id, |i, id| {
            if let Some(p) = i.watched(id)
                && !p.state().hello
            {
                i.kill_for(id, ExitReason::NoHello);
            }
        });
    }

    pub fn on_hello(self: &Arc<Self>, p: &Proc) {
        let handle = self
            .tables()
            .groups
            .get(&p.group)
            .and_then(|g| g.handle.clone());
        if let Some(h) = handle {
            self.domain.hello(&h, p.pid);
        }
        let at = Instant::now() + ms_(self.opts.timing.hung_ms);
        self.arm(at, p.id, |i, id| i.check_hung(id));
    }

    /// Hung: silent for `hung_ms` while its reader waits on the pipe (time spent handing
    /// events to the sink does not count, Decision 11a).
    fn check_hung(self: &Arc<Self>, id: ProcId) {
        let Some(p) = self.watched(id) else { return };
        let hung = ms_(self.opts.timing.hung_ms);
        let now = Instant::now();
        let next = match p.out.waiting_since() {
            Some(s) if now.saturating_duration_since(s) >= hung => {
                return self.kill_for(id, ExitReason::Hung);
            }
            Some(s) => s + hung,
            None => now + hung,
        };
        self.arm(next, id, |i, id| i.check_hung(id));
    }

    fn check_stall(self: &Arc<Self>, id: ProcId, gen_: u64) {
        let Some(p) = self.watched(id) else { return };
        let (stall, last) = {
            let st = p.state();
            match (&st.watch, st.watch_gen == gen_) {
                (Some(w), true) => (w.stall_ms.map(ms_), st.last_progress),
                _ => return,
            }
        };
        let Some(stall) = stall else { return };
        let now = Instant::now();
        let next = if now.saturating_duration_since(last) < stall {
            last + stall
        } else if p.out.waiting_since().is_some() {
            return self.kill_for(id, ExitReason::Stalled);
        } else {
            now + RECHECK
        };
        self.arm(next, id, move |i, id| i.check_stall(id, gen_));
    }

    fn check_timeout(&self, id: ProcId, gen_: u64) {
        if let Some(p) = self.watched(id)
            && p.state().watch_gen == gen_
        {
            self.kill_for(id, ExitReason::TimedOut);
        }
    }

    /// After `Exit`: the tables, then [`Inner::settle`] and the shutdown's end.
    pub fn exited(&self, p: &Proc) {
        let all_gone = {
            let mut t = self.tables();
            t.procs.remove(&p.id);
            if let Some(g) = t.groups.get_mut(&p.group) {
                g.live.remove(&p.id);
            }
            t.shutting && t.procs.is_empty()
        };
        self.settle(p.group);
        if all_gone {
            self.post(Cmd::ShutdownFinish);
        }
    }

    /// A group with no live process and no spawn pending: `GroupEmpty`, its kill callbacks,
    /// and its release if it is closing. After an exit and after a failed spawn.
    pub fn settle(&self, gid: GroupId) {
        let (waiters, finalize) = {
            let mut t = self.tables();
            let Some(g) = t.groups.get_mut(&gid) else {
                return;
            };
            if !g.live.is_empty() || g.pending > 0 {
                return;
            }
            let w: Vec<Done> = g.kill_done.drain(..).map(|(_, d)| d).collect();
            (w, g.closing)
        };
        self.sink.send(HostEvent::GroupEmpty { group: gid });
        for w in waiters {
            self.run_done(w);
        }
        if finalize {
            self.post(Cmd::Finalize(gid));
        }
    }

    /// Runs a caller's callback; a panic is logged, never unwinds into host threads.
    pub fn run_done(&self, d: Done) {
        if catch_unwind(AssertUnwindSafe(d)).is_err() {
            self.log("a host callback panicked");
        }
    }
}
