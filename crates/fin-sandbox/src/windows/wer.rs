//! Fault reporting without UI (plan 7C Decision 22, S9 Windows point 11). The launcher sets
//! the error mode before `CreateProcessW` and never passes `CREATE_DEFAULT_ERROR_MODE`, so the
//! container's processes inherit it: an access violation then starts no `WerFault.exe` and
//! logs no Application Error event. `__fastfail` still reaches the WER service, which may start
//! a headless `WerFault.exe -u`; with the private desktop no window can reach the user, and no
//! report is kept (S9). Excluding the binaries from WER at install is sub-project 9's.

use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::ErrorReporting::{WER_FAULT_REPORTING_NO_UI, WerSetFlags};

/// The error-mode bits every container process inherits.
pub const ERROR_MODE: u32 = SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX;

/// Turns crash UI off for this process and the processes it creates.
pub fn crash_ui_off() {
    // SAFETY: process-wide flags; no pointers.
    unsafe {
        SetErrorMode(ERROR_MODE);
        WerSetFlags(WER_FAULT_REPORTING_NO_UI);
    }
}
