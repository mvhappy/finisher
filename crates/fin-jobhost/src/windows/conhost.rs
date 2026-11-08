//! Console hosts are not foreign processes (Decision 22). A console program created with
//! `CREATE_NO_WINDOW` gets a hidden console, and its `conhost.exe` starts inside the program's
//! Jobs, so a Job's `TotalProcesses` counts it. The host recognises them (the system
//! directory's `conhost.exe`) from snapshots of a group's live processes, taken at each
//! process's `hello` (its console host starts during its initialisation, so it is there by
//! then) and at each count, and from the completion port's `NEW_PROCESS` hint, and leaves them
//! out of the count.

use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::OwnedHandle;
use std::path::PathBuf;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, MAX_PATH};
use windows_sys::Win32::System::JobObjects::{
    JobObjectBasicProcessIdList, QueryInformationJobObject,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

use super::raw;

fn console_host_path() -> Option<&'static str> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut buf = [0u16; MAX_PATH as usize];
        // SAFETY: `buf` holds MAX_PATH characters.
        let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
        (n > 0 && n < buf.len()).then(|| {
            let dir = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
            dir.join("conhost.exe").to_string_lossy().to_lowercase()
        })
    })
    .as_deref()
}

/// Whether `pid` runs the system's `conhost.exe`.
pub(crate) fn is_console_host(pid: u32) -> bool {
    let Some(want) = console_host_path() else {
        return false;
    };
    // SAFETY: opens a process by id for a query; closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return false;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` characters; the handle is ours.
    let ok =
        unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    // SAFETY: closed once.
    unsafe { CloseHandle(h) };
    ok != 0 && String::from_utf16_lossy(&buf[..len as usize]).to_lowercase() == want
}

/// The ids of the Job's live processes (the first 256).
pub(crate) fn live_pids(job: &OwnedHandle) -> Vec<u32> {
    #[repr(C)]
    struct List {
        assigned: u32,
        in_list: u32,
        ids: [usize; 256],
    }
    let mut list = List {
        assigned: 0,
        in_list: 0,
        ids: [0; 256],
    };
    // SAFETY: `list` has the layout of JOBOBJECT_BASIC_PROCESS_ID_LIST with room for 256 ids.
    let ok = unsafe {
        QueryInformationJobObject(
            raw(job),
            JobObjectBasicProcessIdList,
            (&raw mut list).cast(),
            size_of::<List>() as u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    let n = (list.in_list as usize).min(list.ids.len());
    list.ids[..n].iter().map(|&id| id as u32).collect()
}
