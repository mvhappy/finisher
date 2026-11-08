//! Linux volume identity (Decision 15): the `/proc/self/mountinfo` line of the root's
//! `stx_mnt_id`, and `statfs.f_fsid`.
//!
//! - network filesystems and FUSE mounts (`nfs`, `nfs4`, `cifs`, `smb3`, `fuse.*`, …): the
//!   key is the mount source (userinfo stripped) plus the type: a remount gives a new device
//!   and mount id but the same source;
//! - local filesystems: `f_fsid` plus the type (ext4 and btrfs derive `f_fsid` from the
//!   filesystem UUID; tmpfs and xfs from the device number, which a remount may change: then
//!   the root's file id and the sample confirm the volume, Decision 17).

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};

use super::{key, linux_case_sensitive, strip_userinfo};

/// Network filesystem types (remote, keyed by source).
pub const NETWORK_TYPES: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "ceph",
    "9p",
    "afs",
    "glusterfs",
    "lustre",
    "davfs",
];

/// FUSE clients that talk to a server (remote); every `fuse.*` is keyed by source.
pub const REMOTE_FUSE: &[&str] = &[
    "fuse.sshfs",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.davfs2",
    "fuse.gcsfuse",
    "fuse.glusterfs",
];

/// One line of `/proc/self/mountinfo` (proc(5)); octal escapes undone in the paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    pub id: u64,
    pub parent: u64,
    /// `major:minor`.
    pub dev: String,
    /// The directory of the filesystem that forms this mount's root (a bind mount's source).
    pub root: Vec<u8>,
    pub mount_point: Vec<u8>,
    pub options: String,
    pub fstype: String,
    pub source: String,
    pub super_options: String,
}

/// `\040` → space and the other octal escapes the kernel writes (`\011`, `\012`, `\134`).
pub fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = b[i + 1..i + 4]
                .iter()
                .fold(0u32, |a, c| a * 8 + u32::from(c - b'0'));
            if let Ok(v) = u8::try_from(v) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Parses one mountinfo line; `None` for a malformed one.
pub fn parse_line(line: &str) -> Option<MountInfo> {
    let mut f = line.split(' ');
    let id = f.next()?.parse().ok()?;
    let parent = f.next()?.parse().ok()?;
    let dev = f.next()?.to_owned();
    let root = unescape(f.next()?);
    let mount_point = unescape(f.next()?);
    let options = f.next()?.to_owned();
    // Optional fields (`shared:N`, `master:N`, …) end at a lone `-`.
    for opt in f.by_ref() {
        if opt == "-" {
            break;
        }
    }
    let fstype = f.next()?.to_owned();
    let source = String::from_utf8_lossy(&unescape(f.next()?)).into_owned();
    let super_options = f.next().unwrap_or("").to_owned();
    Some(MountInfo {
        id,
        parent,
        dev,
        root,
        mount_point,
        options,
        fstype,
        source,
        super_options,
    })
}

/// `/proc/self/mountinfo`, opened by [`hold_mountinfo`] before a sandbox that does not grant
/// `/proc` is applied (the `worker` profile, plan 7C Task 2). Reading it again from offset 0
/// gives the live table (a `seq_file` regenerates it), so a held handle is never stale.
static HELD: std::sync::OnceLock<std::sync::Mutex<std::fs::File>> = std::sync::OnceLock::new();

/// Opens and keeps `/proc/self/mountinfo` for [`mountinfo`] (once; later calls do nothing).
/// `fin-workers` calls it before its self-sandbox entry.
pub fn hold_mountinfo() -> io::Result<()> {
    if HELD.get().is_none() {
        // `/proc` is never under a library root.
        #[allow(clippy::disallowed_methods)]
        let f = std::fs::File::open("/proc/self/mountinfo")?;
        let _ = HELD.set(std::sync::Mutex::new(f));
    }
    Ok(())
}

/// Every mount of this process's mount namespace: through the held handle when there is one.
pub fn mountinfo() -> io::Result<Vec<MountInfo>> {
    let text = match HELD.get() {
        Some(f) => {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = f.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            f.seek(SeekFrom::Start(0))?;
            let mut text = String::new();
            f.read_to_string(&mut text)?;
            text
        }
        // `/proc` is never under a library root.
        #[allow(clippy::disallowed_methods)]
        None => std::fs::read_to_string("/proc/self/mountinfo")?,
    };
    Ok(text.lines().filter_map(parse_line).collect())
}

/// What a root's identity is computed from (raw: never stored or journaled; only
/// [`Fields::key`] is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields {
    pub mount: MountInfo,
    pub mnt_id: u64,
    pub fsid: [u8; 8],
    pub dev: String,
    pub ino: u64,
}

impl Fields {
    /// Keyed by source (network and FUSE mounts), else by `f_fsid`.
    pub fn by_source(&self) -> bool {
        let t = self.mount.fstype.as_str();
        NETWORK_TYPES.contains(&t) || t.starts_with("fuse.")
    }

    pub fn remote(&self) -> bool {
        let t = self.mount.fstype.as_str();
        NETWORK_TYPES.contains(&t) || REMOTE_FUSE.contains(&t)
    }

    pub fn case_sensitive(&self) -> bool {
        let opts = format!("{},{}", self.mount.options, self.mount.super_options);
        linux_case_sensitive(&self.mount.fstype, &opts)
    }

    /// The volume key (Decision 15).
    pub fn key(&self) -> String {
        let t = self.mount.fstype.as_bytes();
        if self.by_source() {
            let src = strip_userinfo(&self.mount.source);
            key(&[b"linux.source", t, src.as_bytes()])
        } else {
            key(&[b"linux.fsid", t, &self.fsid])
        }
    }
}

/// The mount line of `mnt_id`, or (no mount id from the kernel) the last line whose device is
/// `dev`.
fn find(mounts: Vec<MountInfo>, mnt_id: Option<u64>, dev: &str) -> Option<MountInfo> {
    match mnt_id {
        Some(id) => mounts.into_iter().find(|m| m.id == id),
        None => mounts.into_iter().rev().find(|m| m.dev == dev),
    }
}

/// The identity fields of an open directory whose `statx` is `st`.
pub(crate) fn fields(fd: &OwnedFd, st: &libc::statx) -> io::Result<Fields> {
    let dev = crate::source::linux::dev_id(st);
    let mnt_id = (st.stx_mask & libc::STATX_MNT_ID != 0).then_some(st.stx_mnt_id);
    let mount = find(mountinfo()?, mnt_id, &dev)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no mountinfo line for the root"))?;
    let mut sfs = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: `sfs` is a valid out-pointer.
    crate::source::unix::retry(|| {
        crate::source::unix::cvt(unsafe { libc::fstatfs(fd.as_raw_fd(), sfs.as_mut_ptr()) })
    })?;
    // SAFETY: fstatfs succeeded and filled `sfs`.
    let sfs = unsafe { sfs.assume_init() };
    // SAFETY: `fsid_t` is two `c_int`s (8 bytes, private fields); read them as bytes.
    let fsid: [u8; 8] = unsafe { std::ptr::read((&raw const sfs.f_fsid).cast::<[u8; 8]>()) };
    Ok(Fields {
        mount,
        mnt_id: st.stx_mnt_id,
        fsid,
        dev,
        ino: st.stx_ino,
    })
}

/// The identity fields of a root path (Step 0 measurements; `examples/volume_identity.rs`).
pub fn fields_of(root: &std::path::Path) -> io::Result<Fields> {
    let fd = crate::source::unix::open_root(root)?;
    let st = crate::source::linux::statx_fd(fd.as_raw_fd())?;
    fields(&fd, &st)
}
