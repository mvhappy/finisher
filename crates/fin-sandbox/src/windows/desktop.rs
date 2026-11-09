//! The container's private window station and desktop (plan 7C Decisions 16 and 22, S9
//! Windows point 5): a dialog, if one were raised, lands on a desktop nobody sees and cannot
//! block the user's. The window station is **unnamed** (a named one fails with 5 from a
//! non-elevated, medium-integrity process). Windows names an unnamed station after the logon
//! session, so every launcher of the user gets **the same** station: its descriptor admits
//! every container (`ALL APPLICATION PACKAGES`), and each launch gets its own desktop in it,
//! named uniquely, whose descriptor admits that launch's container alone. A shared `default`
//! desktop would keep the descriptor of whichever launch made it first, and a second
//! container's processes would then fail to start (`STATUS_DLL_INIT_FAILED`). Both live as
//! long as this value; close it only after the container's processes have ended.

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, CloseWindowStation, CreateDesktopW, CreateWindowStationW,
    GetProcessWindowStation, GetUserObjectInformationW, HDESK, HWINSTA, SetProcessWindowStation,
    UOI_NAME,
};

use super::sid::{current_user, from_wide, wide};

/// `GENERIC_ALL` (`MAXIMUM_ALLOWED` is not accepted by `CreateDesktopW`).
const GENERIC_ALL: u32 = 0x1000_0000;

/// A private window station and its desktop.
pub struct PrivateDesktop {
    winsta: HWINSTA,
    desktop: HDESK,
    name: String,
}

// SAFETY: window-station and desktop handles are process-wide kernel handles.
unsafe impl Send for PrivateDesktop {}
// SAFETY: as above; only read after creation.
unsafe impl Sync for PrivateDesktop {}

struct Sd(PSECURITY_DESCRIPTOR);

impl Drop for Sd {
    fn drop(&mut self) {
        // SAFETY: LocalAlloc'd by the conversion, freed once.
        unsafe { LocalFree(self.0 as HLOCAL) };
    }
}

impl PrivateDesktop {
    /// A window station and desktop the user, `SYSTEM` and `container` (a SID string: the
    /// container's own, or `S-1-15-2-1` for every container) may use.
    pub fn new(container: &str) -> io::Result<PrivateDesktop> {
        let user = current_user()?.string();
        let station_sd = descriptor(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;{user})(A;;GA;;;{ALL_APP_PACKAGES})"
        ))?;
        let desktop_sd = descriptor(&desktop_sddl(&user, container))?;
        let attrs = |sd: &Sd| SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        let sa = attrs(&station_sd);
        // SAFETY: an unnamed window station with the descriptor above.
        let winsta = unsafe { CreateWindowStationW(null(), 0, GENERIC_ALL, &sa) };
        if winsta.is_null() {
            return Err(io::Error::last_os_error());
        }
        let desktop_name = unique_name();
        let name = match object_name(winsta) {
            Ok(n) => n,
            Err(e) => {
                // SAFETY: the station we just made.
                unsafe { CloseWindowStation(winsta) };
                return Err(e);
            }
        };
        // A desktop is created in the calling process's window station: switch for the call.
        // SAFETY: plain handle queries and switches; the old station is restored at once.
        let desktop = unsafe {
            let old = GetProcessWindowStation();
            if SetProcessWindowStation(winsta) == 0 {
                let e = io::Error::last_os_error();
                CloseWindowStation(winsta);
                return Err(e);
            }
            let dsa = attrs(&desktop_sd);
            let d = CreateDesktopW(
                wide(&desktop_name).as_ptr(),
                null(),
                null(),
                0,
                GENERIC_ALL,
                &dsa,
            );
            let e = io::Error::last_os_error();
            SetProcessWindowStation(old);
            if d.is_null() {
                CloseWindowStation(winsta);
                return Err(e);
            }
            d
        };
        Ok(PrivateDesktop {
            winsta,
            desktop,
            name: format!("{name}\\{desktop_name}"),
        })
    }

    /// `<window station>\<desktop>`, for `STARTUPINFO.lpDesktop`.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// `ALL APPLICATION PACKAGES`.
const ALL_APP_PACKAGES: &str = "S-1-15-2-1";

/// The desktop's descriptor: the user, `SYSTEM` and one container.
pub fn desktop_sddl(user: &str, container: &str) -> String {
    format!("D:P(A;;GA;;;SY)(A;;GA;;;{user})(A;;GA;;;{container})")
}

/// A desktop name no other launch uses: `fin-<pid>-<n>`.
fn unique_name() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "fin-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn descriptor(sddl: &str) -> io::Result<Sd> {
    let mut sd = Sd(null_mut());
    // SAFETY: a NUL-terminated SDDL string and a valid out-pointer.
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
    Ok(sd)
}

fn object_name(h: HWINSTA) -> io::Result<String> {
    let mut buf = [0u16; 256];
    let mut n = 0u32;
    // SAFETY: a valid handle and a buffer of the size given.
    let ok = unsafe {
        GetUserObjectInformationW(
            h,
            UOI_NAME,
            buf.as_mut_ptr().cast(),
            (buf.len() * 2) as u32,
            &mut n,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call wrote a NUL-terminated name.
    Ok(unsafe { from_wide(buf.as_ptr()) })
}

impl Drop for PrivateDesktop {
    fn drop(&mut self) {
        // SAFETY: our own handles, closed once.
        unsafe {
            CloseDesktop(self.desktop);
            CloseWindowStation(self.winsta);
        }
    }
}
