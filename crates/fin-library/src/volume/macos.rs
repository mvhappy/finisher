//! macOS volume identity (Decision 15): `fstatfs` of the root (`f_fstypename`,
//! `f_mntfromname`, `f_mntonname`, `f_flags`, `f_fsid`) and, for local volumes,
//! `ATTR_VOL_UUID` of the volume's mount point.
//!
//! - network volumes (`smbfs`, `nfs`, `afpfs`, `webdav`, or anything without `MNT_LOCAL`):
//!   the key is `f_mntfromname` with any `user[:pass]@` stripped, plus the type;
//! - local volumes: the volume UUID; a volume without one (some `msdos` and `exfat` media)
//!   falls back to `f_fsid` plus the type.
//!
//! Case sensitivity: `fpathconf(_PC_CASE_SENSITIVE)`, no write; unknown (`-1`, some network
//! servers) counts as insensitive, which only folds more keys together (collisions are
//! flagged, never lost).

use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, OwnedFd};

use libc::c_void;

use super::{key, strip_userinfo};
use crate::source::unix::{cvt, retry};

/// Network filesystem types (keyed by source).
pub const NETWORK_TYPES: &[&str] = &["smbfs", "nfs", "afpfs", "webdav", "ftp", "cifs"];

/// What a root's identity is computed from (raw: only [`Fields::key`] is stored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields {
    pub fstype: String,
    pub mntfrom: String,
    pub mnton: Vec<u8>,
    pub local: bool,
    pub uuid: Option<[u8; 16]>,
    pub fsid: [u8; 8],
    pub case_sensitive: bool,
}

impl Fields {
    pub fn remote(&self) -> bool {
        !self.local || NETWORK_TYPES.contains(&self.fstype.as_str())
    }

    /// The volume key (Decision 15).
    pub fn key(&self) -> String {
        let t = self.fstype.as_bytes();
        if self.remote() {
            let src = strip_userinfo(&self.mntfrom);
            key(&[b"macos.source", t, src.as_bytes()])
        } else if let Some(u) = self.uuid {
            key(&[b"macos.uuid", &u])
        } else {
            key(&[b"macos.fsid", t, &self.fsid])
        }
    }
}

fn c_chars(v: &[libc::c_char]) -> Vec<u8> {
    let b: Vec<u8> = v.iter().map(|&c| c as u8).collect();
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].to_vec()
}

/// `ATTR_VOL_UUID` of the volume mounted at `mnton` (`None` when the volume has none).
fn volume_uuid(mnton: &[u8]) -> Option<[u8; 16]> {
    let path = std::ffi::CString::new(mnton).ok()?;
    // SAFETY: plain data, all-zero valid.
    let mut al: libc::attrlist = unsafe { MaybeUninit::zeroed().assume_init() };
    al.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    al.volattr = libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID;
    #[repr(C)]
    struct Buf {
        len: u32,
        uuid: [u8; 16],
    }
    let mut buf = Buf {
        len: 0,
        uuid: [0; 16],
    };
    // SAFETY: `al` and `buf` are valid for the sizes passed; the path is a mount point (a
    // system path), resolved without following a final link.
    let r = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&raw mut al).cast::<c_void>(),
            (&raw mut buf).cast::<c_void>(),
            std::mem::size_of::<Buf>(),
            libc::FSOPT_NOFOLLOW,
        )
    };
    (r == 0 && buf.uuid != [0; 16]).then_some(buf.uuid)
}

/// The identity fields of an open directory.
pub(crate) fn fields(fd: &OwnedFd) -> io::Result<Fields> {
    let mut sfs = MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: `sfs` is a valid out-pointer.
    retry(|| cvt(unsafe { libc::fstatfs(fd.as_raw_fd(), sfs.as_mut_ptr()) }))?;
    // SAFETY: fstatfs succeeded.
    let sfs = unsafe { sfs.assume_init() };
    let fstype = String::from_utf8_lossy(&c_chars(&sfs.f_fstypename)).into_owned();
    let mntfrom = String::from_utf8_lossy(&c_chars(&sfs.f_mntfromname)).into_owned();
    let mnton = c_chars(&sfs.f_mntonname);
    let local = sfs.f_flags & (libc::MNT_LOCAL as u32) != 0;
    // SAFETY: `fsid_t` is two `i32`s (private fields); read them as bytes.
    let fsid: [u8; 8] = unsafe { std::ptr::read((&raw const sfs.f_fsid).cast::<[u8; 8]>()) };
    // SAFETY: a plain libc call on our descriptor.
    let cs = unsafe { libc::fpathconf(fd.as_raw_fd(), libc::_PC_CASE_SENSITIVE) };
    let uuid = if local { volume_uuid(&mnton) } else { None };
    Ok(Fields {
        fstype,
        mntfrom,
        mnton,
        local,
        uuid,
        fsid,
        case_sensitive: cs == 1,
    })
}

/// The identity fields of a root path (Step 0 measurements; `examples/volume_identity.rs`).
pub fn fields_of(root: &std::path::Path) -> io::Result<Fields> {
    let fd = crate::source::unix::open_root(root)?;
    fields(&fd)
}
