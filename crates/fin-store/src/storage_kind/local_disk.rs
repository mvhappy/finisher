//! Whether a directory is on a plain local disk filesystem, by an allow-list of filesystem
//! types: what the server-data lock's restart shortcut needs (an OS file lock on a network,
//! FUSE or sync filesystem proves nothing about other machines). Anything not listed, or a
//! probe that fails, is `false`.
//!
//! - Linux: ext2/3/4, XFS, Btrfs, ZFS, F2FS, bcachefs, tmpfs, ramfs, overlayfs;
//! - macOS: APFS, HFS+;
//! - Windows: NTFS, ReFS on a fixed drive.

use std::path::Path;

/// The Linux `statfs` magic numbers of the allow-list.
pub const LINUX_LOCAL: [u64; 9] = [
    0xEF53,      // ext2/3/4
    0x5846_5342, // xfs
    0x9123_683E, // btrfs
    0x2FC1_2FC1, // zfs
    0xF2F5_2010, // f2fs
    0xCA45_1A4E, // bcachefs
    0x0102_1994, // tmpfs
    0x8584_58F6, // ramfs
    0x794C_7630, // overlayfs
];

/// overlayfs (Docker's container filesystem): local to the restart shortcut, but it hides the
/// disk under it, so server data on it is not a *known* local filesystem (`crate::locality`).
pub const LINUX_OVERLAY: u64 = 0x794C_7630;

/// Pure part, Linux.
pub fn is_linux_local(f_type: u64) -> bool {
    LINUX_LOCAL.contains(&f_type)
}

/// The `statfs` filesystem type of `dir` (Linux), `None` when the probe fails.
#[cfg(target_os = "linux")]
pub fn linux_fs_type(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL-terminated path and `st` a properly sized out-parameter.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some((st.f_type as u64) & 0xFFFF_FFFF)
}

/// Pure part, macOS.
pub fn is_macos_local(fstypename: &str) -> bool {
    matches!(fstypename, "apfs" | "hfs")
}

/// Pure part, Windows (`fs_name` from `GetVolumeInformationW`, `fixed`: `DRIVE_FIXED`).
pub fn is_windows_local(fs_name: &str, fixed: bool) -> bool {
    fixed && (fs_name.eq_ignore_ascii_case("NTFS") || fs_name.eq_ignore_ascii_case("ReFS"))
}

#[cfg(target_os = "linux")]
pub fn is_local_disk(dir: &Path) -> bool {
    linux_fs_type(dir).is_some_and(is_linux_local)
}

#[cfg(target_os = "macos")]
pub fn is_local_disk(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: as above; `f_fstypename` is NUL-terminated by the kernel.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(st.f_fstypename.as_ptr()) };
    is_macos_local(&name.to_string_lossy())
}

#[cfg(windows)]
pub fn is_local_disk(dir: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetVolumeInformationW, GetVolumePathNameW,
    };
    const DRIVE_FIXED: u32 = 3;
    let Ok(abs) = std::path::absolute(dir) else {
        return false;
    };
    let path: Vec<u16> = abs.as_os_str().encode_wide().chain([0]).collect();
    let mut root = [0u16; 1024];
    // SAFETY: `path` is NUL-terminated; `root` is writable and its length is passed.
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return false;
    }
    // SAFETY: `root` is NUL-terminated by the call above.
    let fixed = unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_FIXED;
    let mut fs = [0u16; 64];
    // SAFETY: optional out-parameters are null; `fs` is writable and its length is passed.
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    };
    let n = fs.iter().position(|c| *c == 0).unwrap_or(fs.len());
    ok != 0 && is_windows_local(&String::from_utf16_lossy(&fs[..n]), fixed)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn is_local_disk(_dir: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list() {
        assert!(is_linux_local(0xEF53) && is_linux_local(0x0102_1994));
        assert!(!is_linux_local(0x6969), "nfs");
        assert!(!is_linux_local(0xFE53_4D42), "smb2");
        assert!(!is_linux_local(0x6573_5546), "fuse");
        assert!(!is_linux_local(0x0102_1997), "v9fs (WSL /mnt/c)");
        assert!(is_macos_local("apfs") && !is_macos_local("smbfs") && !is_macos_local("msdos"));
        assert!(is_windows_local("NTFS", true) && !is_windows_local("NTFS", false));
        assert!(!is_windows_local("FAT32", true));
    }
}
