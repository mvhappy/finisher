//! Windows volume identity (Decision 15), from an open directory handle only:
//!
//! - the **64-bit volume serial** (`FileIdInfo.VolumeSerialNumber`), plus for a network root
//!   (a UNC path or a mapped drive) **`\\server\share`**, lower-cased, from
//!   `GetFinalPathNameByHandleW`'s `\\?\UNC\` form (or the `\Device\Mup\` form when the DOS
//!   form is unavailable). The redirector's path never carries credentials (`net use`
//!   keeps them out of it); [`share_of`] refuses a server part with userinfo anyway;
//! - `kind`: the file system name (`GetVolumeInformationByHandleW`: `NTFS`, `ReFS`,
//!   `exFAT`, …); `remote`: `FileRemoteProtocolInfo` answers (only a redirector does);
//! - `mount_root`: the root is the root of its volume (`VOLUME_NAME_NONE` is `\`): a drive
//!   root, a share root, or a volume mounted in a folder. When such a root later opens on
//!   the folder's own volume (the mount was removed), it is `unmounted` (Decision 17);
//! - `case_sensitive`: the root directory's per-directory flag ([`win::case_sensitive`]).
//!
//! ## Step 0 findings (2026-11-08; plan 5D Task 3, written into the spec by Task 16)
//!
//! Windows 11, NTFS, an ssh session; `tests/{cfapi,source,open}_windows.rs` print (`STEP0`)
//! and assert them, `examples/{volume_identity,list_attrs}.rs` print the raw fields:
//!
//! - **Cloud Files** (our own provider: `CfRegisterSyncRoot` with full hydration and full
//!   population, `CfConnectSyncRoot` with `FETCH_DATA` and `FETCH_PLACEHOLDERS` counted and
//!   refused): a cloud-only file lists with tag `0x9000401a` (`IO_REPARSE_TAG_CLOUD_4`) and
//!   `RECALL_ON_DATA_ACCESS`; a hydrated in-sync file `0x9000601a` and no recall bit; a
//!   directory left for on-demand population `0x9000001a` with `OFFLINE |
//!   RECALL_ON_DATA_ACCESS` (not `RECALL_ON_OPEN`); an unpinned directory ("free up space")
//!   `UNPINNED` alone, fully listed (hence [`crate::placeholder::dir_from_attrs`]). The three
//!   classes report the same tags (`ReparsePointTag`, else `EaSize`). Full scans in every
//!   class, `stat_child`, `open_dir` of the placeholder directory and an opener pass over
//!   every file raise **zero** callbacks.
//! - **WOF** (`compact /c /exe:xpress4k`): the filter hides its tag from enumeration (the
//!   file lists as a plain file with its logical size) and serves the data through the
//!   opener. Data Deduplication is not installed on the test host (not measured).
//! - **8.3 names** are off on the data volume and on for the system volume: a short name
//!   never matches a listing, a lookup or an open (only the exact long name does).
//! - **Directory mtimes** in a parent's listing lag (see `source::windows`): read from the
//!   directory's own handle instead.
//! - **identity across a remount:** a `subst` drive removed is unreachable (`NOT_FOUND`);
//!   re-added it has the same key, root file id and sample, equal to the target directory's
//!   (the letter is not part of the identity). A drive root and a directory on it share the
//!   key; `VOLUME_NAME_NONE` is `\` only for the drive root. VHD attach needs elevation and
//!   `FIN_TEST_SHARE` (a mapped drive and a UNC path reconnected) was not set: both are left to
//!   Task 17.

use std::io;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_NAME_NORMALIZED, VOLUME_NAME_DOS, VOLUME_NAME_NONE, VOLUME_NAME_NT,
};

use super::key;
use crate::source::win;

/// What a root's identity is computed from (raw: never stored or journaled; only
/// [`Fields::key`] is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields {
    pub serial: u64,
    pub file_id: [u8; 16],
    /// `\\server\share`, lower-cased, for a network root.
    pub share: Option<String>,
    pub fs_name: String,
    pub remote: bool,
    pub volume_root: bool,
    pub case_sensitive: bool,
}

impl Fields {
    /// The volume key (Decision 15): the serial, plus the share for a network root.
    pub fn key(&self) -> String {
        let serial = self.serial.to_le_bytes();
        match &self.share {
            Some(s) => key(&[b"windows.share", &serial, s.as_bytes()]),
            None => key(&[b"windows.serial", &serial]),
        }
    }
}

/// `\\server\share` (lower-cased) of a final path: `\\?\UNC\server\share\…` or
/// `\Device\Mup\server\share\…`; `None` for a local path or a server part with userinfo.
pub fn share_of(final_path: &str) -> Option<String> {
    let rest = final_path
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| final_path.strip_prefix(r"\Device\Mup\"))?;
    let mut parts = rest.split('\\').filter(|p| !p.is_empty());
    let server = parts.next()?;
    let share = parts.next()?;
    // WebDAV writes `server@SSL`, `server@8080`, `server@SSL@8443`; any other `@` is userinfo.
    let webdav = |s: &str| {
        s.split_once('@').is_some_and(|(_, tail)| {
            tail.split('@').all(|p| {
                p.eq_ignore_ascii_case("ssl")
                    || (!p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
            })
        })
    };
    if server.contains('@') && !webdav(server) {
        return None;
    }
    Some(format!(r"\\{server}\{share}").to_lowercase())
}

/// The identity fields of an open directory.
pub(crate) fn fields(h: &impl AsRawHandle) -> io::Result<Fields> {
    let (serial, file_id) = win::file_id(h)?;
    let fs_name = win::fs_name(h).unwrap_or_default();
    let remote = win::remote_protocol(h).is_some();
    let path = |flags| win::final_path(h, flags).map(|w| String::from_utf16_lossy(&w));
    let share = if remote {
        path(FILE_NAME_NORMALIZED | VOLUME_NAME_DOS)
            .ok()
            .and_then(|p| share_of(&p))
            .or_else(|| {
                path(FILE_NAME_NORMALIZED | VOLUME_NAME_NT)
                    .ok()
                    .and_then(|p| share_of(&p))
            })
    } else {
        None
    };
    let volume_root =
        path(FILE_NAME_NORMALIZED | VOLUME_NAME_NONE).is_ok_and(|p| p == "\\" || p.is_empty());
    Ok(Fields {
        serial,
        file_id,
        share,
        fs_name,
        remote,
        volume_root,
        case_sensitive: win::case_sensitive(h),
    })
}

/// The identity fields of a root path (Step 0 measurements; `examples/volume_identity.rs`).
pub fn fields_of(root: &std::path::Path) -> io::Result<Fields> {
    fields(&win::open_root(root)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_from_final_paths() {
        assert_eq!(
            share_of(r"\\?\UNC\NAS\Media\x"),
            Some(r"\\nas\media".into())
        );
        assert_eq!(
            share_of(r"\Device\Mup\nas\media"),
            Some(r"\\nas\media".into())
        );
        assert_eq!(share_of(r"\\?\C:\x"), None);
        assert_eq!(share_of(r"\\?\UNC\nas"), None);
        assert_eq!(share_of(r"\\?\UNC\user@nas\m"), None);
        assert_eq!(
            share_of(r"\\?\UNC\dav@SSL\DavWWWRoot"),
            Some(r"\\dav@ssl\davwwwroot".into())
        );
        assert_eq!(
            share_of(r"\\?\UNC\dav@8080\x"),
            Some(r"\\dav@8080\x".into())
        );
    }

    #[test]
    fn key_depends_on_serial_and_share() {
        let f = Fields {
            serial: 7,
            file_id: [0; 16],
            share: None,
            fs_name: "NTFS".into(),
            remote: false,
            volume_root: true,
            case_sensitive: false,
        };
        let shared = Fields {
            share: Some(r"\\nas\media".into()),
            ..f.clone()
        };
        let other = Fields {
            serial: 8,
            ..f.clone()
        };
        assert_ne!(f.key(), shared.key());
        assert_ne!(f.key(), other.key());
        assert_eq!(
            f.key(),
            Fields {
                file_id: [1; 16],
                ..f.clone()
            }
            .key()
        );
    }
}
