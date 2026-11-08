//! `fin-jobhost` on Windows (5C-1 plan, Task 5): the host → group → leaf Jobs, suspended
//! spawns, leaf limits, the kill tree, crash UI, inherited handles, exits without the
//! completion port, the environment block, the group memory limit, foreign processes and
//! priority classes. Host death is in `host_death_windows.rs`.

#![cfg(windows)]

mod host_support;

use std::io::{self, PipeReader};
use std::os::windows::io::AsRawHandle;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use fin_jobhost::*;
use fin_jobs::WorkerMsg;
use host_support::*;
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation,
};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass, NORMAL_PRIORITY_CLASS, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

fn job_limits(f: impl Fn(&mut JobLimits) + Send + Sync + 'static) -> Arc<dyn Spawner> {
    rewrite(move |cmd| f(&mut cmd.job_limits))
}

/// Runs a hidden entry of the test binary (no `hello`) in a new group; its `Exit`.
fn run_bare(h: &mut TestHost, args: &[&str]) -> ExitInfo {
    let g = h.group();
    let p = h.spawn(g, h.req(args)).expect("it spawns");
    h.exit(p)
}

#[test]
fn caps_are_job_objects() {
    let caps = host().host.caps();
    assert_eq!(caps.kill_tree, KillTree::JobObject);
    assert!(caps.leaf_limits && caps.cgroup_memory && !caps.watchdog);
}

#[test]
fn process_is_in_its_leaf_job_before_it_runs() {
    let probes = Arc::new(Mutex::new(Vec::new()));
    let seen = probes.clone();
    let mut h = host_with(move |o| {
        o.suspended_hook_for_test = Some(Arc::new(move |p: &SuspendedProbe| {
            seen.lock().unwrap().push(*p);
            Ok(())
        }));
    });
    let g = h.group();
    let p1 = h.worker(g);
    let pid1 = h.hello(p1).pid;
    let p2 = h.worker(g);
    let pid2 = h.hello(p2).pid;
    let probes = probes.lock().unwrap().clone();
    assert_eq!(probes.len(), 2, "{probes:?}");
    for (probe, pid) in probes.iter().zip([pid1, pid2]) {
        assert_eq!(probe.pid, pid);
        assert!(
            probe.in_host && probe.in_group && probe.in_leaf,
            "in all three Jobs while suspended: {probe:?}"
        );
        assert_eq!(probe.suspend_count, 1, "still suspended: {probe:?}");
    }
}

#[test]
fn spawn_failure_leaves_no_suspended_process() {
    let pids = Arc::new(Mutex::new(Vec::new()));
    let seen = pids.clone();
    let h = host_with(move |o| {
        o.suspended_hook_for_test = Some(Arc::new(move |p: &SuspendedProbe| {
            seen.lock().unwrap().push(p.pid);
            Err(io::Error::other("assignment forced to fail"))
        }));
    });
    let g = h.group();
    let err = h.spawn(g, h.req(&["test"])).unwrap_err();
    assert!(matches!(err, HostError::Spawn(_)), "{err}");
    let pid = pids.lock().unwrap()[0];
    assert!(wait_gone(pid), "the suspended process was terminated");
}

#[test]
fn leaf_active_process_limit() {
    let mut h = host_with(|o| o.spawner = job_limits(|l| l.active_processes = Some(1)));
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(
        p,
        "test.spawn",
        json!({ "detach": "none", "out": "pids.json" }),
    );
    match h.message(p) {
        WorkerMsg::Error { error, .. } => assert_eq!(error.code, "IO_ERROR", "{error:?}"),
        m => panic!("the worker cannot start a child, got {m:?}"),
    }
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
}

#[test]
fn leaf_ui_limits_applied() {
    let mut h = host_with(|o| {
        o.spawner = job_limits(|l| {
            l.active_processes = Some(3);
            l.ui = Some(UiLimits(0xFF));
        })
    });
    let out = h.dir.path().join("job.json");
    let exit = run_bare(&mut h, &["--test-job-report", out.to_str().unwrap()]);
    assert_eq!((exit.reason, exit.code), (ExitReason::Exited, Some(0)));
    let r: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(r["ui"], 0xFF, "{r}");
    assert_eq!(r["activeLimit"], 3, "{r}");
    // ACTIVE_PROCESS (8) and DIE_ON_UNHANDLED_EXCEPTION (0x400) on the leaf.
    assert_eq!(r["limitFlags"].as_u64().unwrap() & 0x408, 0x408, "{r}");
}

#[test]
fn cancel_terminates_job_tree() {
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
    assert!(!gone(pids.grandchild), "the grandchild runs");
    h.kill(g, KillReason::Cancel);
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::Killed(KillReason::Cancel));
    assert!(killed(&exit), "{exit:?}");
    assert!(wait_gone(pids.worker), "the worker is gone");
    assert!(wait_gone(pids.grandchild), "the grandchild is gone");
}

#[test]
fn foreign_processes_counted_from_total_processes() {
    let mut h = host();
    let (p, _) = h.run("test.echo", json!({ "value": 1 }));
    assert_eq!(h.exit(p).foreign, Some(0));
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(
        p,
        "test.spawn",
        json!({ "detach": "none", "out": "pids.json" }),
    );
    spawn_pids(&h, "pids.json");
    h.kill(g, KillReason::Test);
    // The middle process and the grandchild.
    assert_eq!(h.exit(p).foreign, Some(2));
}

#[test]
fn breakaway_is_refused() {
    let mut h = host();
    let exit = run_bare(&mut h, &["--test-breakaway"]);
    assert_eq!(
        exit.code,
        Some(10),
        "CREATE_BREAKAWAY_FROM_JOB is refused: {exit:?}"
    );
}

/// The `WerFault.exe` command lines naming `pid` seen over the next `secs` seconds.
fn wer_watch(pid: u32, secs: u32) -> std::process::Child {
    let script = format!(
        "$end=(Get-Date).AddSeconds({secs}); $hit=@{{}}; \
         while((Get-Date) -lt $end) {{ \
           Get-CimInstance Win32_Process -Filter \"Name='WerFault.exe'\" | \
           ForEach-Object {{ $c=[string]$_.CommandLine; \
             if ($c -match '-p\\s+{pid}(\\s|$)') {{ $hit[$c]=1 }} }}; \
           Start-Sleep -Milliseconds 100 }}; \
         $hit.Keys"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("powershell runs")
}

fn crash_without_wer(how: &str, want: u32) {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    let pid = h.hello(p).pid;
    let watch = wer_watch(pid, 7);
    // Let the watcher's first query run before the crash.
    std::thread::sleep(Duration::from_millis(1500));
    let t0 = Instant::now();
    h.start(
        p,
        "test.crash",
        json!({ "items": ["a"], "poison": ["a"], "how": how }),
    );
    let exit = h.exit(p);
    let took = t0.elapsed();
    assert_eq!(exit.reason, ExitReason::Exited, "{how}");
    assert_eq!(exit.code, Some(i64::from(want)), "{how}: {exit:?}");
    assert!(
        took < Duration::from_secs(2),
        "{how}: exited after {took:?}"
    );
    let out = watch.wait_with_output().unwrap();
    let hits = String::from_utf8_lossy(&out.stdout);
    assert!(
        hits.trim().is_empty(),
        "{how}: WerFault ran for the worker: {hits}"
    );
}

#[test]
fn unhandled_exception_dies_without_wer() {
    // An access violation (0xC0000005) and a RaiseException(EXCEPTION_IN_PAGE_ERROR).
    crash_without_wer("segv", 0xC000_0005);
    crash_without_wer("bus", 0xC000_0006);
}

#[test]
fn fastfail_dies_without_wer() {
    crash_without_wer("fastfail", 0xC000_0409);
    // Rust's abort is a __fastfail too.
    crash_without_wer("abort", 0xC000_0409);
}

/// 50 pipes whose write ends are inheritable in this process.
fn inheritable_pipes() -> Vec<(PipeReader, io::PipeWriter)> {
    (0..50)
        .map(|_| {
            let (r, w) = io::pipe().unwrap();
            // SAFETY: a pipe end we own; only its inherit flag changes.
            let ok = unsafe {
                SetHandleInformation(
                    w.as_raw_handle() as HANDLE,
                    HANDLE_FLAG_INHERIT,
                    HANDLE_FLAG_INHERIT,
                )
            };
            assert_ne!(ok, 0);
            (r, w)
        })
        .collect()
}

fn bytes_waiting(r: &PipeReader) -> u32 {
    let mut avail = 0u32;
    // SAFETY: a pipe end we own; only the count is read.
    let ok = unsafe {
        PeekNamedPipe(
            r.as_raw_handle() as HANDLE,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut avail,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(ok, 0, "{}", io::Error::last_os_error());
    avail
}

#[test]
fn only_stdio_is_inherited() {
    let pipes = inheritable_pipes();
    let values: Vec<String> = pipes
        .iter()
        .map(|(_, w)| (w.as_raw_handle() as usize).to_string())
        .collect();
    let mut args = vec!["--test-poke-handles"];
    args.extend(values.iter().map(String::as_str));
    let mut h = host();
    let exit = run_bare(&mut h, &args);
    assert_eq!(exit.code, Some(0), "{exit:?}");
    let leaked = pipes.iter().filter(|(r, _)| bytes_waiting(r) > 0).count();
    assert_eq!(leaked, 0, "the worker wrote to {leaked} inherited handles");
    // The probe works: a child that inherits every inheritable handle reaches them.
    let status = Command::new(BIN)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let reached = pipes.iter().filter(|(r, _)| bytes_waiting(r) > 0).count();
    assert_eq!(reached, 50, "the control child inherits them all");
}

/// A thread spawning children that inherit every inheritable handle (as libuv and `std` do)
/// while the host spawns 200 workers. Those children live 10 s, so a pipe end leaked into one
/// keeps the worker's stdout open after it exits and holds its `Exit` back for the whole exit
/// drain (3 s here); without a leak `Exit` follows at once. Half the workers end by a `result`,
/// half by stdin EOF (exit 70), which must reach every one of them.
#[test]
fn concurrent_libuv_spawn_inherits_no_pipe() {
    let mut h = host_with(|o| o.timing.exit_drain_ms = 3_000);
    let stop = Arc::new(AtomicBool::new(false));
    let spawner = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut kids: Vec<std::process::Child> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                kids.retain_mut(|c| c.try_wait().map_or(true, |s| s.is_none()));
                if kids.len() >= 100 {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                if let Ok(c) = Command::new(BIN)
                    .args(["--test-sleep", "10000"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    kids.push(c);
                }
            }
            let n = kids.len();
            for mut c in kids {
                let _ = c.kill();
                let _ = c.wait();
            }
            n
        })
    };
    let mut slow = Vec::new();
    let mut done = 0;
    while done < 200 && slow.len() < 5 {
        let procs: Vec<ProcId> = (0..10)
            .map(|_| {
                let g = h.group();
                h.worker(g)
            })
            .collect();
        for (i, &p) in procs.iter().enumerate() {
            h.hello(p);
            let t0 = Instant::now();
            let exit = if i % 2 == 0 {
                h.start(p, "test.echo", json!({ "value": i }));
                h.result(p);
                h.exit(p)
            } else {
                h.host.close_stdin(p);
                let e = h.exit(p);
                assert_eq!(e.code, Some(70), "stdin EOF reaches the worker: {e:?}");
                e
            };
            let took = t0.elapsed();
            if took > Duration::from_millis(1500) {
                slow.push((done, took, exit.reason));
            }
            done += 1;
        }
    }
    stop.store(true, Ordering::Relaxed);
    let live = spawner.join().unwrap();
    assert!(
        slow.is_empty(),
        "exits that waited for the drain (a leaked pipe end), after {done} workers with {live} \
         inheriting children alive: {slow:?}"
    );
}

#[test]
fn exits_come_from_process_handles() {
    let mut h = host_with(|o| o.port_stopped_for_test = true);
    for i in 0..20 {
        let (p, v) = h.run("test.echo", json!({ "value": i }));
        assert_eq!(v["value"], i);
        let exit = h.exit(p);
        assert_eq!((exit.reason, exit.code), (ExitReason::Exited, Some(0)));
        // The console host is recognised from a snapshot alone.
        assert_eq!(exit.foreign, Some(0), "run {i}");
    }
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.hang", json!({}));
    h.kill(g, KillReason::Test);
    assert_eq!(h.exit(p).reason, ExitReason::Killed(KillReason::Test));
}

#[test]
fn env_block_is_explicit() {
    let mut h = host();
    let g = h.group();
    let mut req = h.req(&["test"]);
    req.env.push(("Path".into(), r"C:\nowhere".into()));
    req.env.push(("MiXeD".into(), "1".into()));
    let p = h.spawn(g, req).unwrap();
    h.hello(p);
    h.start(p, "test.echo", json!({ "value": 1, "report": ["env"] }));
    let env: std::collections::BTreeMap<String, String> =
        serde_json::from_value(h.result(p)["env"].clone()).unwrap();
    let keys: Vec<&str> = env.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["FIN_TEST_MARKER", "MiXeD", "Path", "TMPDIR"],
        "{env:?}"
    );
    assert_eq!(env["Path"], r"C:\nowhere");
}

fn oom_group(h: &TestHost) -> GroupId {
    h.host.open_group(GroupSpec {
        label: "oom".into(),
        memory_bytes: Some(64 << 20),
        qos: Qos::Normal,
    })
}

fn oom_run(port_stopped: bool) {
    let mut h = host_with(|o| o.port_stopped_for_test = port_stopped);
    let g = oom_group(&h);
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.oom", json!({ "mib": 256 }));
    let exit = h.exit(p);
    assert_eq!(
        exit.reason,
        ExitReason::Oom,
        "port stopped {port_stopped}: {exit:?}"
    );
    // The peak counts the commit that was refused, so it may exceed the limit.
    assert!(exit.memory_peak.is_some(), "{exit:?}");
    // Under the limit, a job runs.
    let g = oom_group(&h);
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.oom", json!({ "mib": 16 }));
    assert_eq!(h.result(p)["mib"], 16);
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
}

#[test]
fn job_memory_limit_is_oom_confirmed_by_violation_info() {
    oom_run(false);
    oom_run(true);
}

#[test]
fn ready_carries_the_group_memory() {
    let mut h = host_with(|o| o.spawner = rewrite(|cmd| cmd.args.push("--warm".into())));
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.echo", json!({ "value": 1 }));
    h.result(p);
    match h.next_of(p) {
        HostEvent::Message {
            msg: WorkerMsg::Ready {},
            memory,
            ..
        } => assert!(memory.is_some_and(|m| m > 0), "{memory:?}"),
        e => panic!("ready, got {e:?}"),
    }
}

fn priority_of(pid: u32) -> u32 {
    // SAFETY: opens a process by id for a query; closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    assert!(!h.is_null());
    // SAFETY: as above.
    let class = unsafe { GetPriorityClass(h) };
    // SAFETY: closed once.
    unsafe { CloseHandle(h) };
    class
}

#[test]
fn priority_class_applied() {
    let mut h = host();
    for (qos, want) in [
        (Qos::Normal, NORMAL_PRIORITY_CLASS),
        (Qos::Utility, BELOW_NORMAL_PRIORITY_CLASS),
        (Qos::Background, BELOW_NORMAL_PRIORITY_CLASS),
    ] {
        let g = h.group_qos(qos);
        let mut req = h.req(&["test"]);
        req.qos = qos;
        let p = h.spawn(g, req).unwrap();
        let pid = h.hello(p).pid;
        assert_eq!(priority_of(pid), want, "{qos:?}");
        h.host.close_group(g);
    }
}

#[test]
fn groups_and_leaves_are_released_and_renewed() {
    // 30 groups, each opened, used and closed (its leaf and group Jobs released), then a
    // shutdown: every one starts clean, and nothing counts a process of an earlier group.
    let mut h = host();
    for i in 0..30 {
        let (p, _) = h.run("test.echo", json!({ "value": i }));
        assert_eq!(h.exit(p).foreign, Some(0), "group {i}");
    }
    let (tx, rx) = mpsc::channel();
    h.host.shutdown(
        100,
        Box::new(move || {
            let _ = tx.send(());
        }),
    );
    rx.recv_timeout(WAIT).unwrap();
}
