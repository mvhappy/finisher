//! The Windows spawn broker and the Job the host itself may run in (5C-1 plan, Task 5 review):
//! the broker is the bundled binary (or `cmd.exe`), no other process can open it, without one
//! nothing spawns; inside an outer Job with an active-process limit a full Job refuses as
//! `processLimit` (the broker's slot is taken at start), and a commit the outer Job's memory
//! limit refused is an `Oom`, not a crash. The outer-Job cases run in this test binary
//! re-executed (`outer_child`), which first puts itself into a Job with that limit.

// Tests drive the host and inspect job directories and process state by path; none is a
// library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(windows)]

mod host_support;

use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fin_jobhost::*;
use host_support::*;
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE, LUID,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, DuplicateTokenEx, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
    RevertToSelf, SE_DEBUG_NAME, SE_PRIVILEGE_REMOVED, SecurityImpersonation, TOKEN_ALL_ACCESS,
    TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenImpersonation,
};
use windows_sys::Win32::Storage::FileSystem::{READ_CONTROL, WRITE_DAC};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_JOB_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, OpenThread, PROCESS_DUP_HANDLE,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_OPERATION, PROCESS_VM_WRITE,
    SetThreadToken, THREAD_SUSPEND_RESUME,
};

const MODE: &str = "FIN_TEST_OUTER_CHILD";
const OUT: &str = "FIN_TEST_OUTER_OUT";

/// Should anything resume the broker, its entry does nothing and exits 0 at once.
#[test]
fn broker_entry_exits_at_once() {
    let t = Instant::now();
    let out = Command::new(BIN)
        .arg("--spawn-broker")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[test]
fn no_broker_refuses_spawns() {
    let (log, lines) = captured_log();
    let h = host_with(|o| {
        o.broker_images_for_test = Some(Vec::new());
        o.log = log;
    });
    assert!(h.host.broker_for_test().is_none());
    let g = h.group();
    let err = h.spawn(g, h.req(&["test"])).unwrap_err();
    assert_eq!(err.code(), "WORKER_UNAVAILABLE", "{err}");
    assert_eq!(err.reason(), Some("noBroker"), "{err}");
    assert!(
        lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("no spawn broker")),
        "{:?}",
        lines.lock().unwrap()
    );
    // Still none at the next spawn, still refused (no inheritable fallback).
    let err = h.spawn(g, h.req(&["test"])).unwrap_err();
    assert_eq!(err.reason(), Some("noBroker"), "{err}");
}

/// Every thread of `pid` (the broker has one, never resumed).
fn threads_of(pid: u32) -> Vec<u32> {
    // SAFETY: a snapshot of every thread; closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    // SAFETY: a plain C struct; zeros are valid.
    let mut e: THREADENTRY32 = unsafe { std::mem::zeroed() };
    e.dwSize = size_of::<THREADENTRY32>() as u32;
    let mut out = Vec::new();
    // SAFETY: the snapshot and a sized entry.
    let mut ok = unsafe { Thread32First(snap, &mut e) };
    while ok != 0 {
        if e.th32OwnerProcessID == pid {
            out.push(e.th32ThreadID);
        }
        // SAFETY: as above.
        ok = unsafe { Thread32Next(snap, &mut e) };
    }
    // SAFETY: closed once.
    unsafe { CloseHandle(snap) };
    out
}

/// Runs `f` on this thread as this user without `SeDebugPrivilege` (an impersonation token
/// with it removed): an elevated session (as over OpenSSH) has it enabled, and it opens any
/// process whatever its DACL. A normal desktop session has no such privilege.
fn without_debug_privilege<R>(f: impl FnOnce() -> R) -> R {
    let mut tok: HANDLE = std::ptr::null_mut();
    let mut imp: HANDLE = std::ptr::null_mut();
    // SAFETY: our own process token, duplicated as an impersonation token; both closed below.
    unsafe {
        assert_ne!(
            OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut tok),
            0
        );
        assert_ne!(
            DuplicateTokenEx(
                tok,
                TOKEN_ALL_ACCESS,
                std::ptr::null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut imp,
            ),
            0
        );
    }
    let mut luid: LUID = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    // SAFETY: a well-known privilege name and an out-pointer.
    assert_ne!(
        unsafe { LookupPrivilegeValueW(std::ptr::null(), SE_DEBUG_NAME, &mut luid) },
        0
    );
    let tp = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_REMOVED,
        }],
    };
    // SAFETY: the impersonation token we own and one privilege to remove; then this thread
    // impersonates it until `RevertToSelf`.
    unsafe {
        assert_ne!(
            AdjustTokenPrivileges(imp, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut()),
            0
        );
        assert_ne!(SetThreadToken(std::ptr::null(), imp), 0);
    }
    let r = f();
    // SAFETY: ends the impersonation; closes our two token handles once.
    unsafe {
        RevertToSelf();
        CloseHandle(imp);
        CloseHandle(tok);
    }
    r
}

/// Whether this thread can open `pid` with `access`; the error when not.
fn can_open(pid: u32, access: u32) -> Result<(), u32> {
    // SAFETY: an open by pid; a handle that opens is closed at once.
    let p = unsafe { OpenProcess(access, 0, pid) };
    // SAFETY: the error of the call just made.
    let err = unsafe { GetLastError() };
    if p.is_null() {
        return Err(err);
    }
    // SAFETY: closed once.
    unsafe { CloseHandle(p) };
    Ok(())
}

#[test]
fn broker_is_the_bundled_binary_and_closed_to_other_processes() {
    let mut h = host();
    let (pid, image) = h.host.broker_for_test().expect("a broker at start");
    assert_eq!(image, bin_dir().join("fin-workers.exe"));
    // It works as the parent.
    let (_, v) = h.run("test.echo", json!({ "value": 1 }));
    assert_eq!(v["value"], 1);
    assert!(!gone(pid), "never resumed, still there");
    // An ordinary process of ours opens as the same user would.
    let mut plain = Command::new(BIN)
        .args(["--test-sleep", "20000"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let threads = threads_of(pid);
    assert_eq!(threads.len(), 1, "{threads:?}");
    let (control, denied, thread) = without_debug_privilege(|| {
        let control = can_open(plain.id(), PROCESS_DUP_HANDLE | PROCESS_VM_WRITE);
        // The broker: not to copy handles out of it, write into it, query it, read or change
        // its DACL, or end it; nor its thread, to resume it.
        let denied: Vec<(&str, Result<(), u32>)> = [
            ("dup", PROCESS_DUP_HANDLE),
            ("write", PROCESS_VM_WRITE | PROCESS_VM_OPERATION),
            ("query", PROCESS_QUERY_LIMITED_INFORMATION),
            ("readControl", READ_CONTROL),
            ("writeDac", WRITE_DAC),
            ("terminate", PROCESS_TERMINATE),
        ]
        .into_iter()
        .map(|(name, access)| (name, can_open(pid, access)))
        .collect();
        // SAFETY: an open by id; a handle that opens is closed at once.
        let t = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, threads[0]) };
        if !t.is_null() {
            // SAFETY: closed once.
            unsafe { CloseHandle(t) };
        }
        (control, denied, t.is_null())
    });
    let _ = plain.kill();
    let _ = plain.wait();
    assert_eq!(control, Ok(()), "the same user opens its other processes");
    for (name, r) in denied {
        assert_eq!(r, Err(ERROR_ACCESS_DENIED), "{name}");
    }
    assert!(thread, "the broker's thread opened for resume");
}

#[test]
fn broker_falls_back_to_cmd() {
    let (log, lines) = captured_log();
    let cmd = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\cmd.exe");
    let missing = bin_dir().join("no-such-broker.exe");
    let mut h = host_with(|o| {
        o.broker_images_for_test = Some(vec![missing.clone(), cmd.clone()]);
        o.log = log;
    });
    let (_, image) = h.host.broker_for_test().expect("a broker");
    assert!(
        image
            .to_string_lossy()
            .eq_ignore_ascii_case(&cmd.to_string_lossy()),
        "{image:?}"
    );
    let (_, v) = h.run("test.echo", json!({ "value": 2 }));
    assert_eq!(v["value"], 2);
    assert!(
        lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("no-such-broker.exe")),
        "{:?}",
        lines.lock().unwrap()
    );
}

/// A Job with `f`'s limits that this process joins.
fn join_outer(
    f: impl FnOnce(&mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION, &OwnedHandle),
) -> OwnedHandle {
    // SAFETY: an unnamed Job; this process joins it.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(!job.is_null());
    // SAFETY: a fresh handle.
    let job = unsafe { OwnedHandle::from_raw_handle(job as _) };
    let raw = || std::os::windows::io::AsRawHandle::as_raw_handle(&job) as _;
    // SAFETY: our own process and the Job just made.
    assert_ne!(
        unsafe { AssignProcessToJobObject(raw(), GetCurrentProcess()) },
        0
    );
    // SAFETY: a plain C struct; zeros are valid.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the struct this class fills, with its size.
    assert_ne!(
        unsafe {
            QueryInformationJobObject(
                raw(),
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        },
        0
    );
    f(&mut info, &job);
    // SAFETY: the struct this class takes, with its size.
    assert_ne!(
        unsafe {
            SetInformationJobObject(
                raw(),
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        },
        0
    );
    job
}

/// The committed memory of the Job (`JobObjectMemoryUsageInformation`, class 28).
fn job_commit(job: &OwnedHandle) -> u64 {
    let mut m = [0u64; 2];
    // SAFETY: class 28 fills two u64s (current, peak).
    let ok = unsafe {
        QueryInformationJobObject(
            std::os::windows::io::AsRawHandle::as_raw_handle(job) as _,
            28,
            m.as_mut_ptr().cast(),
            16,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(ok, 0);
    m[0]
}

/// Spawns workers until one is refused (at most 6).
fn fill(h: &mut TestHost) -> Value {
    let g = h.group();
    let mut ok = 0;
    for _ in 0..6 {
        match h.spawn(g, h.req(&["test"])) {
            Ok(p) => {
                h.hello(p);
                ok += 1;
            }
            Err(e) => {
                return json!({ "ok": ok, "code": e.code(), "reason": e.reason(), "err": e.to_string() });
            }
        }
    }
    json!({ "ok": ok })
}

fn reason_of(exit: &ExitInfo) -> String {
    format!("{:?}", exit.reason)
}

/// The re-executed host for one outer-Job case.
#[test]
fn outer_child() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let out = PathBuf::from(std::env::var(OUT).unwrap());
    let report = if let Some(n) = mode.strip_prefix("active:") {
        let n: u32 = n.parse().unwrap();
        let _job = join_outer(|i, _| {
            i.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            i.BasicLimitInformation.ActiveProcessLimit = n;
        });
        let mut h = host();
        let broker = h.host.broker_for_test().is_some();
        let mut r = fill(&mut h);
        r["broker"] = json!(broker);
        r
    } else {
        assert_eq!(mode, "memory");
        let _job = join_outer(|i, job| {
            i.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
            i.JobMemoryLimit = (job_commit(job) + (384 << 20)) as usize;
        });
        let mut h = host();
        let g = h.group();
        let p = h.worker(g);
        h.hello(p);
        h.start(p, "test.oom", json!({ "mib": 1024 }));
        let oom = h.exit(p);
        let (p, v) = h.run("test.echo", json!({ "value": 3 }));
        let echo = (v["value"].clone(), reason_of(&h.exit(p)));
        let g = h.group();
        let p = h.worker(g);
        h.hello(p);
        h.start(
            p,
            "test.crash",
            json!({ "items": ["a"], "poison": ["a"], "how": "abort" }),
        );
        let crash = h.exit(p);
        json!({
            "oom": reason_of(&oom), "oomCode": oom.code,
            "echo": echo.0, "echoExit": echo.1,
            "crash": reason_of(&crash), "crashCode": crash.code,
        })
    };
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, report.to_string()).unwrap();
    std::fs::rename(&tmp, &out).unwrap();
}

fn run_child(mode: &str) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report.json");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "outer_child", "--nocapture", "--test-threads=1"])
        .env(MODE, mode)
        .env(OUT, &out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            if let Ok(bytes) = std::fs::read(&out) {
                return serde_json::from_slice(&bytes).unwrap();
            }
            let mut err = String::new();
            use std::io::Read;
            child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut err)
                .unwrap();
            panic!("{mode}: the child reported nothing ({status}): {err}");
        }
        assert!(Instant::now() < until, "{mode}: the child hung");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn outer_active_process_limit_refuses_as_process_limit() {
    // Only the host fits: no broker can be made.
    let r = run_child("active:1");
    assert_eq!(
        (&r["broker"], &r["ok"], &r["reason"]),
        (&json!(false), &json!(0), &json!("noBroker")),
        "{r}"
    );
    // The host and its broker (made at start, so its slot is taken first).
    let r = run_child("active:2");
    assert_eq!(
        (&r["broker"], &r["ok"], &r["reason"]),
        (&json!(true), &json!(0), &json!("processLimit")),
        "{r}"
    );
    assert_eq!(r["code"], "WORKER_UNAVAILABLE", "{r}");
    // Room for some workers, then a typed refusal.
    let r = run_child("active:6");
    assert_eq!(
        (&r["broker"], &r["reason"]),
        (&json!(true), &json!("processLimit")),
        "{r}"
    );
    assert!(r["ok"].as_u64().unwrap() >= 1, "{r}");
}

#[test]
fn outer_memory_limit_is_oom_not_a_crash() {
    let r = run_child("memory");
    assert_eq!(r["oom"], "Oom", "{r}");
    // Not sticky: a later job runs, and a later crash is a crash.
    assert_eq!(
        (&r["echo"], &r["echoExit"]),
        (&json!(3), &json!("Exited")),
        "{r}"
    );
    assert_eq!(r["crash"], "Exited", "{r}");
}
