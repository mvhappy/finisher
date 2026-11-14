//! Allow-only ACEs on files and directories (plan 7C Decision 18): the primitives the ACL
//! broker (`fin-node`, Task 10) records in its write-ahead log before applying, and the
//! escape suites use to set up their trees. A grant adds one explicit allow ACE for a SID
//! (merged with the DACL's other entries by `SetEntriesInAclW`); a revoke removes every
//! explicit allow ACE of that SID and leaves inherited ones (which go with their parent's).

use std::io;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree, WIN32_ERROR};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, EXPLICIT_ACCESS_W, GRANT_ACCESS,
    GetNamedSecurityInfoW, GetSecurityInfo, NO_MULTIPLE_TRUSTEE, SDDL_REVISION_1, SE_FILE_OBJECT,
    SE_WINDOW_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, SetSecurityInfo, TRUSTEE_IS_SID,
    TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, AddAce, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
    INHERITED_ACE, InitializeAcl, LABEL_SECURITY_INFORMATION, NO_INHERITANCE,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};

use super::sid::{Sid, from_wide, wide};

pub use super::grants::{MODIFY, READ, READ_EXECUTE};

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

struct Local<T>(*mut T);

impl<T> Drop for Local<T> {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: LocalAlloc'd by the call that returned it, freed once.
            unsafe { LocalFree(self.0 as HLOCAL) };
        }
    }
}

fn win(call: &str, e: WIN32_ERROR) -> io::Result<()> {
    if e == ERROR_SUCCESS {
        Ok(())
    } else {
        let os = io::Error::from_raw_os_error(e as i32);
        Err(io::Error::new(os.kind(), format!("{call}: {os}")))
    }
}

/// The DACL of `path` and the descriptor that holds it.
fn dacl_of(path: &[u16]) -> io::Result<(*mut ACL, Local<std::ffi::c_void>)> {
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: a NUL-terminated path and valid out-pointers.
    let e = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        )
    };
    win("GetNamedSecurityInfoW", e)?;
    Ok((dacl, Local(sd)))
}

fn set_dacl(path: &[u16], dacl: *const ACL) -> io::Result<()> {
    // SAFETY: a NUL-terminated path and a valid DACL; only the DACL changes (its protection
    // flag is kept, inherited entries are recomputed by the system).
    let e = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            dacl,
            null(),
        )
    };
    win("SetNamedSecurityInfoW", e)
}

/// Adds an allow ACE for `sid` on `path` with `mask`; `inherit` makes it apply to the subtree.
pub fn grant(path: &Path, sid: &Sid, mask: u32, inherit: bool) -> io::Result<()> {
    let p = wide(path);
    let (dacl, _sd) = dacl_of(&p)?;
    let ea = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: if inherit {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        },
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            ptstrName: sid.as_psid().cast(),
        },
    };
    let mut new = Local(null_mut::<ACL>());
    // SAFETY: one entry, the current DACL (or null), a valid out-pointer.
    win("SetEntriesInAclW", unsafe {
        SetEntriesInAclW(1, &ea, dacl, &mut new.0)
    })?;
    set_dacl(&p, new.0)
}

/// Removes every explicit allow ACE of `sid` on `path`; the number removed.
pub fn revoke(path: &Path, sid: &Sid) -> io::Result<usize> {
    let p = wide(path);
    let (dacl, _sd) = dacl_of(&p)?;
    if dacl.is_null() {
        return Ok(0);
    }
    // SAFETY: `dacl` is the valid ACL the descriptor holds; each ACE is read in bounds.
    unsafe {
        let size = usize::from((*dacl).AclSize).max(64);
        let mut buf = vec![0u64; size.div_ceil(8)];
        let new = buf.as_mut_ptr().cast::<ACL>();
        if InitializeAcl(new, size as u32, u32::from((*dacl).AclRevision)) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut removed = 0;
        for i in 0..u32::from((*dacl).AceCount) {
            let mut ace = null_mut();
            if GetAce(dacl, i, &mut ace) == 0 {
                return Err(io::Error::last_os_error());
            }
            let h = ace.cast::<ACE_HEADER>();
            let explicit = u32::from((*h).AceFlags) & INHERITED_ACE == 0;
            let ours = explicit
                && (*h).AceType == ACCESS_ALLOWED_ACE_TYPE
                && EqualSid(
                    (&raw mut (*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart)
                        .cast::<std::ffi::c_void>() as PSID,
                    sid.as_psid(),
                ) != 0;
            if ours {
                removed += 1;
            } else if AddAce(
                new,
                u32::from((*new).AclRevision),
                u32::MAX,
                ace,
                u32::from((*h).AceSize),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        if removed > 0 {
            set_dacl(&p, new)?;
        }
        Ok(removed)
    }
}

const ACCESS_DENIED_ACE_TYPE: u8 = 1;

/// Whether the DACL of `path` already gives `sid` (by an explicit ACE) or every AppContainer
/// (`ALL APPLICATION PACKAGES`) `mask`, and its subtree too when `inherit`
/// ([`super::dacl::covers`]). Reads only (`READ_CONTROL`).
pub fn admits(path: &Path, sid: &Sid, mask: u32, inherit: bool) -> io::Result<bool> {
    use super::dacl::{ALL_APPLICATION_PACKAGES, Ace, Who, covers};
    let all = Sid::parse(ALL_APPLICATION_PACKAGES)?;
    let p = wide(path);
    let (dacl, _sd) = dacl_of(&p)?;
    if dacl.is_null() {
        // A null DACL admits everyone.
        return Ok(true);
    }
    let mut aces = Vec::new();
    // SAFETY: `dacl` is the valid ACL the descriptor holds; each ACE is read in bounds.
    unsafe {
        for i in 0..u32::from((*dacl).AceCount) {
            let mut ace = null_mut();
            if GetAce(dacl, i, &mut ace) == 0 {
                return Err(io::Error::last_os_error());
            }
            let h = ace.cast::<ACE_HEADER>();
            let kind = (*h).AceType;
            if kind != ACCESS_ALLOWED_ACE_TYPE && kind != ACCESS_DENIED_ACE_TYPE {
                continue;
            }
            // Allowed and denied ACEs share one layout (header, mask, SID).
            let body = ace.cast::<ACCESS_ALLOWED_ACE>();
            let who_sid = (&raw mut (*body).SidStart).cast::<std::ffi::c_void>() as PSID;
            let who = if EqualSid(who_sid, all.as_psid()) != 0 {
                Who::AllPackages
            } else if EqualSid(who_sid, sid.as_psid()) != 0 {
                Who::Grantee
            } else {
                Who::Other
            };
            aces.push(Ace {
                allow: kind == ACCESS_ALLOWED_ACE_TYPE,
                flags: (*h).AceFlags,
                who,
                mask: (*body).Mask,
            });
        }
    }
    Ok(covers(&aces, mask, inherit))
}

/// The owner, DACL and label of `path` as SDDL (tests, the broker's read-back).
pub fn sddl(path: &Path) -> io::Result<String> {
    let p = wide(path);
    let si = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION;
    let mut sd = Local(null_mut::<std::ffi::c_void>());
    // SAFETY: a NUL-terminated path and a valid out-pointer.
    let e = unsafe {
        GetNamedSecurityInfoW(
            p.as_ptr(),
            SE_FILE_OBJECT,
            si,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd.0,
        )
    };
    win("GetNamedSecurityInfoW", e)?;
    let mut s = Local(null_mut::<u16>());
    // SAFETY: the descriptor just read and a valid out-pointer.
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd.0,
            SDDL_REVISION_1,
            si,
            &mut s.0,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a NUL-terminated string the call returned.
    Ok(unsafe { from_wide(s.0) })
}

/// Replaces the owner and DACL of `path` with those of `sddl` (protected when it says `D:P`):
/// the suites' owner-only server data, as `private_dir` makes it.
pub fn set_sddl(path: &Path, sddl: &str) -> io::Result<()> {
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
        PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    let mut sd = Local(null_mut::<std::ffi::c_void>());
    // SAFETY: a NUL-terminated string and a valid out-pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide(sddl).as_ptr(),
            SDDL_REVISION_1,
            &mut sd.0,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor just made; out-pointers valid.
    unsafe {
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = null_mut();
        GetSecurityDescriptorDacl(sd.0, &mut present, &mut dacl, &mut defaulted);
        let mut owner: PSID = null_mut();
        GetSecurityDescriptorOwner(sd.0, &mut owner, &mut defaulted);
        let (mut control, mut rev) = (0u16, 0u32);
        GetSecurityDescriptorControl(sd.0, &mut control, &mut rev);
        let protection = if control & SE_DACL_PROTECTED != 0 {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        let mut si = DACL_SECURITY_INFORMATION | protection;
        if !owner.is_null() {
            si |= OWNER_SECURITY_INFORMATION;
        }
        let p = wide(path);
        win(
            "SetNamedSecurityInfoW",
            SetNamedSecurityInfoW(
                p.as_ptr(),
                SE_FILE_OBJECT,
                si,
                owner,
                null_mut(),
                dacl,
                null(),
            ),
        )
    }
}

/// Adds an allow ACE for `sid` with `mask` to the DACL of a kernel or USER object (a window
/// station); the handle needs `READ_CONTROL` and `WRITE_DAC`.
pub(crate) fn grant_object(h: HANDLE, sid: &Sid, mask: u32) -> io::Result<()> {
    let mut dacl: *mut ACL = null_mut();
    let mut sd = Local(null_mut::<std::ffi::c_void>());
    // SAFETY: a valid handle and out-pointers.
    win("GetSecurityInfo", unsafe {
        GetSecurityInfo(
            h,
            SE_WINDOW_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd.0,
        )
    })?;
    let ea = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            ptstrName: sid.as_psid().cast(),
        },
    };
    let mut new = Local(null_mut::<ACL>());
    // SAFETY: one entry, the current DACL, a valid out-pointer.
    win("SetEntriesInAclW", unsafe {
        SetEntriesInAclW(1, &ea, dacl, &mut new.0)
    })?;
    // SAFETY: a valid handle and DACL.
    win("SetSecurityInfo", unsafe {
        SetSecurityInfo(
            h,
            SE_WINDOW_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            new.0,
            null(),
        )
    })
}

/// The DACL of a window station or desktop as SDDL.
pub(crate) fn object_sddl(h: HANDLE) -> io::Result<String> {
    let mut sd = Local(null_mut::<std::ffi::c_void>());
    // SAFETY: a valid handle and out-pointers.
    win("GetSecurityInfo", unsafe {
        GetSecurityInfo(
            h,
            SE_WINDOW_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd.0,
        )
    })?;
    descriptor_sddl(sd.0)
}

/// The DACL of a descriptor (which the caller keeps owning) as SDDL.
pub(crate) fn descriptor_sddl(sd: PSECURITY_DESCRIPTOR) -> io::Result<String> {
    let mut s = Local(null_mut::<u16>());
    // SAFETY: a valid descriptor and a valid out-pointer.
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut s.0,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a NUL-terminated string the call returned.
    Ok(unsafe { from_wide(s.0) })
}
