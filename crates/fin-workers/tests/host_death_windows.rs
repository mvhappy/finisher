//! Host death on Windows (5C-1 plan, Task 5): the host is this test binary re-executed
//! (`child_host`, the fin-fonts crash-child pattern), killed with `TerminateProcess`. Its host
//! Job's only handle closes with it, so every process in its Jobs dies: a worker, the
//! grandchild `test.spawn` started, and a `test.hang` worker that reads nothing. With `outer`,
//! the host first puts itself into a Job of its own, as a terminal or Electron may, and the
//! nesting still holds.

#![cfg(windows)]

mod host_support;

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use host_support::*;
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

const MODE: &str = "FIN_TEST_HOST_CHILD";
const OUT: &str = "FIN_TEST_HOST_OUT";

fn in_job(pid: u32, job: &OwnedHandle) -> bool {
    // SAFETY: opens a process for a query; the handle is owned below.
    let p = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    assert!(!p.is_null());
    // SAFETY: a fresh handle.
    let p = unsafe { OwnedHandle::from_raw_handle(p as _) };
    let mut r = 0;
    // SAFETY: both handles are valid.
    unsafe {
        IsProcessInJob(
            p.as_raw_handle() as HANDLE,
            job.as_raw_handle() as HANDLE,
            &mut r,
        )
    };
    r != 0
}

/// The re-executed host: starts the processes, reports their pids and waits to be killed.
#[test]
fn child_host() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let out = PathBuf::from(std::env::var(OUT).unwrap());
    let outer = (mode == "outer").then(|| {
        // SAFETY: an unnamed Job with default limits; this process joins it.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        assert!(!job.is_null());
        // SAFETY: our own process and the Job just made.
        assert_ne!(
            unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) },
            0
        );
        // SAFETY: a fresh handle.
        unsafe { OwnedHandle::from_raw_handle(job as _) }
    });
    let mut h = host();
    let g1 = h.group();
    let p1 = h.worker(g1);
    h.hello(p1);
    h.start(
        p1,
        "test.spawn",
        json!({ "detach": "none", "out": "pids.json" }),
    );
    let pids = spawn_pids(&h, "pids.json");
    let g2 = h.group();
    let p2 = h.worker(g2);
    let hang = h.hello(p2).pid;
    h.start(p2, "test.hang", json!({}));
    std::thread::sleep(Duration::from_millis(200));
    let in_outer = outer.as_ref().map(|j| {
        [pids.worker, pids.grandchild, hang]
            .iter()
            .all(|&p| in_job(p, j))
    });
    let report = json!({
        "host": std::process::id(),
        "worker": pids.worker,
        "grandchild": pids.grandchild,
        "hang": hang,
        "inOuter": in_outer,
    });
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, report.to_string()).unwrap();
    std::fs::rename(&tmp, &out).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn read_report(path: &Path, child: &mut std::process::Child) -> Value {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(bytes) = std::fs::read(path) {
            return serde_json::from_slice(&bytes).unwrap();
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!("the host child exited early: {status}");
        }
        assert!(Instant::now() < until, "the host child reported nothing");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn host_killed(mode: &str) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report.json");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_host", "--nocapture", "--test-threads=1"])
        .env(MODE, mode)
        .env(OUT, &out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let r = read_report(&out, &mut child);
    let pid = |k: &str| r[k].as_u64().unwrap() as u32;
    let all = [pid("worker"), pid("grandchild"), pid("hang")];
    assert!(all.iter().all(|&p| !gone(p)), "{r}");
    if mode == "outer" {
        assert_eq!(r["inOuter"], true, "nested under the host's own Job: {r}");
    }
    // TerminateProcess: the host gets no chance to clean up.
    child.kill().unwrap();
    child.wait().unwrap();
    for (name, p) in ["worker", "grandchild", "hang"].iter().zip(all) {
        assert!(
            wait_gone(p),
            "{mode}: the {name} ({p}) outlived its host: {r}"
        );
    }
}

#[test]
fn grandchild_gone_after_host_terminate() {
    host_killed("plain");
}

#[test]
fn nesting_works_inside_an_outer_job() {
    host_killed("outer");
}
