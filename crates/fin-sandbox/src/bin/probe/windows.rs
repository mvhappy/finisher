//! The Windows probe (plan 7C Task 9): started by `fin-sandbox-exec` inside a container, it
//! reports how it was set up and uses what it was granted. It never tries to reach anything
//! it was not granted (owner decision 2026-11-08: denials are computed by the suite with
//! `AccessCheck` on the container's token, not attempted).
//!
//! ```text
//! fin-sandbox-probe token | wait | errormode | job | desktop | env <name> | read <file>
//!                   | write <file> <text> | mkdir <dir> | list <dir> | pipe <name>
//!                   | crash av|fastfail | handle <n> | spawn-wait <exe>
//! ```
//! Each op prints one JSON line. `wait` prints the `token` line, then waits for its stdin to
//! close (the suite reads its token meanwhile). Exit 0 when the op ran, 2 for a usage error.

use std::io::{Read, Write};
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use serde_json::{Value, json};
use windows_sys::Win32::System::Diagnostics::Debug::GetErrorMode;
use windows_sys::Win32::System::JobObjects::{
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, QueryInformationJobObject,
};

fn usage(msg: &str) -> ! {
    eprintln!("fin-sandbox-probe: {msg}");
    std::process::exit(2)
}

fn token() -> Value {
    match fin_sandbox::windows::token::current() {
        Ok(t) => json!({ "op": "token", "pid": std::process::id(), "token": t }),
        Err(e) => json!({ "op": "token", "error": e.to_string() }),
    }
}

fn io_result(op: &str, r: std::io::Result<Value>) -> Value {
    match r {
        Ok(v) => json!({ "op": op, "ok": v }),
        Err(e) => json!({ "op": op, "error": e.raw_os_error(), "message": e.to_string() }),
    }
}

/// The limits of the job this process is in (the innermost one).
fn job() -> Value {
    // SAFETY: plain C structs; zeros are their defaults. A null handle queries the job of the
    // calling process.
    unsafe {
        let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        let ok = QueryInformationJobObject(
            null_mut(),
            JobObjectExtendedLimitInformation,
            (&raw mut ext).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            null_mut(),
        );
        if ok == 0 {
            return json!({ "op": "job", "error": std::io::Error::last_os_error().raw_os_error() });
        }
        let mut ui: JOBOBJECT_BASIC_UI_RESTRICTIONS = zeroed();
        let ui_ok = QueryInformationJobObject(
            null_mut(),
            JobObjectBasicUIRestrictions,
            (&raw mut ui).cast(),
            size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
            null_mut(),
        );
        json!({
            "op": "job",
            "flags": ext.BasicLimitInformation.LimitFlags,
            "activeProcesses": ext.BasicLimitInformation.ActiveProcessLimit,
            "jobMemory": ext.JobMemoryLimit,
            "ui": if ui_ok == 0 { Value::Null } else { json!(ui.UIRestrictionsClass) },
        })
    }
}

/// The window station and desktop this process runs on.
fn desktop() -> Value {
    use windows_sys::Win32::System::StationsAndDesktops::{
        GetProcessWindowStation, GetThreadDesktop, GetUserObjectInformationW, UOI_NAME,
    };
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    let name = |h: windows_sys::Win32::Foundation::HANDLE| -> Option<String> {
        let mut buf = [0u16; 256];
        let mut n = 0u32;
        // SAFETY: a valid handle and a buffer of the size given.
        let ok =
            unsafe { GetUserObjectInformationW(h, UOI_NAME, buf.as_mut_ptr().cast(), 512, &mut n) };
        (ok != 0).then(|| {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..end])
        })
    };
    // SAFETY: handles of this process's own station and thread desktop (not to be closed).
    let (ws, desk) = unsafe {
        (
            GetProcessWindowStation(),
            GetThreadDesktop(GetCurrentThreadId()),
        )
    };
    json!({ "op": "desktop", "station": name(ws), "desktop": name(desk) })
}

/// Opens a pipe as `fin` does (`GENERIC_READ | FILE_WRITE_DATA`) and writes one line.
fn pipe(name: &str) -> std::io::Result<Value> {
    use std::os::windows::fs::OpenOptionsExt;
    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_WRITE_DATA: u32 = 0x2;
    let mut f = std::fs::OpenOptions::new()
        .access_mode(GENERIC_READ | FILE_WRITE_DATA)
        .open(name)?;
    f.write_all(b"hi\n")?;
    Ok(json!(true))
}

fn crash(how: &str) -> ! {
    match how {
        // SAFETY: a deliberate access violation (the suite's crash case).
        "av" => unsafe {
            std::ptr::null_mut::<u32>().write_volatile(1);
            unreachable!()
        },
        "fastfail" => std::process::abort(),
        _ => usage("crash av|fastfail"),
    }
}

pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let op = args
        .first()
        .map(String::as_str)
        .unwrap_or_else(|| usage("no op"));
    let a = |i: usize| -> &str {
        args.get(i)
            .map(String::as_str)
            .unwrap_or_else(|| usage(&format!("{op} needs {i} argument(s)")))
    };
    let line = match op {
        "token" => token(),
        "wait" => {
            println!("{}", token());
            let _ = std::io::stdout().flush();
            let mut sink = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut sink);
            json!({ "op": "wait", "ok": true })
        }
        // SAFETY: reads a process-wide flag.
        "errormode" => json!({ "op": op, "errorMode": unsafe { GetErrorMode() } }),
        "job" => job(),
        "desktop" => desktop(),
        "env" => {
            json!({ "op": op, "value": std::env::var_os(a(1)).map(|v| v.to_string_lossy().into_owned()) })
        }
        "read" => io_result(op, std::fs::read_to_string(a(1)).map(Value::from)),
        "write" => io_result(op, std::fs::write(a(1), a(2)).map(|()| json!(true))),
        "mkdir" => io_result(op, std::fs::create_dir(a(1)).map(|()| json!(true))),
        "list" => io_result(
            op,
            std::fs::read_dir(a(1)).map(|d| json!(d.filter_map(Result::ok).count())),
        ),
        "pipe" => io_result(op, pipe(a(1))),
        "crash" => crash(a(1)),
        // Starts `<exe> wait` with this process's standard handles and waits for it: the child
        // prints its own `token` line (with its pid).
        "spawn-wait" => {
            let st = std::process::Command::new(a(1)).arg("wait").status();
            json!({ "op": op, "ok": st.map(|s| s.success()).unwrap_or(false) })
        }
        // Whether handle value `n` is open in this process (only the standard handles may be
        // inherited).
        "handle" => {
            let n: usize = a(1).parse().unwrap_or_else(|_| usage("a handle value"));
            let mut flags = 0u32;
            // SAFETY: only queries a handle value's flags.
            let ok =
                unsafe { windows_sys::Win32::Foundation::GetHandleInformation(n as _, &mut flags) };
            json!({ "op": op, "open": ok != 0 })
        }
        _ => usage(&format!("unknown op {op:?}")),
    };
    println!("{line}");
}
