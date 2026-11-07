//! Windows-only hidden entries of the test binary, for the host's Job tests. None says
//! `hello`; each reports and exits.
//!
//! - `--test-job-report <file>`: writes the limits of the process's own (innermost) Job as
//!   JSON `{ui, limitFlags, activeLimit}`.
//! - `--test-breakaway`: tries to start a child with `CREATE_BREAKAWAY_FROM_JOB`; exits 10 if
//!   it is refused, 11 if the child was created (and kills it).
//! - `--test-poke-handles <value>…`: writes one byte to each handle value given, except its
//!   own standard handles, then exits 0. A handle it inherited would carry the byte.

use std::mem::{size_of, zeroed};
use std::process::{Command, Stdio};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::WriteFile;
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::JobObjects::{
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, QueryInformationJobObject,
};

/// One class of the calling process's innermost Job (a null Job handle).
fn own_job<T>(class: i32) -> Option<T> {
    // SAFETY: a plain C struct; zeros are valid.
    let mut out: T = unsafe { zeroed() };
    // SAFETY: `out` is the struct this class fills, with its size.
    let ok = unsafe {
        QueryInformationJobObject(
            std::ptr::null_mut(),
            class,
            (&raw mut out).cast(),
            size_of::<T>() as u32,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(out)
}

fn job_report(path: &str) -> i32 {
    let ui = own_job::<JOBOBJECT_BASIC_UI_RESTRICTIONS>(JobObjectBasicUIRestrictions);
    let ext = own_job::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(JobObjectExtendedLimitInformation);
    let report = serde_json::json!({
        "ui": ui.map(|u| u.UIRestrictionsClass),
        "limitFlags": ext.map(|e| e.BasicLimitInformation.LimitFlags),
        "activeLimit": ext.map(|e| e.BasicLimitInformation.ActiveProcessLimit),
    });
    match std::fs::write(path, report.to_string()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn breakaway() -> i32 {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB;
    let Ok(exe) = std::env::current_exe() else {
        return 1;
    };
    let child = Command::new(exe)
        .args(["--test-sleep", "60000"])
        .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Err(_) => 10,
        Ok(mut c) => {
            let _ = c.kill();
            11
        }
    }
}

fn poke(values: &[String]) -> i32 {
    // SAFETY: reads this process's standard handles.
    let own: Vec<HANDLE> = unsafe {
        [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .map(|s| GetStdHandle(s))
            .to_vec()
    };
    for v in values {
        let Ok(n) = v.parse::<usize>() else { continue };
        let h = n as HANDLE;
        if own.contains(&h) {
            continue;
        }
        let mut written = 0;
        // SAFETY: a write of one byte to whatever the value names; a value that names no
        // writable object fails, which is the point.
        unsafe { WriteFile(h, b"x".as_ptr(), 1, &mut written, std::ptr::null_mut()) };
    }
    0
}

/// The Windows hidden entries; `None` for anything else.
pub(super) fn hidden_main(args: &[String]) -> Option<i32> {
    match args.first().map(String::as_str)? {
        "--test-job-report" => Some(job_report(args.get(1)?)),
        "--test-breakaway" => Some(breakaway()),
        "--test-poke-handles" => Some(poke(&args[1..])),
        _ => None,
    }
}
