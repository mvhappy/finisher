//! Identities the OS gives files, directories and the machine, for telling copies and clones
//! apart (plan Decision 6, the server-data lock's restart shortcut).
//!
//! - [`dir_identity`]: a directory as one object on disk, whatever path reaches it: device and
//!   inode on Unix; on Windows the volume serial number and the 128-bit file id
//!   (`FileIdInfo`, ReFS) or else the 64-bit file index, of the directory opened with
//!   `FILE_FLAG_BACKUP_SEMANTICS`. A zero volume serial or file index (FAT/exFAT, some
//!   redirectors) is unknown (`None`). A hard-linked copy (`cp -al`, rsnapshot) has new
//!   directories, so it is never the same directory.
//! - [`machine_id`]: the OS's machine id: `/etc/machine-id` (or `/var/lib/dbus/machine-id`)
//!   on Linux, the host UUID (`gethostuuid`, the IOPlatformUUID) on macOS, `MachineGuid`
//!   under `HKLM\SOFTWARE\Microsoft\Cryptography` on Windows. `None` when it cannot be read.

use std::path::Path;

#[cfg(unix)]
pub(crate) fn dir_identity(p: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p).ok()?;
    (m.ino() != 0).then(|| format!("unix:{}:{}", m.dev(), m.ino()))
}

#[cfg(windows)]
pub(crate) fn dir_identity(p: &Path) -> Option<String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FileIdInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx,
    };
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(p)
        .ok()?;
    let h = f.as_raw_handle() as _;
    // SAFETY: an all-zero FILE_ID_INFO is a valid value of that plain struct.
    let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the call; `id` is writable and its size is passed.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            h,
            FileIdInfo,
            (&raw mut id).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok != 0 && id.VolumeSerialNumber != 0 && id.FileId.Identifier.iter().any(|b| *b != 0) {
        let hex: String = id
            .FileId
            .Identifier
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        return Some(format!("win:{}:{hex}", id.VolumeSerialNumber));
    }
    // SAFETY: as above, for the older call.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(h, &mut info) };
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    (ok != 0 && info.dwVolumeSerialNumber != 0 && index != 0)
        .then(|| format!("win:{}:{index:x}", info.dwVolumeSerialNumber))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn dir_identity(_p: &Path) -> Option<String> {
    None
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static MACHINE: std::cell::RefCell<Option<Option<String>>> = const { std::cell::RefCell::new(None) };
}

/// Test hook: on this thread, [`machine_id`] returns `id` (`None`: unreadable).
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn set_machine_id_for_test(id: Option<&str>) {
    MACHINE.with(|m| *m.borrow_mut() = Some(id.map(str::to_string)));
}

pub(crate) fn machine_id() -> Option<String> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(id) = MACHINE.with(|m| m.borrow().clone()) {
        return id;
    }
    os_machine_id().filter(|s| !s.is_empty())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn os_machine_id() -> Option<String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
}

#[cfg(target_os = "macos")]
fn os_machine_id() -> Option<String> {
    let mut uuid = [0u8; 16];
    let wait = libc::timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };
    // SAFETY: `uuid` is the 16 bytes the call fills; `wait` outlives the call.
    let rc = unsafe { libc::gethostuuid(uuid.as_mut_ptr(), &wait) };
    (rc == 0 && uuid.iter().any(|b| *b != 0))
        .then(|| uuid.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(windows)]
fn os_machine_id() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, value) = (
        wide(r"SOFTWARE\Microsoft\Cryptography"),
        wide("MachineGuid"),
    );
    let mut buf = [0u16; 128];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: the strings are NUL-terminated and `buf`/`len` describe a writable buffer.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return None;
    }
    let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
fn os_machine_id() -> Option<String> {
    None
}
