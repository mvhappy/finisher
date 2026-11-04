//! Per-OS gathering of the inputs for the pure classifiers in the parent module. Every
//! failure degrades to `Network` (see the module docs), never to an error.

use std::path::{Path, PathBuf};

use super::StorageKind;

fn real_path(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn markers(real: &Path) -> super::SyncMarkers {
    super::find_markers(real, |p| std::fs::symlink_metadata(p).is_ok())
}

#[cfg(target_os = "linux")]
pub(super) fn probe(dir: &Path) -> StorageKind {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let real = real_path(dir);
    let markers = markers(&real);
    let Ok(c) = CString::new(real.as_os_str().as_bytes()) else {
        return fallback(markers);
    };
    // SAFETY: `c` is a valid NUL-terminated path and `st` is a properly sized out-parameter.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return fallback(markers);
    }
    super::classify_linux_fs((st.f_type as u64) & 0xFFFF_FFFF, markers)
}

#[cfg(target_os = "linux")]
fn fallback(markers: super::SyncMarkers) -> StorageKind {
    markers
        .provider()
        .map_or(StorageKind::Network, StorageKind::Sync)
}

#[cfg(target_os = "macos")]
pub(super) fn probe(dir: &Path) -> StorageKind {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let real = real_path(dir);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(p) = home.and_then(|h| super::classify_macos_path(&real, &h)) {
        return StorageKind::Sync(p);
    }
    if let Some(p) = markers(&real).provider() {
        return StorageKind::Sync(p);
    }
    let Ok(c) = CString::new(real.as_os_str().as_bytes()) else {
        return StorageKind::Network;
    };
    // SAFETY: as on Linux; `f_fstypename` is NUL-terminated by the kernel.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return StorageKind::Network;
    }
    let name = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) };
    super::classify_macos_fs(&name.to_string_lossy())
}

#[cfg(windows)]
pub(super) fn probe(dir: &Path) -> StorageKind {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
    };

    let real = real_path(dir);
    let text = real.to_string_lossy().into_owned();
    let wide = |s: &std::ffi::OsStr| -> Vec<u16> { s.encode_wide().chain(Some(0)).collect() };

    let norm = super::normalize_windows(&text);
    // `C:\` for a drive path; UNC paths are classified without a drive type.
    let drive_type =
        (!super::is_unc(&norm) && norm.len() >= 2 && norm.as_bytes()[1] == b':').then(|| {
            let root = format!("{}:\\", &text.trim_start_matches("\\\\?\\")[..1]);
            // SAFETY: NUL-terminated wide string.
            unsafe { GetDriveTypeW(wide(std::ffi::OsStr::new(&root)).as_ptr()) }
        });
    // SAFETY: NUL-terminated wide string.
    let a = unsafe { GetFileAttributesW(wide(real.as_os_str()).as_ptr()) };
    let attrs = (a != INVALID_FILE_ATTRIBUTES).then_some(a);

    let env_roots = |names: &[&str]| -> Vec<String> {
        names
            .iter()
            .filter_map(|n| std::env::var(n).ok())
            .filter(|v| !v.is_empty())
            .collect()
    };
    let onedrive = env_roots(&["OneDrive", "OneDriveCommercial", "OneDriveConsumer"]);
    let mut dropbox = Vec::new();
    for var in ["APPDATA", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(var) {
            let info = Path::new(&base).join("Dropbox").join("info.json");
            if let Ok(json) = std::fs::read_to_string(info) {
                dropbox.extend(super::parse_dropbox_info(&json));
            }
        }
    }
    let kind = super::classify_windows(&text, &onedrive, &dropbox, drive_type, attrs);
    if kind == StorageKind::Local && drive_type == Some(0) {
        // DRIVE_UNKNOWN: the drive type could not be determined.
        return StorageKind::Network;
    }
    kind
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) fn probe(_dir: &Path) -> StorageKind {
    StorageKind::Local
}
