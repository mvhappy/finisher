//! The paths a process holds open or mapped (feature `test-hooks`; the warm contract's check,
//! 5C-1 plan Decision 36, Task 11 review L3). macOS reads them with `proc_pidinfo`: the open
//! vnode descriptors (`PROC_PIDLISTFDS`, then `PROC_PIDFDVNODEPATHINFO` per descriptor) and
//! the file-backed regions (`PROC_PIDREGIONPATHINFO`, address by address). No process is
//! started. Any failure is an error, so the check fails rather than passes. Linux reads
//! `/proc` from TypeScript; Windows probes the directory.

use std::io;

/// The paths `pid` holds open or mapped (unsorted, may repeat).
#[cfg(target_os = "macos")]
pub fn held_paths(pid: u32) -> io::Result<Vec<String>> {
    macos::held_paths(pid)
}

/// Not implemented here: Linux reads `/proc` itself, Windows probes the directory.
#[cfg(not(target_os = "macos"))]
pub fn held_paths(_pid: u32) -> io::Result<Vec<String>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "held paths are read here on macOS only",
    ))
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_int, c_void};
    use std::io;
    use std::mem::size_of;

    const PROC_PIDFDVNODEPATHINFO: c_int = 2;
    const PROC_PIDREGIONPATHINFO: c_int = 8;

    /// `struct proc_fileinfo` (`<sys/proc_info.h>`).
    #[repr(C)]
    #[allow(dead_code)] // laid out for the kernel; not every field is read
    struct ProcFileInfo {
        fi_openflags: u32,
        fi_status: u32,
        fi_offset: i64,
        fi_type: i32,
        fi_guardflags: u32,
    }

    /// `struct vnode_fdinfowithpath`.
    #[repr(C)]
    #[allow(dead_code)] // laid out for the kernel; not every field is read
    struct VnodeFdInfoWithPath {
        pfi: ProcFileInfo,
        pvip: libc::vnode_info_path,
    }

    /// `struct proc_regioninfo`.
    #[repr(C)]
    #[allow(dead_code)] // laid out for the kernel; not every field is read
    struct ProcRegionInfo {
        pri_protection: u32,
        pri_max_protection: u32,
        pri_inheritance: u32,
        pri_flags: u32,
        pri_offset: u64,
        pri_behavior: u32,
        pri_user_wired_count: u32,
        pri_user_tag: u32,
        pri_pages_resident: u32,
        pri_pages_shared_now_private: u32,
        pri_pages_swapped_out: u32,
        pri_pages_dirtied: u32,
        pri_ref_count: u32,
        pri_shadow_depth: u32,
        pri_share_mode: u32,
        pri_private_pages_resident: u32,
        pri_shared_pages_resident: u32,
        pri_obj_id: u32,
        pri_depth: u32,
        pri_address: u64,
        pri_size: u64,
    }

    /// `struct proc_regionwithpathinfo`.
    #[repr(C)]
    #[allow(dead_code)] // laid out for the kernel; not every field is read
    struct ProcRegionWithPathInfo {
        prp_prinfo: ProcRegionInfo,
        prp_vip: libc::vnode_info_path,
    }

    // The kernel checks the buffer size against these exactly.
    const _: () = assert!(size_of::<VnodeFdInfoWithPath>() == 1200);
    const _: () = assert!(size_of::<ProcRegionWithPathInfo>() == 1272);

    fn path_of(p: &libc::vnode_info_path) -> Option<String> {
        let bytes: Vec<u8> = p.vip_path.iter().flatten().map(|&c| c as u8).collect();
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        (end > 0).then(|| String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    fn alive(pid: c_int) -> io::Result<()> {
        // SAFETY: signal 0 only checks that the process exists.
        if unsafe { libc::kill(pid, 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn fds(pid: c_int) -> io::Result<Vec<libc::proc_fdinfo>> {
        let item = size_of::<libc::proc_fdinfo>();
        // SAFETY: a null buffer asks for the size needed.
        let need =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if need <= 0 {
            return Err(io::Error::last_os_error());
        }
        let n = need as usize / item + 16;
        let mut list: Vec<libc::proc_fdinfo> = Vec::with_capacity(n);
        let bytes =
            c_int::try_from(n * item).map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        // SAFETY: `list` has room for `n` entries; the kernel writes at most `bytes`.
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                list.as_mut_ptr().cast::<c_void>(),
                bytes,
            )
        };
        if got <= 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the kernel initialised `got` bytes of whole entries.
        unsafe { list.set_len((got as usize / item).min(n)) };
        Ok(list)
    }

    pub fn held_paths(pid: u32) -> io::Result<Vec<String>> {
        let pid = c_int::try_from(pid).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        alive(pid)?;
        let mut out = Vec::new();
        for fd in fds(pid)? {
            if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 {
                continue;
            }
            let mut info = std::mem::MaybeUninit::<VnodeFdInfoWithPath>::zeroed();
            let size = size_of::<VnodeFdInfoWithPath>() as c_int;
            // SAFETY: `info` is `size` bytes; the kernel fills it or returns an error.
            let r = unsafe {
                libc::proc_pidfdinfo(
                    pid,
                    fd.proc_fd,
                    PROC_PIDFDVNODEPATHINFO,
                    info.as_mut_ptr().cast::<c_void>(),
                    size,
                )
            };
            if r == size {
                // SAFETY: filled by the kernel (a zeroed struct is valid too).
                let info = unsafe { info.assume_init() };
                out.extend(path_of(&info.pvip));
                continue;
            }
            let e = io::Error::last_os_error();
            // A descriptor closed since the listing.
            if e.raw_os_error() == Some(libc::EBADF) {
                continue;
            }
            return Err(e);
        }
        let mut address = 0u64;
        loop {
            let mut info = std::mem::MaybeUninit::<ProcRegionWithPathInfo>::zeroed();
            let size = size_of::<ProcRegionWithPathInfo>() as c_int;
            // SAFETY: as above; `address` is where the search for the next region starts.
            let r = unsafe {
                libc::proc_pidinfo(
                    pid,
                    PROC_PIDREGIONPATHINFO,
                    address,
                    info.as_mut_ptr().cast::<c_void>(),
                    size,
                )
            };
            if r <= 0 {
                // No region past `address` (the end), unless the process is gone.
                alive(pid)?;
                break;
            }
            // SAFETY: filled by the kernel.
            let info = unsafe { info.assume_init() };
            if info.prp_vip.vip_vi.vi_stat.vst_ino != 0 {
                out.extend(path_of(&info.prp_vip));
            }
            let next = info
                .prp_prinfo
                .pri_address
                .saturating_add(info.prp_prinfo.pri_size);
            if next <= address {
                break;
            }
            address = next;
        }
        Ok(out)
    }
}
