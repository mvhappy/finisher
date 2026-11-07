//! `fin-jobhost` on Unix: process groups, reaping, PDEATHSIG, the spawn thread, `pre_exec`,
//! `EINTR`, inherited descriptors and QoS (5C-1 plan, Task 3). Step 0
//! (`sigcont_resumes_start_suspended`) is macOS only.

#![cfg(unix)]

mod host_support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use fin_jobhost::*;
use host_support::*;
use serde_json::json;

/// Aborts (exit 99) on any allocation in a forked child of this test process before it execs
/// (`pre_exec_does_not_allocate`). Armed with the host's pid.
struct NoAllocInChild;

static PARENT: AtomicI32 = AtomicI32::new(0);

fn in_forked_child() -> bool {
    let parent = PARENT.load(Ordering::Relaxed);
    // SAFETY: getpid has no preconditions.
    parent != 0 && unsafe { libc::getpid() } != parent
}

// SAFETY: forwards to the system allocator; only adds the check.
unsafe impl GlobalAlloc for NoAllocInChild {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if in_forked_child() {
            // SAFETY: ends the child at once, without running anything else.
            unsafe { libc::_exit(99) };
        }
        // SAFETY: as the caller promised.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as the caller promised.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if in_forked_child() {
            // SAFETY: as in `alloc`.
            unsafe { libc::_exit(99) };
        }
        // SAFETY: as the caller promised.
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOC: NoAllocInChild = NoAllocInChild;

fn pgid_of(pid: u32) -> i32 {
    // SAFETY: getpgid only reads.
    unsafe { libc::getpgid(pid as i32) }
}

#[test]
fn processes_share_the_group_pgid() {
    let mut h = host();
    let g = h.group();
    let a = h.worker(g);
    let b = h.worker(g);
    let pa = h.hello(a).pid;
    let pb = h.hello(b).pid;
    assert_eq!(pgid_of(pa), pa as i32, "the first process makes the group");
    assert_eq!(pgid_of(pb), pa as i32, "later ones join it");
    assert_ne!(pgid_of(pa), pgid_of(std::process::id()));
    h.kill(g, KillReason::Test);
    assert_eq!(h.exit(a).reason, ExitReason::Killed(KillReason::Test));
    assert_eq!(h.exit(b).reason, ExitReason::Killed(KillReason::Test));
    h.group_empty(g);
}

#[test]
fn cancel_kills_double_forked_grandchild() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(
        p,
        "test.spawn",
        json!({ "detach": "none", "out": "pids.json" }),
    );
    let pids = spawn_pids(&h, "pids.json");
    assert!(!gone(pids.grandchild));
    assert_eq!(pgid_of(pids.grandchild), pgid_of(pids.worker));
    h.kill(g, KillReason::Cancel);
    assert_eq!(h.exit(p).reason, ExitReason::Killed(KillReason::Cancel));
    assert!(wait_gone(pids.worker), "the worker is gone");
    assert!(wait_gone(pids.grandchild), "the grandchild is gone");
}

#[test]
fn leader_unreaped_until_close_group() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    let pid = h.hello(p).pid as i32;
    h.start(p, "test.echo", json!({ "value": 1 }));
    h.result(p);
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
    h.group_empty(g);
    std::thread::sleep(Duration::from_millis(100));
    // SAFETY: signal 0 only checks for existence.
    assert_eq!(
        unsafe { libc::kill(-pid, 0) },
        0,
        "the zombie leader keeps its pgid"
    );
    h.host.close_group(g);
    let until = Instant::now() + WAIT;
    // SAFETY: as above.
    while unsafe { libc::kill(-pid, 0) } == 0 {
        assert!(Instant::now() < until, "close_group reaps the leader");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn kill_on_closed_group_is_a_no_op() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.echo", json!({ "value": 1 }));
    h.result(p);
    h.exit(p);
    h.host.close_group(g);
    let t0 = Instant::now();
    h.kill(g, KillReason::Test);
    assert!(t0.elapsed() < Duration::from_secs(1), "answered at once");
    // Another group's process is untouched.
    let g2 = h.group();
    let p2 = h.worker(g2);
    h.hello(p2);
    h.kill(g, KillReason::Test);
    h.start(p2, "test.echo", json!({ "value": 2 }));
    assert_eq!(h.result(p2)["value"], 2);
}

#[cfg(target_os = "linux")]
#[test]
fn pdeathsig_set_on_linux() {
    let mut h = host();
    let (_, v) = h.run("test.echo", json!({ "value": 1, "report": ["pdeathsig"] }));
    assert_eq!(v["pdeathsig"], libc::SIGKILL);
}

#[test]
fn pdeathsig_survives_spawning_thread_exit() {
    let mut h = host();
    let g = h.group();
    let req = h.req(&["test"]);
    let (tx, rx) = mpsc::channel();
    let host = &h.host;
    std::thread::scope(|s| {
        s.spawn(|| {
            host.spawn(
                g,
                req,
                Box::new(move |r| {
                    let _ = tx.send(r);
                }),
            )
        });
    });
    let p = rx.recv_timeout(WAIT).unwrap().unwrap();
    h.hello(p);
    std::thread::sleep(Duration::from_millis(300));
    h.start(p, "test.echo", json!({ "value": 7 }));
    assert_eq!(
        h.result(p)["value"],
        7,
        "the worker outlived the thread that asked for it"
    );
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
}

#[cfg(target_os = "linux")]
#[test]
fn pre_exec_does_not_allocate() {
    // SAFETY: getpid has no preconditions.
    PARENT.store(unsafe { libc::getpid() }, Ordering::Relaxed);
    let mut h = host();
    for _ in 0..5 {
        let (_, v) = h.run("test.echo", json!({ "value": 1 }));
        assert_eq!(
            v["value"], 1,
            "a child that allocated before exec would exit 99"
        );
    }
}

/// Signals every `jobhost-*` thread of this process (Linux) or the process (elsewhere).
fn storm(stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        #[cfg(target_os = "linux")]
        if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
            for t in tasks.flatten() {
                let comm = std::fs::read_to_string(t.path().join("comm")).unwrap_or_default();
                if let (true, Ok(tid)) = (
                    comm.starts_with("jobhost-"),
                    t.file_name().to_string_lossy().parse::<i32>(),
                ) {
                    // SAFETY: tgkill to a thread of this process with a handled signal.
                    unsafe { libc::syscall(libc::SYS_tgkill, libc::getpid(), tid, libc::SIGUSR1) };
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        // SAFETY: a handled signal to this process.
        unsafe {
            libc::kill(libc::getpid(), libc::SIGUSR1);
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

extern "C" fn on_usr1(_: libc::c_int) {}

#[test]
fn eintr_is_retried() {
    // SAFETY: installs a no-op handler without SA_RESTART, so blocking calls see EINTR.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_usr1 as *const () as usize;
        sa.sa_flags = 0;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGUSR1, &sa, std::ptr::null_mut());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let stormer = std::thread::spawn(move || storm(&s));
    let mut h = host();
    for i in 0..10 {
        let (p, v) = h.run("test.sleep", json!({ "ms": 30, "progressEveryMs": 5 }));
        assert!(v["sleptMs"].as_u64().unwrap() >= 30, "run {i}");
        let exit = h.exit(p);
        assert_eq!(
            (exit.reason, exit.code),
            (ExitReason::Exited, Some(0)),
            "run {i}"
        );
    }
    stop.store(true, Ordering::Relaxed);
    stormer.join().unwrap();
}

#[test]
fn only_stdio_is_inherited() {
    // 50 descriptors without close-on-exec.
    // SAFETY: dup of stdin; each copy is closed below.
    let extra: Vec<i32> = (0..50).map(|_| unsafe { libc::dup(0) }).collect();
    assert!(extra.iter().all(|&fd| fd > 2));
    let mut h = host();
    let (_, v) = h.run("test.echo", json!({ "value": 1, "report": ["fds"] }));
    for fd in extra {
        // SAFETY: closes our own copies.
        unsafe { libc::close(fd) };
    }
    assert_eq!(v["fds"], json!([0, 1, 2]));
}

#[cfg(target_os = "linux")]
#[test]
fn qos_nice_applied() {
    // SAFETY: reads this process's priority.
    let base = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
    let mut h = host();
    for (qos, nice) in [
        (Qos::Normal, base),
        (Qos::Utility, 5.max(base)),
        (Qos::Background, 10.max(base)),
    ] {
        let g = h.group();
        let mut req = h.req(&["test"]);
        req.qos = qos;
        let p = h.spawn(g, req).unwrap();
        h.hello(p);
        h.start(p, "test.echo", json!({ "value": 1, "report": ["nice"] }));
        assert_eq!(h.result(p)["nice"], nice, "{qos:?}");
    }
}

#[cfg(target_os = "macos")]
fn task_priority(pid: u32) -> i32 {
    // SAFETY: proc_pidinfo fills one proc_taskinfo.
    unsafe {
        let mut info: libc::proc_taskinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
        let n = libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            (&mut info as *mut libc::proc_taskinfo).cast(),
            size,
        );
        assert_eq!(n, size);
        info.pti_priority
    }
}

#[cfg(target_os = "macos")]
#[test]
fn qos_nice_applied() {
    let mut h = host();
    let mut prio = Vec::new();
    for qos in [Qos::Normal, Qos::Utility, Qos::Background] {
        let g = h.group();
        let mut req = h.req(&["test"]);
        req.qos = qos;
        let p = h.spawn(g, req).unwrap();
        prio.push(task_priority(h.hello(p).pid));
        h.kill(g, KillReason::Test);
    }
    assert!(
        prio[0] > prio[1] && prio[1] > prio[2],
        "QoS lowers the priority: {prio:?}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_spawn_is_suspended_until_registered() {
    let mut h = host_with(|o| o.resume_delay_for_test = Some(Duration::from_millis(500)));
    let g = h.group();
    let t0 = Instant::now();
    let p = h.worker(g);
    assert!(
        t0.elapsed() >= Duration::from_millis(500),
        "spawn returns after the resume"
    );
    h.hello(p);
    // The hello came only after the resume: its seq is 1 and nothing preceded it.
    h.start(p, "test.echo", json!({ "value": 1 }));
    h.result(p);
}

/// Step 0 (Decision 13): a process made with `POSIX_SPAWN_START_SUSPENDED` writes nothing
/// until `kill(pid, SIGCONT)`, then runs.
#[cfg(target_os = "macos")]
#[test]
fn sigcont_resumes_start_suspended() {
    use std::ffi::CString;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let (mut r, w) = std::io::pipe().unwrap();
    let sh = CString::new("/bin/sh").unwrap();
    let args = [
        sh.clone(),
        CString::new("-c").unwrap(),
        CString::new("echo resumed").unwrap(),
    ];
    let mut argv: Vec<*mut libc::c_char> = args.iter().map(|a| a.as_ptr() as _).collect();
    argv.push(std::ptr::null_mut());
    let envp: [*mut libc::c_char; 1] = [std::ptr::null_mut()];
    let mut pid = 0;
    // SAFETY: plain posix_spawn of /bin/sh with its stdout on our pipe.
    unsafe {
        let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
        let mut fa: libc::posix_spawn_file_actions_t = std::mem::zeroed();
        assert_eq!(libc::posix_spawnattr_init(&mut attr), 0);
        assert_eq!(libc::posix_spawn_file_actions_init(&mut fa), 0);
        let flags = libc::POSIX_SPAWN_START_SUSPENDED | libc::POSIX_SPAWN_SETPGROUP;
        assert_eq!(libc::posix_spawnattr_setflags(&mut attr, flags as _), 0);
        assert_eq!(
            libc::posix_spawn_file_actions_adddup2(&mut fa, w.as_raw_fd(), 1),
            0
        );
        assert_eq!(
            libc::posix_spawn(
                &mut pid,
                sh.as_ptr(),
                &fa,
                &attr,
                argv.as_ptr(),
                envp.as_ptr()
            ),
            0
        );
        libc::posix_spawnattr_destroy(&mut attr);
        libc::posix_spawn_file_actions_destroy(&mut fa);
    }
    drop(w);
    // Suspended: nothing arrives within 500 ms.
    // SAFETY: fcntl on our own descriptor.
    unsafe { libc::fcntl(r.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
    std::thread::sleep(Duration::from_millis(500));
    let mut buf = [0u8; 64];
    let early = r.read(&mut buf);
    assert!(
        matches!(&early, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "{early:?}"
    );
    // SAFETY: resumes our own child.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
    // SAFETY: as above.
    unsafe { libc::fcntl(r.as_raw_fd(), libc::F_SETFL, 0) };
    let mut out = String::new();
    r.read_to_string(&mut out).unwrap();
    assert_eq!(out, "resumed\n");
    let mut status = 0;
    // SAFETY: reaps our own child.
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
}
