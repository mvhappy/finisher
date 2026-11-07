//! The `jobhost-spawn` thread (Decision 11): every spawn, kill, close and the shutdown run
//! here, in order. It lives as long as the host, so on Linux `PR_SET_PDEATHSIG` (which follows
//! the thread that forked) fires only when the host itself goes. `done` callbacks run here
//! and must be quick.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Weak};
use std::time::Instant;

use fin_jobs::sandbox;

use crate::domain::GroupHandle;
use crate::exit::ExitReason;
use crate::framing::Expect;
use crate::host::{Inner, ms_};
use crate::os::{self, SpawnCtx};
use crate::proc::{self, Proc};
use crate::spawner::{self, Program, Role, SpawnRequest};
use crate::{GroupId, GroupSpec, HostError, KillReason, ProcId};

/// A callback with no result.
pub type Done = Box<dyn FnOnce() + Send>;
/// A spawn's callback.
pub type SpawnDone = Box<dyn FnOnce(Result<ProcId, HostError>) + Send>;

pub(crate) enum Cmd {
    Spawn(GroupId, Box<SpawnRequest>, SpawnDone),
    Kill(GroupId, KillReason, Option<Done>),
    KillFor(ProcId, ExitReason),
    KillWaitOver(GroupId),
    CloseLeaves(GroupId),
    Close(GroupId),
    Finalize(GroupId),
    Shutdown(u32, Done),
    ShutdownKill,
    ShutdownFinish,
    Stop,
}

/// Answers a command the thread will never run (the host is gone).
pub(crate) fn refuse(cmd: Cmd) {
    match cmd {
        Cmd::Spawn(_, _, done) => done(Err(HostError::ShutDown)),
        Cmd::Kill(_, _, Some(done)) | Cmd::Shutdown(_, done) => done(),
        _ => {}
    }
}

pub(crate) fn run(inner: &Arc<Inner>, rx: Receiver<Cmd>) {
    while let Ok(cmd) = rx.recv() {
        let stop = matches!(cmd, Cmd::Stop);
        // A panicking callback must not take the spawn thread (and every later spawn) with it.
        if catch_unwind(AssertUnwindSafe(|| one(inner, cmd))).is_err() {
            inner.log("a spawn-thread command panicked");
        }
        if stop {
            break;
        }
    }
    while let Ok(cmd) = rx.try_recv() {
        refuse(cmd);
    }
}

fn one(inner: &Arc<Inner>, cmd: Cmd) {
    match cmd {
        Cmd::Spawn(g, req, done) => done(spawn(inner, g, *req)),
        Cmd::Kill(g, reason, done) => kill(inner, g, reason, done),
        Cmd::KillFor(p, reason) => kill_for(inner, p, reason),
        Cmd::KillWaitOver(g) => kill_wait_over(inner, g),
        Cmd::CloseLeaves(g) => close_leaves(inner, g),
        Cmd::Close(g) => close(inner, g),
        Cmd::Finalize(g) => finalize(inner, g),
        Cmd::Shutdown(grace, done) => shutdown(inner, grace, done),
        Cmd::ShutdownKill => shutdown_kill(inner),
        Cmd::ShutdownFinish => shutdown_finish(inner),
        Cmd::Stop => stop(inner),
    }
}

fn spawn(inner: &Arc<Inner>, gid: GroupId, req: SpawnRequest) -> Result<ProcId, HostError> {
    let (handle, spec) = {
        let mut t = inner.tables();
        if t.shutting {
            return Err(HostError::ShutDown);
        }
        let next = t.next_group;
        let Some(g) = t.groups.get_mut(&gid) else {
            return Err(if gid.0 <= next {
                HostError::GroupClosed(gid)
            } else {
                HostError::UnknownGroup(gid)
            });
        };
        if g.closing {
            return Err(HostError::GroupClosed(gid));
        }
        g.pending += 1;
        (g.handle.clone(), g.spec.clone())
    };
    let r = spawn_in(inner, gid, handle, &spec, req);
    if r.is_err() {
        let fin = {
            let mut t = inner.tables();
            t.groups.get_mut(&gid).is_some_and(|g| {
                g.pending -= 1;
                g.closing && g.live.is_empty() && g.pending == 0
            })
        };
        if fin {
            finalize(inner, gid);
        }
    }
    r
}

fn spawn_in(
    inner: &Arc<Inner>,
    gid: GroupId,
    handle: Option<Arc<GroupHandle>>,
    spec: &GroupSpec,
    req: SpawnRequest,
) -> Result<ProcId, HostError> {
    let handle = match handle {
        Some(h) => h,
        None => {
            let h = Arc::new(
                inner
                    .domain
                    .open_group(gid, spec)
                    .map_err(HostError::Spawn)?,
            );
            if let Some(g) = inner.tables().groups.get_mut(&gid) {
                g.handle = Some(h.clone());
            }
            h
        }
    };
    let mut cmd = spawner::command(&inner.opts.binary_dir, &req).map_err(HostError::Program)?;
    inner
        .opts
        .spawner
        .prepare(&req, &mut cmd)
        .map_err(|e| HostError::Spawner(e.0))?;
    let self_sandbox = match &cmd.self_sandbox {
        Some(sb) => {
            let json = serde_json::to_string(sb).map_err(|e| HostError::Spawner(e.to_string()))?;
            cmd.env.push((sandbox::ENV.into(), json.into()));
            Some(sb.profile.clone())
        }
        None => None,
    };
    let leaf = inner
        .domain
        .open_leaf(&handle, &cmd.job_limits)
        .map_err(HostError::Spawn)?;
    #[cfg(unix)]
    let leader = handle.pgid() == 0;
    #[cfg(not(unix))]
    let leader = false;
    let ctx = SpawnCtx {
        domain: &*inner.domain,
        group: &handle,
        leaf: &leaf,
        qos: req.qos,
        host_pid: inner.host_pid,
        #[cfg(feature = "test-hooks")]
        resume_delay: inner.opts.resume_delay_for_test,
    };
    let spawned = os::spawn(cmd, &ctx).map_err(HostError::Spawn)?;
    let expect = Expect {
        bundled: matches!(req.program, Program::Bundled(_)),
        helper: matches!(req.role, Role::Helper { .. }),
        self_sandbox: self_sandbox.clone(),
        host_minor: inner.opts.host_minor,
    };
    let (p, reap) = {
        let mut t = inner.tables();
        t.next_proc += 1;
        let id = ProcId(t.next_proc);
        let p = Arc::new(Proc::new(
            id,
            gid,
            spawned.pid,
            leader,
            self_sandbox,
            inner.sink.clone(),
            leaf,
        ));
        t.procs.insert(id, p.clone());
        let g = t
            .groups
            .get_mut(&gid)
            .expect("a group with a pending spawn stays");
        g.pending -= 1;
        g.spawned += 1;
        g.live.insert(id);
        let reap = leader.then(|| {
            let (tx, rx) = mpsc::channel();
            g.reap = Some(tx);
            rx
        });
        (p, reap)
    };
    let w = Arc::downgrade(inner);
    let drain = ms_(inner.opts.timing.exit_drain_ms);
    let os::Spawned {
        stdin,
        stdout,
        stderr,
        os: os_proc,
        ..
    } = spawned;
    let started = [
        thread("jobhost-wait", {
            let (p, w) = (p.clone(), w.clone());
            move || proc::wait_exit(p, w, os_proc, drain, reap)
        }),
        thread("jobhost-out", {
            let (p, w) = (p.clone(), w.clone());
            move || proc::read_stdout(p, w, stdout, expect)
        }),
        thread("jobhost-err", {
            let p = p.clone();
            move || proc::read_stderr(p, stderr)
        }),
        thread("jobhost-in", {
            let p = p.clone();
            move || p.writer.run(stdin)
        }),
    ];
    if started.iter().any(|ok| !ok) {
        inner.log(&format!("{}: a thread did not start; killing it", p.id));
        kill_for(inner, p.id, ExitReason::SpawnFailed("thread".into()));
    }
    inner.arm_hello(&p);
    Ok(p.id)
}

fn thread(name: &str, f: impl FnOnce() + Send + 'static) -> bool {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .is_ok()
}

fn kill_domain(inner: &Inner, handle: Option<Arc<GroupHandle>>) {
    if let Some(h) = handle
        && let Err(e) = inner.domain.kill(&h)
    {
        inner.log(&format!("kill of {} failed: {e}", h.id));
    }
}

fn kill(inner: &Arc<Inner>, gid: GroupId, reason: KillReason, done: Option<Done>) {
    let handle = {
        let mut t = inner.tables();
        let live: Vec<Arc<Proc>> = match t.groups.get(&gid) {
            Some(g) => g
                .live
                .iter()
                .filter_map(|id| t.procs.get(id).cloned())
                .collect(),
            None => Vec::new(),
        };
        if live.is_empty() {
            drop(t);
            if let Some(d) = done {
                d();
            }
            return;
        }
        for p in &live {
            p.mark_kill(ExitReason::Killed(reason));
        }
        let g = t.groups.get_mut(&gid).expect("checked above");
        if let Some(d) = done {
            let wait = ms_(inner.opts.timing.kill_wait_ms);
            g.kill_done.push((Instant::now() + wait, d));
            let w: Weak<Inner> = Arc::downgrade(inner);
            inner.timers.at(Instant::now() + wait, move || {
                if let Some(i) = w.upgrade() {
                    i.post(Cmd::KillWaitOver(gid));
                }
            });
        }
        g.handle.clone()
    };
    kill_domain(inner, handle);
}

fn kill_for(inner: &Inner, pid: ProcId, reason: ExitReason) {
    let handle = {
        let t = inner.tables();
        let Some(p) = t.procs.get(&pid) else { return };
        if !p.mark_kill(reason.clone()) {
            return;
        }
        let Some(g) = t.groups.get(&p.group) else {
            return;
        };
        for other in g.live.iter().filter(|&&o| o != pid) {
            if let Some(o) = t.procs.get(other) {
                o.mark_kill(reason.for_others());
            }
        }
        g.handle.clone()
    };
    kill_domain(inner, handle);
}

/// The kill wait passed: run the due callbacks. A process still alive is abandoned (Task 4
/// reports it).
fn kill_wait_over(inner: &Inner, gid: GroupId) {
    let (due, left) = {
        let mut t = inner.tables();
        let Some(g) = t.groups.get_mut(&gid) else {
            return;
        };
        let now = Instant::now();
        let (due, keep): (Vec<_>, Vec<_>) = g.kill_done.drain(..).partition(|(at, _)| *at <= now);
        g.kill_done = keep;
        (due, g.live.len())
    };
    if !due.is_empty() && left > 0 {
        inner.log(&format!(
            "{gid}: {left} process(es) still alive after the kill wait"
        ));
    }
    for (_, d) in due {
        d();
    }
}

fn close(inner: &Arc<Inner>, gid: GroupId) {
    let empty = {
        let mut t = inner.tables();
        let Some(g) = t.groups.get_mut(&gid) else {
            return;
        };
        g.closing = true;
        g.live.is_empty() && g.pending == 0
    };
    if empty {
        finalize(inner, gid);
    } else {
        kill(inner, gid, KillReason::JobEnded, None);
    }
}

/// Releases the leaves of the group's exited processes (`KillDomain::close_leaf`).
fn close_leaves(inner: &Inner, gid: GroupId) {
    let (handle, leaves) = {
        let mut t = inner.tables();
        let Some(g) = t.groups.get_mut(&gid) else {
            return;
        };
        (g.handle.clone(), std::mem::take(&mut g.leaves))
    };
    if let Some(h) = handle {
        for leaf in leaves {
            inner.domain.close_leaf(&h, leaf);
        }
    }
}

/// A closing group is empty: release its leaves and its domain, then let the leader be reaped.
fn finalize(inner: &Inner, gid: GroupId) {
    let g = {
        let mut t = inner.tables();
        match t.groups.get(&gid) {
            Some(g) if g.closing && g.live.is_empty() && g.pending == 0 => t.groups.remove(&gid),
            _ => None,
        }
    };
    let Some(mut g) = g else { return };
    if let Some(h) = &g.handle {
        for leaf in std::mem::take(&mut g.leaves) {
            inner.domain.close_leaf(h, leaf);
        }
    }
    if let Some(h) = g.handle.take() {
        match Arc::try_unwrap(h) {
            Ok(h) => inner.domain.close_group(h),
            Err(_) => inner.log(&format!("{gid}: its handle is still in use; not released")),
        }
    }
    drop(g.reap.take());
    for (_, d) in g.kill_done {
        d();
    }
}

fn shutdown(inner: &Arc<Inner>, grace_ms: u32, done: Done) {
    let procs: Vec<Arc<Proc>> = {
        let mut t = inner.tables();
        t.shutting = true;
        t.shutdown_done.push(done);
        t.procs.values().cloned().collect()
    };
    if procs.is_empty() {
        return shutdown_finish(inner);
    }
    for p in &procs {
        p.mark_kill(ExitReason::Killed(KillReason::Shutdown));
        p.writer.close();
    }
    let w = Arc::downgrade(inner);
    inner.timers.at(Instant::now() + ms_(grace_ms), move || {
        if let Some(i) = w.upgrade() {
            i.post(Cmd::ShutdownKill);
        }
    });
}

fn live_handles(inner: &Inner) -> Vec<Arc<GroupHandle>> {
    let t = inner.tables();
    t.groups
        .values()
        .filter(|g| !g.live.is_empty())
        .filter_map(|g| g.handle.clone())
        .collect()
}

fn shutdown_kill(inner: &Arc<Inner>) {
    for h in live_handles(inner) {
        kill_domain(inner, Some(h));
    }
    let w = Arc::downgrade(inner);
    let wait = ms_(inner.opts.timing.kill_wait_ms);
    inner.timers.at(Instant::now() + wait, move || {
        if let Some(i) = w.upgrade() {
            i.post(Cmd::ShutdownFinish);
        }
    });
}

/// Marks every group closing and releases the empty ones; returns the shutdown callbacks.
fn close_all(inner: &Inner) -> Vec<Done> {
    let (done, empty) = {
        let mut t = inner.tables();
        let done = std::mem::take(&mut t.shutdown_done);
        let mut empty = Vec::new();
        for (id, g) in &mut t.groups {
            g.closing = true;
            if g.live.is_empty() && g.pending == 0 {
                empty.push(*id);
            }
        }
        (done, empty)
    };
    for id in empty {
        finalize(inner, id);
    }
    done
}

fn shutdown_finish(inner: &Inner) {
    if inner.tables().shutdown_done.is_empty() {
        return;
    }
    for d in close_all(inner) {
        d();
    }
}

/// The host is dropped: kill everything, release what is empty, refuse the rest.
fn stop(inner: &Inner) {
    let procs: Vec<Arc<Proc>> = {
        let mut t = inner.tables();
        t.shutting = true;
        t.procs.values().cloned().collect()
    };
    for p in &procs {
        p.mark_kill(ExitReason::Killed(KillReason::Shutdown));
    }
    for h in live_handles(inner) {
        kill_domain(inner, Some(h));
    }
    for d in close_all(inner) {
        d();
    }
}
