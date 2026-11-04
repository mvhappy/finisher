//! Per-OS gathering of the inputs for the pure classifiers. Every failure degrades towards
//! sync when a sync signal was seen and to `Network` otherwise, never to an error.

use std::path::{Path, PathBuf};

use super::StorageKind;

fn real_path(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// Canonical form of a configured root, or the raw value when it cannot be resolved.
fn canon(p: impl AsRef<Path>) -> PathBuf {
    let p = p.as_ref();
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The path as given (made absolute) and its canonical form: sync roots are often reached
/// through links, so both are checked.
#[cfg(unix)]
fn candidates(dir: &Path) -> Vec<PathBuf> {
    let mut v = vec![std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf())];
    let real = real_path(dir);
    if !v.contains(&real) {
        v.push(real);
    }
    v
}

#[cfg(unix)]
fn home_dirs() -> Vec<PathBuf> {
    use std::ffi::CStr;
    let mut homes: Vec<PathBuf> = std::env::var_os("HOME")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    // SAFETY: getpwuid_r fills `pwd` and `buf`; `pw_dir` is read only on success.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        if libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        ) == 0
            && !out.is_null()
            && !pwd.pw_dir.is_null()
        {
            homes.push(PathBuf::from(
                CStr::from_ptr(pwd.pw_dir).to_string_lossy().into_owned(),
            ));
        }
    }
    let canonical: Vec<PathBuf> = homes.iter().map(canon).collect();
    homes.extend(canonical);
    homes.sort();
    homes.dedup();
    homes
}

/// Well-known sync folders in a home directory and the roots in `~/.dropbox/info.json`.
#[cfg(unix)]
fn unix_path_provider(cands: &[PathBuf], fold: bool) -> Option<crate::conflict_names::Provider> {
    use super::{classify_home_folder, parse_dropbox_info, path_under};
    use crate::conflict_names::Provider;
    for home in home_dirs() {
        for c in cands {
            if let Some(p) = classify_home_folder(c, &home, fold) {
                return Some(p);
            }
        }
        if let Ok(json) = std::fs::read_to_string(home.join(".dropbox").join("info.json")) {
            for root in parse_dropbox_info(&json) {
                let roots = [PathBuf::from(&root), canon(&root)];
                if cands
                    .iter()
                    .any(|c| roots.iter().any(|r| path_under(c, r, fold)))
                {
                    return Some(Provider::Dropbox);
                }
            }
        }
    }
    None
}

#[cfg(unix)]
fn markers(real: &Path) -> super::SyncMarkers {
    super::find_markers(
        real,
        |p| std::fs::symlink_metadata(p).ok().map(|m| m.is_dir()),
        |dir| {
            std::fs::read_dir(dir)
                .map(|rd| {
                    rd.take(5000)
                        .filter_map(|e| e.ok()?.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default()
        },
    )
}

#[cfg(target_os = "linux")]
pub(super) fn probe(dir: &Path) -> StorageKind {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let cands = candidates(dir);
    let real = real_path(dir);
    let markers = markers(&real);
    if let Some(p) = markers.provider() {
        return StorageKind::Sync(p);
    }
    if let Some(p) = unix_path_provider(&cands, false)
        .or_else(|| cands.iter().find_map(|c| super::classify_wsl_path(c)))
    {
        return StorageKind::Sync(p);
    }
    let Ok(c) = CString::new(real.as_os_str().as_bytes()) else {
        return StorageKind::Network;
    };
    // SAFETY: `c` is a valid NUL-terminated path and `st` is a properly sized out-parameter.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return StorageKind::Network;
    }
    let f_type = (st.f_type as u64) & 0xFFFF_FFFF;
    let subtype = (f_type == super::FUSE_SUPER_MAGIC)
        .then(|| std::fs::read_to_string("/proc/self/mountinfo").ok())
        .flatten()
        .and_then(|info| super::mount_fstype(&info, &real));
    super::classify_linux_fs(f_type, markers, subtype.as_deref())
}

#[cfg(target_os = "macos")]
pub(super) fn probe(dir: &Path) -> StorageKind {
    use super::{classify_icloud_redirect, classify_macos_path, is_dataless};
    use crate::conflict_names::Provider;
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let cands = candidates(dir);
    let real = real_path(dir);
    if let Some(p) = cands.iter().find_map(|c| classify_macos_path(c)) {
        return StorageKind::Sync(p);
    }
    // "Desktop & Documents" are links out of `com~apple~CloudDocs`: resolve its entries.
    let mut targets = Vec::new();
    for home in home_dirs() {
        let docs = home.join("Library/Mobile Documents/com~apple~CloudDocs");
        let Ok(rd) = std::fs::read_dir(&docs) else {
            continue;
        };
        for e in rd.flatten() {
            let is_link = e.file_type().is_ok_and(|t| t.is_symlink());
            let name = e.file_name();
            if is_link || name == "Desktop" || name == "Documents" {
                targets.push(canon(e.path()));
            }
        }
    }
    if let Some(p) = cands
        .iter()
        .find_map(|c| classify_icloud_redirect(c, &targets))
    {
        return StorageKind::Sync(p);
    }
    if let Some(p) = unix_path_provider(&cands, true).or(markers(&real).provider()) {
        return StorageKind::Sync(p);
    }
    // A dataless (cloud-only) folder or ancestor: some File Provider owns it.
    for a in real.ancestors() {
        let Ok(c) = CString::new(a.as_os_str().as_bytes()) else {
            continue;
        };
        // SAFETY: valid NUL-terminated path; `st` is a properly sized out-parameter.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::lstat(c.as_ptr(), &mut st) } == 0 && is_dataless(st.st_flags) {
            return StorageKind::Sync(Provider::Unknown);
        }
    }
    let Ok(c) = CString::new(real.as_os_str().as_bytes()) else {
        return StorageKind::Network;
    };
    // SAFETY: as above; `f_fstypename` is NUL-terminated by the kernel.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return StorageKind::Network;
    }
    let name = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) };
    super::classify_macos_fs(&name.to_string_lossy())
}

#[cfg(windows)]
pub(super) fn probe(dir: &Path) -> StorageKind {
    use super::{WinFacts, classify_windows, is_unc, normalize_windows, parse_dropbox_info};
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::CloudFilters::{
        CF_SYNC_ROOT_INFO_STANDARD, CF_SYNC_ROOT_STANDARD_INFO, CfGetSyncRootInfoByPath,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetFileAttributesW, GetVolumeInformationW, GetVolumePathNameW,
        INVALID_FILE_ATTRIBUTES,
    };

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }
    fn from_wide(buf: &[u16]) -> String {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }
    /// Without the `\\?\` prefix (the Cloud Files and volume APIs want plain paths).
    fn plain(p: &Path) -> PathBuf {
        let s = p.to_string_lossy();
        match s.strip_prefix("\\\\?\\UNC\\") {
            Some(rest) => PathBuf::from(format!("\\\\{rest}")),
            None => PathBuf::from(s.strip_prefix("\\\\?\\").unwrap_or(&s).to_string()),
        }
    }

    let real = plain(&real_path(dir));
    let text = real.to_string_lossy().into_owned();
    let mut facts = WinFacts::default();

    // The volume that holds the folder (handles folder-mounted volumes too).
    if !is_unc(&normalize_windows(&text)) {
        let mut root = vec![0u16; 1024];
        let path_w = wide(real.as_os_str());
        // SAFETY: NUL-terminated input and a buffer of the stated length.
        if unsafe { GetVolumePathNameW(path_w.as_ptr(), root.as_mut_ptr(), root.len() as u32) } != 0
        {
            // SAFETY: `root` is NUL-terminated by the call above.
            facts.drive_type = Some(unsafe { GetDriveTypeW(root.as_ptr()) });
            let (mut label, mut fs) = (vec![0u16; 261], vec![0u16; 261]);
            // SAFETY: buffers of the stated lengths; the other out-parameters are optional.
            let ok = unsafe {
                GetVolumeInformationW(
                    root.as_ptr(),
                    label.as_mut_ptr(),
                    label.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    fs.as_mut_ptr(),
                    fs.len() as u32,
                )
            };
            if ok != 0 {
                facts.volume_label = Some(from_wide(&label));
                facts.volume_fs = Some(from_wide(&fs));
            }
        }
    }

    // Recall / pin attributes on the folder and every ancestor.
    for a in real.ancestors() {
        // SAFETY: NUL-terminated wide string.
        let attrs = unsafe { GetFileAttributesW(wide(a.as_os_str()).as_ptr()) };
        if attrs != INVALID_FILE_ATTRIBUTES {
            facts.attr_chain.push(attrs);
        }
    }

    // A registered Cloud Files sync root (OneDrive incl. SharePoint/Teams, Box, iCloud, ...).
    let mut info = vec![0u8; std::mem::size_of::<CF_SYNC_ROOT_STANDARD_INFO>() + 1024];
    let mut returned = 0u32;
    // SAFETY: NUL-terminated path; the buffer is at least one info struct long.
    let hr = unsafe {
        CfGetSyncRootInfoByPath(
            wide(real.as_os_str()).as_ptr(),
            CF_SYNC_ROOT_INFO_STANDARD,
            info.as_mut_ptr().cast(),
            info.len() as u32,
            &mut returned,
        )
    };
    if hr >= 0 {
        // SAFETY: success wrote a CF_SYNC_ROOT_STANDARD_INFO at the start of the buffer.
        let name = unsafe {
            let p = info.as_ptr().cast::<CF_SYNC_ROOT_STANDARD_INFO>();
            from_wide(&(*p).ProviderName)
        };
        facts.cloud_root = Some(name);
    }

    let env = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let canon_s = |s: String| canon(&s).to_string_lossy().into_owned();
    let plain_s = |s: String| plain(Path::new(&s)).to_string_lossy().into_owned();
    for var in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
        if let Some(v) = env(var) {
            facts.onedrive_roots.push(plain_s(canon_s(v.clone())));
            facts.onedrive_roots.push(v);
        }
    }
    facts.user_profile = env("USERPROFILE").map(|v| plain_s(canon_s(v)));
    facts.local_app_data = env("LOCALAPPDATA").map(|v| plain_s(canon_s(v)));
    for var in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(base) = env(var) {
            let info = Path::new(&base).join("Dropbox").join("info.json");
            if let Ok(json) = std::fs::read_to_string(info) {
                for root in parse_dropbox_info(&json) {
                    facts.dropbox_roots.push(plain_s(canon_s(root.clone())));
                    facts.dropbox_roots.push(root);
                }
            }
        }
    }
    classify_windows(&text, &facts)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) fn probe(_dir: &Path) -> StorageKind {
    StorageKind::Local
}
