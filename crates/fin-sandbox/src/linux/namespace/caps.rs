//! What the namespace's capabilities must not outlive (plan 7C Decision 26). The launcher holds
//! every capability inside the user namespace it made; a program started as root (uid 0
//! mapped to itself) would keep them across `execve`, and Landlock does not refuse
//! `open_tree(OPEN_TREE_CLONE)`, which could copy a hidden tree back. So before `exec`:
//! nested user namespaces are limited to none (the per-namespace `max_user_namespaces`), then
//! every capability is dropped for good.

use crate::error::ApplyError;

const SECBIT_NOROOT: libc::c_ulong = 1 << 0;
const SECBIT_NOROOT_LOCKED: libc::c_ulong = 1 << 1;
const SECBIT_NO_SETUID_FIXUP: libc::c_ulong = 1 << 2;
const SECBIT_NO_SETUID_FIXUP_LOCKED: libc::c_ulong = 1 << 3;
const SECBIT_KEEP_CAPS_LOCKED: libc::c_ulong = 1 << 5;
const SECBIT_NO_CAP_AMBIENT_RAISE: libc::c_ulong = 1 << 6;
const SECBIT_NO_CAP_AMBIENT_RAISE_LOCKED: libc::c_ulong = 1 << 7;
const CAP_VERSION_3: u32 = 0x2008_0522;

fn os(call: &str) -> ApplyError {
    ApplyError::Os {
        call: call.into(),
        error: std::io::Error::last_os_error().to_string(),
    }
}

/// No user namespace below this one: limits the kernel surface the program can reach (and,
/// on Ubuntu, what the launcher's AppArmor profile would otherwise let it create).
pub fn no_nested_user_namespaces() -> Result<(), ApplyError> {
    std::fs::write("/proc/sys/user/max_user_namespaces", "0").map_err(|e| ApplyError::Os {
        call: "max_user_namespaces".into(),
        error: e.to_string(),
    })
}

/// Drops every capability (bounding, ambient, effective, permitted, inheritable) and locks the
/// securebits so `execve` of a root-owned or setuid program cannot regain any.
pub fn drop_all() -> Result<(), ApplyError> {
    let bits = SECBIT_NOROOT
        | SECBIT_NOROOT_LOCKED
        | SECBIT_NO_SETUID_FIXUP
        | SECBIT_NO_SETUID_FIXUP_LOCKED
        | SECBIT_KEEP_CAPS_LOCKED
        | SECBIT_NO_CAP_AMBIENT_RAISE
        | SECBIT_NO_CAP_AMBIENT_RAISE_LOCKED;
    // SAFETY: prctl with integer arguments only.
    unsafe {
        if libc::prctl(libc::PR_SET_SECUREBITS, bits, 0, 0, 0) != 0 {
            return Err(os("PR_SET_SECUREBITS"));
        }
        if libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        ) != 0
        {
            return Err(os("PR_CAP_AMBIENT_CLEAR_ALL"));
        }
        for cap in 0..64 {
            if libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) != 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
                    break; // past the kernel's last capability
                }
                return Err(os("PR_CAPBSET_DROP"));
            }
        }
        #[repr(C)]
        struct Header {
            version: u32,
            pid: i32,
        }
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        let mut h = Header {
            version: CAP_VERSION_3,
            pid: 0,
        };
        let d = [Data {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        }; 2];
        if libc::syscall(libc::SYS_capset, &raw mut h, d.as_ptr()) != 0 {
            return Err(os("capset"));
        }
    }
    Ok(())
}
