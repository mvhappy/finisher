//! Windows half of [`crate::private`]: protected DACLs set at creation, and the DACL check.
//! The module docs of `private` give the descriptors and the rules.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::{Kind, Unprotected};

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const ACCESS_DENIED_OBJECT_ACE_TYPE: u8 = 6;
const ACCESS_ALLOWED_CALLBACK_ACE_TYPE: u8 = 9;
const ACCESS_DENIED_CALLBACK_ACE_TYPE: u8 = 10;
const ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE: u8 = 12;
const INHERITED_ACE: u8 = 0x10;
const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// `CREATOR OWNER` and `OWNER RIGHTS`: placeholders for the owner, who is checked.
const OWNER_ALIASES: [&str; 2] = ["S-1-3-0", "S-1-3-4"];

/// `path` as a NUL-terminated verbatim path (`\\?\C:\…`, `\\?\UNC\host\share\…`), made
/// absolute the way `std` does, so long paths work as they do for `std::fs`.
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let abs = std::path::absolute(path)?;
    let s: Vec<u16> = abs.as_os_str().encode_wide().collect();
    let bs = u16::from(b'\\');
    let mut out: Vec<u16> = if s.starts_with(&[bs, bs]) {
        let verbatim = s.get(2) == Some(&u16::from(b'?')) || s.get(2) == Some(&u16::from(b'.'));
        if verbatim {
            s
        } else {
            r"\\?\UNC\"
                .encode_utf16()
                .chain(s[2..].iter().copied())
                .collect()
        }
    } else {
        r"\\?\".encode_utf16().chain(s).collect()
    };
    out.push(0);
    Ok(out)
}

/// Frees a `LocalAlloc`ed pointer on drop.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from an API documented to `LocalAlloc` it.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// A SID as `S-1-…`.
fn sid_string(sid: PSID) -> Option<String> {
    let mut out: *mut u16 = null_mut();
    // SAFETY: `sid` is a valid SID owned by the caller; `out` receives a LocalAlloc'ed string.
    if unsafe { ConvertSidToStringSidW(sid, &mut out) } == 0 {
        return None;
    }
    let _free = Local(out.cast());
    let mut len = 0;
    // SAFETY: `out` is NUL-terminated.
    while unsafe { *out.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` u16s are readable at `out`.
    Some(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(out, len)
    }))
}

/// The current process token's user SID.
pub(crate) fn current_user() -> io::Result<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: plain FFI with valid out-pointers.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buf = vec![0u8; 256];
    let mut len = 0u32;
    // SAFETY: `buf` has `buf.len()` writable bytes; `token` is open.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
            &mut len,
        )
    };
    let err = io::Error::last_os_error();
    // SAFETY: `token` is open.
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err(err);
    }
    // SAFETY: GetTokenInformation(TokenUser) filled a TOKEN_USER at the start of `buf`.
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    sid_string(user.User.Sid).ok_or_else(io::Error::last_os_error)
}

/// The owner-only descriptor for a new folder (inherited by its children) or file.
fn descriptor(kind: Kind) -> io::Result<Local> {
    let u = current_user()?;
    let inherit = match kind {
        Kind::Folder => "OICI",
        Kind::File => "",
    };
    let sddl: Vec<u16> = format!("O:{u}D:P(A;{inherit};FA;;;{u})(A;{inherit};FA;;;{SYSTEM})")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: NUL-terminated SDDL; `sd` receives a LocalAlloc'ed descriptor.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Local(sd))
}

fn attributes(sd: &Local) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    }
}

pub fn create_dir(path: &Path) -> io::Result<()> {
    let sd = descriptor(Kind::Folder)?;
    let sa = attributes(&sd);
    let name = wide(path)?;
    // SAFETY: NUL-terminated name; `sa` and the descriptor it points to outlive the call.
    if unsafe { CreateDirectoryW(name.as_ptr(), &sa) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn create_new(path: &Path) -> io::Result<File> {
    let sd = descriptor(Kind::File)?;
    let sa = attributes(&sd);
    let name = wide(path)?;
    // SAFETY: NUL-terminated name; `sa` and its descriptor outlive the call. The share mode is
    // `std`'s, so replacing the file later works as it does for files `std` opened.
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &sa,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `h` is a new, open file handle that nothing else owns.
    Ok(unsafe { File::from_raw_handle(h) })
}

pub fn check(path: &Path, kind: Kind) -> io::Result<Option<Unprotected>> {
    let user = current_user()?;
    let name = wide(path)?;
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: valid NUL-terminated name and out-pointers; `sd` is LocalAlloc'ed.
    let rc = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    let _free = Local(sd);
    let trusted = |s: &str| s == user || s == SYSTEM || s == ADMINISTRATORS;
    let mut problems = Vec::new();
    let mut owner_bad = false;
    match sid_string(owner) {
        Some(o) if trusted(&o) => {}
        Some(o) => {
            owner_bad = true;
            problems.push(format!("owner {o}"));
        }
        None => {
            owner_bad = true;
            problems.push("owner unknown".to_string());
        }
    }
    // SIDs granted by explicit ACEs, which removing inheritance leaves in place.
    let mut explicit = Vec::new();
    let mut null_dacl = false;
    if dacl.is_null() {
        null_dacl = true;
        problems.push("null dacl".to_string());
    } else {
        // SAFETY: `dacl` points into `sd`, alive until `_free` drops.
        let count = unsafe { (*dacl).AceCount };
        for i in 0..u32::from(count) {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: `i` < AceCount.
            if unsafe { GetAce(dacl, i, &mut ace) } == 0 {
                problems.push(format!("unreadable ace {i}"));
                continue;
            }
            // SAFETY: every ACE starts with an ACE_HEADER.
            let header = unsafe { *(ace as *const ACE_HEADER) };
            match header.AceType {
                ACCESS_DENIED_ACE_TYPE
                | ACCESS_DENIED_OBJECT_ACE_TYPE
                | ACCESS_DENIED_CALLBACK_ACE_TYPE
                | ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE => {}
                ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE => {
                    let allowed = ace as *const ACCESS_ALLOWED_ACE;
                    // SAFETY: both types are laid out as ACCESS_ALLOWED_ACE up to `SidStart`.
                    let (mask, sid) = unsafe {
                        (
                            (*allowed).Mask,
                            std::ptr::addr_of!((*allowed).SidStart) as PSID,
                        )
                    };
                    if mask == 0 {
                        continue;
                    }
                    match sid_string(sid) {
                        Some(s) if trusted(&s) || OWNER_ALIASES.contains(&s.as_str()) => {}
                        Some(s) => {
                            if header.AceFlags & INHERITED_ACE == 0 && !explicit.contains(&s) {
                                explicit.push(s.clone());
                            }
                            let p = format!("allows {s}");
                            if !problems.contains(&p) {
                                problems.push(p);
                            }
                        }
                        None => problems.push(format!("unreadable ace {i}")),
                    }
                }
                other => problems.push(format!("unsupported ace {other}")),
            }
        }
    }
    if problems.is_empty() {
        return Ok(None);
    }
    Ok(Some(Unprotected {
        problems,
        fix: fix(path, kind, &user, owner_bad, &explicit, null_dacl),
    }))
}

/// The `icacls` commands (for `cmd`) that leave only the user and `SYSTEM` on `path`: own it,
/// grant both, drop inherited ACEs, then remove the explicit grants to anyone else.
fn fix(
    path: &Path,
    kind: Kind,
    user: &str,
    owner_bad: bool,
    explicit: &[String],
    null_dacl: bool,
) -> String {
    let p = path.display();
    let g = match kind {
        Kind::Folder => "(OI)(CI)F",
        Kind::File => "F",
    };
    let mut cmds = Vec::new();
    if owner_bad {
        cmds.push(format!("icacls \"{p}\" /setowner *{user}"));
    }
    cmds.push(format!("icacls \"{p}\" /grant:r *{user}:{g} *{SYSTEM}:{g}"));
    cmds.push(format!("icacls \"{p}\" /inheritance:r"));
    if !explicit.is_empty() {
        let sids: Vec<String> = explicit.iter().map(|s| format!("*{s}")).collect();
        cmds.push(format!("icacls \"{p}\" /remove:g {}", sids.join(" ")));
    }
    let mut out = format!("in cmd: {}", cmds.join(" && "));
    if null_dacl {
        out.push_str(" (FAT and exFAT volumes keep no ACLs: keep keys on an NTFS volume)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_text_names_every_step() {
        let f = fix(
            Path::new(r"E:\keys"),
            Kind::Folder,
            "S-1-5-21-1-2-3-1001",
            true,
            &["S-1-5-11".into(), "S-1-5-32-545".into()],
            false,
        );
        assert_eq!(
            f,
            r#"in cmd: icacls "E:\keys" /setowner *S-1-5-21-1-2-3-1001 && icacls "E:\keys" /grant:r *S-1-5-21-1-2-3-1001:(OI)(CI)F *S-1-5-18:(OI)(CI)F && icacls "E:\keys" /inheritance:r && icacls "E:\keys" /remove:g *S-1-5-11 *S-1-5-32-545"#
        );
        let f = fix(
            Path::new(r"E:\k.pem"),
            Kind::File,
            "S-1-5-21-9",
            false,
            &[],
            true,
        );
        assert!(f.starts_with(r#"in cmd: icacls "E:\k.pem" /grant:r *S-1-5-21-9:F *S-1-5-18:F && icacls "E:\k.pem" /inheritance:r"#), "{f}");
        assert!(f.ends_with("keep keys on an NTFS volume)"), "{f}");
    }

    /// Runs `cmd /C <line>`, panicking with its output on failure. The line is passed raw:
    /// `std` would escape its quotes as `\"`, which `cmd` does not read.
    fn cmd(line: &str) {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("cmd")
            .arg("/C")
            .raw_arg(line)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{line}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Whether `path`'s DACL is protected (inherits nothing), as PowerShell reads it.
    fn protected(path: &Path) -> bool {
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(format!(
                "(Get-Acl -LiteralPath '{}').AreAccessRulesProtected",
                path.display()
            ))
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim() == "True"
    }

    /// A folder that grants Authenticated Users modify and Users read to everything below it,
    /// as a folder at the root of a data drive (`E:\`) does.
    fn permissive(tmp: &Path) -> std::path::PathBuf {
        let d = tmp.join("open");
        std::fs::create_dir(&d).unwrap();
        cmd(&format!(
            "icacls \"{}\" /grant *S-1-5-11:(OI)(CI)M *S-1-5-32-545:(OI)(CI)R",
            d.display()
        ));
        d
    }

    #[test]
    fn new_folders_and_files_are_owner_only_under_a_permissive_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let open = permissive(tmp.path());
        // What the parent's ACL gives a file that inherits it.
        let inherited = open.join("plain.pem");
        std::fs::write(&inherited, b"x").unwrap();
        let u = check(&inherited, Kind::File)
            .unwrap()
            .expect("inherited ACL is refused");
        assert!(u.problems.contains(&"allows S-1-5-11".to_string()), "{u:?}");
        assert!(
            u.problems.contains(&"allows S-1-5-32-545".to_string()),
            "{u:?}"
        );

        let keys = open.join("keys");
        create_dir(&keys).unwrap();
        assert!(protected(&keys));
        assert_eq!(check(&keys, Kind::Folder).unwrap(), None);
        let f = keys.join("k.pem");
        {
            use std::io::Write;
            let mut h = create_new(&f).unwrap();
            h.write_all(b"secret").unwrap();
        }
        assert!(protected(&f));
        assert_eq!(check(&f, Kind::File).unwrap(), None);
        assert_eq!(std::fs::read(&f).unwrap(), b"secret");
        assert_eq!(
            create_new(&f).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        // A file another tool drops into the key folder inherits the folder's owner-only ACEs.
        let dropped = keys.join("copied.pem");
        std::fs::write(&dropped, b"y").unwrap();
        assert_eq!(check(&dropped, Kind::File).unwrap(), None);
        // A file renamed in keeps its own protected DACL.
        let moved = open.join("moved.pem");
        drop(create_new(&moved).unwrap());
        std::fs::rename(&moved, keys.join("moved.pem")).unwrap();
        assert_eq!(check(&keys.join("moved.pem"), Kind::File).unwrap(), None);
    }

    /// The fix text, run as given, makes a refused folder and file pass.
    #[test]
    fn the_fix_text_fixes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let open = permissive(tmp.path());
        let keys = open.join("keys");
        std::fs::create_dir(&keys).unwrap();
        // An explicit grant too, which removing inheritance leaves.
        cmd(&format!(
            "icacls \"{}\" /grant *S-1-1-0:(R)",
            keys.display()
        ));
        let f = keys.join("k.pem");
        std::fs::write(&f, b"x").unwrap();
        let u = check(&keys, Kind::Folder).unwrap().expect("refused");
        assert!(u.problems.contains(&"allows S-1-1-0".to_string()), "{u:?}");
        assert!(u.fix.contains("/remove:g *S-1-1-0"), "{}", u.fix);
        assert!(
            !u.fix.contains("*S-1-5-11 "),
            "inherited grants are not removed one by one"
        );
        cmd(u.fix.strip_prefix("in cmd: ").unwrap());
        assert_eq!(check(&keys, Kind::Folder).unwrap(), None);
        // The file only had inherited ACEs: fixing the folder fixed it.
        assert_eq!(check(&f, Kind::File).unwrap(), None);

        let g = open.join("loose.pem");
        std::fs::write(&g, b"x").unwrap();
        let u = check(&g, Kind::File).unwrap().expect("refused");
        cmd(u.fix.strip_prefix("in cmd: ").unwrap());
        assert_eq!(check(&g, Kind::File).unwrap(), None);
    }

    /// A folder in the user's temp folder (owner, SYSTEM and Administrators, inherited) passes:
    /// keys kept in the profile, as before, keep working.
    #[test]
    fn the_profile_default_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("keys");
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("k.pem"), b"x").unwrap();
        assert_eq!(check(&d, Kind::Folder).unwrap(), None);
        assert_eq!(check(&d.join("k.pem"), Kind::File).unwrap(), None);
    }

    #[test]
    fn verbatim_names() {
        let s = |p: &str| String::from_utf16(&wide(Path::new(p)).unwrap()).unwrap();
        assert_eq!(s(r"C:\a\..\b"), "\\\\?\\C:\\b\0");
        assert_eq!(s(r"\\host\share\k"), "\\\\?\\UNC\\host\\share\\k\0");
        assert_eq!(s(r"\\?\C:\k"), "\\\\?\\C:\\k\0");
    }
}
