//! The `jobhost-spawn` thread (Decision 11): every spawn, kill, close and the shutdown run
//! here, in order. It lives as long as the host, so on Linux `PR_SET_PDEATHSIG` (which follows
//! the thread that forked) fires only when the host itself goes. Callbacks (`done`) run here
//! or on a waiter thread and must be quick; a panicking one is caught and logged
//! ([`Inner::run_done`]), so it skips no release, reap or shutdown step.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Weak};
use std::time::Instant;

use fin_jobs::sandbox;

use crate::domain::GroupHandle;
use crate::exit::{ExitReason, RawExit};
use crate::framing::Expect;
use crate::host::{Inner, ms_};
use crate::os::{self, SpawnCtx};
use crate::proc::{self, Proc};
use crate::spawner::{self, Program, Role, SpawnRequest};
use crate::{GroupId, GroupSpec, HostError, HostEvent, KillReason, ProcId};

/// A callback with no result.
pub type Done = Box<dyn FnOnce() + Send>;
/// A spawn's callback. On success it runs on the spawn thread before the process's
/// `Spawned` and before its readers start, so it comes before every event of the process.
pub type SpawnDone = Box<dyn FnOnce(Result<ProcId, HostError>) + Send>;

pub(crate) enum Cmd {
    Spawn(GroupId, Box<SpawnRequest>, SpawnDone),
    Kill(GroupId, KillReason, Option<Done>),
    KillFor(ProcId, ExitReason),
    KillWaitOver(GroupId),
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
        Cmd::Spawn(g, req, done) => {
            #[cfg(feature = "test-hooks")]
            if inner.opts.panic_on_spawn_for_test {
                panic!("test: a spawn command panics");
            }
            spawn(inner, g, *req, done)
        }
        Cmd::Kill(g, reason, done) => kill(inner, g, reason, done),
        Cmd::KillFor(p, reason) => kill_for(inner, p, reason),
        Cmd::KillWaitOver(g) => kill_wait_over(inner, g),
        Cmd::Close(g) => close(inner, g),
        Cmd::Finalize(g) => finalize(inner, g),
        Cmd::Shutdown(grace, done) => shutdown(inner, grace, done),
        Cmd::ShutdownKill => shutdown_kill(inner),
        Cmd::ShutdownFinish => shutdown_finish(inner),
        Cmd::Stop => stop(inner),
    }
}

fn spawn(inner: &Arc<Inner>, gid: GroupId, req: SpawnRequest, done: SpawnDone) {
    let refused = {
        let mut t = inner.tables();
        let (next, shutting) = (t.next_group, t.shutting);
        match t.groups.get_mut(&gid) {
            _ if shutting => Err(HostError::ShutDown),
            None if gid.0 <= next => Err(HostError::GroupClosed(gid)),
            None => Err(HostError::UnknownGroup(gid)),
            Some(g) if g.closing => Err(HostError::GroupClosed(gid)),
            Some(g) => {
                g.pending += 1;
                Ok((g.handle.clone(), g.spec.clone()))
            }
        }
    };
    let (handle, spec) = match refused {
        Ok(x) => x,
        Err(e) => return done(Err(e)),
    };
    let mut done = Some(done);
    if let Err(e) = spawn_in(inner, gid, handle, &spec, req, &mut done) {
        if let Some(g) = inner.tables().groups.get_mut(&gid) {
            g.pending -= 1;
        }
        // As after an exit: `GroupEmpty`, the kill callbacks, the release if closing.
        inner.settle(gid);
        if let Some(d) = done.take() {
            d(Err(e));
        }
    }
}

/// Spawns the process; on success `done` has run (before the process's first event).
fn spawn_in(
    inner: &Arc<Inner>,
    gid: GroupId,
    handle: Option<Arc<GroupHandle>>,
    spec: &GroupSpec,
    req: SpawnRequest,
    done: &mut Option<SpawnDone>,
) -> Result<(), HostError> {
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
    let spawned = match os::spawn(cmd, &ctx) {
        Ok(s) => s,
        Err(e) => {
            inner.domain.close_leaf(&handle, leaf);
            return Err(HostError::from_spawn(e));
        }
    };
    let expect = Expect {
        bundled: matches!(req.program, Program::Bundled(_)),
        helper: matches!(req.role, Role::Helper { .. }),
        self_sandbox: self_sandbox.clone(),
        unconfined_ok: inner.opts.unconfined_ok,
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
        ));
        t.procs.insert(id, p.clone());
        let g = t
            .groups
            .get_mut(&gid)
            .expect("a group with a pending spawn stays");
        g.pending -= 1;
        g.spawned += 1;
        g.live.insert(id);
        g.leaves.push(leaf);
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
    // The caller learns the id before any event of the process, then `Spawned` leads them.
    if let Some(d) = done.take() {
        let id = p.id;
        inner.run_done(Box::new(move || d(Ok(id))));
    }
    p.emit(HostEvent::Spawned {
        proc: p.id,
        group: gid,
    });
    // Kept here should the waiter not start (`no_waiter` then needs the process handle).
    let slot = Arc::new(std::sync::Mutex::new(Some(os_proc)));
    #[cfg(feature = "test-hooks")]
    let no_thread = inner.opts.fail_waiter_thread_for_test;
    #[cfg(not(feature = "test-hooks"))]
    let no_thread = false;
    let waiter = !no_thread
        && thread("jobhost-wait", {
            let (p, w, slot) = (p.clone(), w.clone(), slot.clone());
            move || {
                let os = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(os) = os {
                    proc::wait_exit(p, w, os, drain, reap);
                }
            }
        });
    let started = [
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
    if !waiter {
        let os = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        no_waiter(inner, &p, os);
    } else if started.iter().any(|ok| !ok) {
        inner.log(&format!("{}: a thread did not start; killing it", p.id));
        kill_for(inner, p.id, ExitReason::SpawnFailed("thread".into()));
    }
    inner.arm_hello(&p);
    Ok(())
}

/// The waiter did not start: kill the process and do the waiter's work here, so its `Exit`
/// still goes out. A Unix leader stays a zombie until its group is released.
fn no_waiter(inner: &Arc<Inner>, p: &Arc<Proc>, os: Option<os::OsProc>) {
    inner.log(&format!("{}: no waiter thread; killing it", p.id));
    p.mark_kill(ExitReason::SpawnFailed("thread".into()));
    let raw = match os {
        Some(os) => {
            os::kill_one(&os, p.pid);
            os::wait(&os, p.pid, !p.leader).unwrap_or_default()
        }
        None => RawExit::default(),
    };
    p.state().exited = true;
    p.writer.kill();
    #[cfg(unix)]
    if p.leader
        && let Some(g) = inner.tables().groups.get_mut(&p.group)
    {
        g.reap = None;
        g.orphan_leader = Some(p.pid);
    }
    let info = inner.exit_info(p, raw);
    p.emit_exit(info);
    inner.exited(p);
}

fn thread(name: &str, f: impl FnOnce() + Send + 'static) -> bool {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .is_ok()
}

/// Kills the group's domain and starts its kill wait: once it passes, a process still alive
/// is abandoned (Decision 18).
fn kill_domain(inner: &Arc<Inner>, gid: GroupId, handle: Option<Arc<GroupHandle>>) {
    let Some(h) = handle else { return };
    if let Err(e) = inner.domain.kill(&h) {
        inner.log(&format!("kill of {} failed: {e}", h.id));
    }
    let at = Instant::now();
    if let Some(g) = inner.tables().groups.get_mut(&gid) {
        g.killed_at = Some(at);
    }
    let w: Weak<Inner> = Arc::downgrade(inner);
    inner
        .timers
        .at(at + ms_(inner.opts.timing.kill_wait_ms), move || {
            if let Some(i) = w.upgrade() {
                i.post(Cmd::KillWaitOver(gid));
            }
        });
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
                inner.run_done(d);
            }
            return;
        }
        for p in &live {
            p.mark_kill(ExitReason::Killed(reason));
        }
        let g = t.groups.get_mut(&gid).expect("checked above");
        if let Some(d) = done {
            g.kill_done
                .push((Instant::now() + ms_(inner.opts.timing.kill_wait_ms), d));
        }
        g.handle.clone()
    };
    kill_domain(inner, gid, handle);
}

fn kill_for(inner: &Arc<Inner>, pid: ProcId, reason: ExitReason) {
    let (gid, handle) = {
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
        (p.group, g.handle.clone())
    };
    kill_domain(inner, gid, handle);
}

/// The kill wait passed. Each process the kill should have ended and that is still alive
/// (uninterruptible I/O) is **abandoned**: reported once as `Abandoned`, released from the
/// tables (so `GroupEmpty` and the kill callbacks follow), and only logged when it finally
/// exits (Decision 18). Then the due callbacks run.
fn kill_wait_over(inner: &Inner, gid: GroupId) {
    let wait = ms_(inner.opts.timing.kill_wait_ms);
    let (due, stuck) = {
        let mut t = inner.tables();
        let Some(g) = t.groups.get_mut(&gid) else {
            return;
        };
        let now = Instant::now();
        let (due, keep): (Vec<_>, Vec<_>) = g.kill_done.drain(..).partition(|(at, _)| *at <= now);
        g.kill_done = keep;
        let killed = g.killed_at.filter(|&k| k + wait <= now);
        let live: Vec<ProcId> = g.live.iter().copied().collect();
        let stuck: Vec<Arc<Proc>> = live
            .iter()
            .filter_map(|id| t.procs.get(id))
            .filter(|p| killed.is_some_and(|k| p.started <= k) && p.state().kill.is_some())
            .cloned()
            .collect();
        (due, stuck)
    };
    #[cfg(feature = "test-hooks")]
    let delay = inner.opts.abandon_emit_delay_for_test;
    #[cfg(not(feature = "test-hooks"))]
    let delay = None;
    for p in stuck {
        let mut info = inner.exit_info(&p, RawExit::default());
        info.reason = ExitReason::Abandoned;
        if !p.abandon_with_exit(info, delay) {
            continue;
        }
        inner.log(&format!(
            "{} (pid {}) is still alive {wait:?} after its kill: abandoned",
            p.id, p.pid
        ));
        inner.exited(&p);
    }
    for (_, d) in due {
        inner.run_done(d);
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
    if let Some(h) = g.handle.take() {
        for leaf in g.leaves.drain(..) {
            inner.domain.close_leaf(&h, leaf);
        }
        inner.domain.close_group(&h);
    }
    drop(g.reap.take());
    #[cfg(unix)]
    if let Some(pid) = g.orphan_leader {
        os::reap(pid);
    }
    for (_, d) in g.kill_done {
        inner.run_done(d);
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

fn live_handles(inner: &Inner) -> Vec<(GroupId, Arc<GroupHandle>)> {
    let t = inner.tables();
    t.groups
        .iter()
        .filter(|(_, g)| !g.live.is_empty())
        .filter_map(|(id, g)| Some((*id, g.handle.clone()?)))
        .collect()
}

fn shutdown_kill(inner: &Arc<Inner>) {
    for (gid, h) in live_handles(inner) {
        kill_domain(inner, gid, Some(h));
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
        inner.run_done(d);
    }
}

/// The host is dropped: kill everything, release what is empty, refuse the rest.
fn stop(inner: &Arc<Inner>) {
    let procs: Vec<Arc<Proc>> = {
        let mut t = inner.tables();
        t.shutting = true;
        t.procs.values().cloned().collect()
    };
    for p in &procs {
        p.mark_kill(ExitReason::Killed(KillReason::Shutdown));
    }
    for (gid, h) in live_handles(inner) {
        kill_domain(inner, gid, Some(h));
    }
    for d in close_all(inner) {
        inner.run_done(d);
    }
}
