//! The spawn broker: how a child gets its three pipe ends without the host ever holding an
//! inheritable handle (Decision 10, as built).
//!
//! An inheritable handle in the host, even only around `CreateProcessW`, leaks into any other
//! `CreateProcess` with `bInheritHandles` in the host process during that call (libuv's, or
//! `std`'s), and the process tests show it does under load. So each host keeps one
//! **broker**: a process created suspended and never resumed (no code of it runs), in the host
//! Job so it dies with the host. For each spawn the three ends are duplicated *into the
//! broker* as inheritable handles, the child is created with
//! `PROC_THREAD_ATTRIBUTE_PARENT_PROCESS` = the broker (so it inherits from the broker's handle
//! table, restricted further by `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`), and the broker's copies
//! are closed right after. The host's own handles are never inheritable.
//!
//! - **Image.** Any image serves, since none of it runs. The first is the bundled
//!   `fin-workers.exe --spawn-broker` (0.4 MB of commit, where a copy of an Electron app's own
//!   executable costs about 8 MB; its `--spawn-broker` entry exits at once should anything
//!   resume it). The second is `%SystemRoot%\System32\cmd.exe /d /c exit 0`: always present
//!   and protected by Windows Resource Protection, found through `GetSystemDirectoryW` (never
//!   a `PATH` search), and harmless if resumed (`/d` skips the AutoRun commands, `/c exit 0`
//!   ends it at once, `DETACHED_PROCESS` gives it no console). `conhost.exe` is not used: it
//!   opens a console window when resumed.
//! - **Access.** The broker and its thread are created with a protected DACL that grants
//!   access to `SYSTEM` only, plus an `OWNER RIGHTS` entry with no rights (which removes the
//!   owner's implicit `READ_CONTROL | WRITE_DAC`). No other process of the user, a worker
//!   included, can open it to copy the pipe ends it holds for a moment or to write into it;
//!   the host keeps full access through the handles `CreateProcessW` returned.
//! - **Lifetime.** It is made when the host starts (so its slot in a Job with an
//!   active-process limit is taken first) and made again at a spawn that finds it dead.
//! - **No broker, no spawn.** When neither image can be made, the spawn is refused
//!   (`WORKER_UNAVAILABLE`, reason `noBroker`): there is no inheritable fallback.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::OwnedHandle;
use std::path::PathBuf;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{
    DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, LocalFree, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DETACHED_PROCESS, GetCurrentProcess, PROCESS_INFORMATION,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};

use super::env_block::{command_line, wide};
use super::{check, owned, raw};
use crate::LogFn;

/// `SYSTEM` only; `OWNER RIGHTS` with no rights; protected (nothing inherited).
const BROKER_SDDL: &str = "D:P(A;;GA;;;SY)(A;;0x0;;;OW)";

/// The reason a spawn without a broker is refused with.
pub(crate) const NO_BROKER: &str = "noBroker";

/// An executable the broker can be made from, and its (never used) arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Image {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// The images in order: the bundled `fin-workers.exe`, then the system's `cmd.exe`.
pub(crate) fn default_images(binary_dir: &std::path::Path) -> Vec<Image> {
    let mut v = Vec::new();
    let bundled = crate::spawner::Program::Bundled("fin-workers".into());
    if let Ok(program) = crate::spawner::resolve(binary_dir, &bundled) {
        v.push(Image {
            program,
            args: vec!["--spawn-broker".into()],
        });
    }
    if let Some(sys) = system_dir() {
        v.push(Image {
            program: sys.join("cmd.exe"),
            args: ["/d", "/c", "exit", "0"].map(OsString::from).to_vec(),
        });
    }
    v
}

fn system_dir() -> Option<PathBuf> {
    let mut buf = [0u16; 260];
    // SAFETY: `buf` holds `buf.len()` u16s; the call writes at most that many.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(OsString::from_wide(&buf[..n])))
}

/// A security descriptor from `LocalAlloc`, freed when dropped.
struct Sd(PSECURITY_DESCRIPTOR);

impl Sd {
    fn broker() -> io::Result<Sd> {
        let text = wide(BROKER_SDDL.as_ref())?;
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: a NUL-terminated SDDL string; `sd` receives a LocalAlloc'd descriptor.
        check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        })?;
        Ok(Sd(sd))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for Sd {
    fn drop(&mut self) {
        // SAFETY: the descriptor the conversion allocated, freed once.
        unsafe { LocalFree(self.0) };
    }
}

/// A suspended, locked-down process of `image` in `host_job`.
fn make(image: &Image, host_job: &OwnedHandle) -> io::Result<OwnedHandle> {
    if !image.program.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} does not exist", image.program.display()),
        ));
    }
    let app = wide(image.program.as_os_str())?;
    let mut line = command_line(&image.program, &image.args)?;
    let sd = Sd::broker()?;
    let (pa, ta) = (sd.attributes(), sd.attributes());
    // SAFETY: a plain C struct; zeros are its defaults.
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = size_of::<STARTUPINFOW>() as u32;
    // SAFETY: a plain C struct, filled by the call.
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: valid, NUL-terminated strings and security attributes that outlive the call; no
    // handle is inherited.
    check(unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            &pa,
            &ta,
            0,
            CREATE_SUSPENDED | DETACHED_PROCESS,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        )
    })?;
    let process = owned(pi.hProcess)?;
    drop(owned(pi.hThread)?);
    // SAFETY: our Job and the process just made.
    if let Err(e) = check(unsafe { AssignProcessToJobObject(raw(host_job), raw(&process)) }) {
        // SAFETY: our own suspended process.
        unsafe { TerminateProcess(raw(&process), 1) };
        return Err(e);
    }
    Ok(process)
}

fn alive(p: &OwnedHandle) -> bool {
    // SAFETY: a process handle we own.
    unsafe { WaitForSingleObject(raw(p), 0) == WAIT_TIMEOUT }
}

struct Made {
    process: OwnedHandle,
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    image: PathBuf,
}

pub(crate) struct Broker {
    current: Mutex<Option<Made>>,
    images: Vec<Image>,
    log: LogFn,
}

impl Broker {
    /// Makes the broker now, in `host_job` (a failure is logged; spawns retry).
    pub fn new(images: Vec<Image>, host_job: &OwnedHandle, log: LogFn) -> Broker {
        let b = Broker {
            current: Mutex::new(None),
            images,
            log,
        };
        let mut slot = b.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = b.remake(&mut slot, host_job) {
            (b.log)(&format!(
                "no spawn broker yet ({e}); spawns are refused until one is made"
            ));
        }
        drop(slot);
        b
    }

    fn remake(&self, slot: &mut Option<Made>, host_job: &OwnedHandle) -> io::Result<()> {
        let mut errors = Vec::new();
        for image in &self.images {
            match make(image, host_job) {
                Ok(process) => {
                    *slot = Some(Made {
                        process,
                        image: image.program.clone(),
                    });
                    if !errors.is_empty() {
                        (self.log)(&format!(
                            "spawn broker from {} ({})",
                            image.program.display(),
                            errors.join("; ")
                        ));
                    }
                    return Ok(());
                }
                Err(e) => errors.push(format!("{}: {e}", image.program.display())),
            }
        }
        *slot = None;
        if errors.is_empty() {
            errors.push("no image".into());
        }
        Err(io::Error::other(errors.join("; ")))
    }

    /// Runs `f` with the broker (remade if it died), or refuses with `noBroker`.
    pub fn with<R>(
        &self,
        host_job: &OwnedHandle,
        f: impl FnOnce(HANDLE) -> io::Result<R>,
    ) -> io::Result<R> {
        let mut slot = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if !slot.as_ref().is_some_and(|m| alive(&m.process))
            && let Err(e) = self.remake(&mut slot, host_job)
        {
            return Err(crate::Refusal::io(NO_BROKER, e.to_string()));
        }
        let broker = raw(&slot.as_ref().expect("made").process);
        f(broker)
    }

    /// The broker's pid and image (tests).
    #[cfg(feature = "test-hooks")]
    pub fn current(&self) -> Option<(u32, PathBuf)> {
        use windows_sys::Win32::System::Threading::GetProcessId;
        let slot = self.current.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_ref().map(|m| {
            // SAFETY: a process handle we own.
            let pid = unsafe { GetProcessId(raw(&m.process)) };
            (pid, m.image.clone())
        })
    }
}

/// Copies of the child's ends in the broker, inheritable there; closed there when dropped.
pub(crate) struct InBroker {
    broker: HANDLE,
    pub handles: Vec<HANDLE>,
}

impl InBroker {
    pub fn new(broker: HANDLE, ours: &[HANDLE]) -> io::Result<InBroker> {
        let mut me = InBroker {
            broker,
            handles: Vec::with_capacity(ours.len()),
        };
        for &h in ours {
            let mut theirs: HANDLE = std::ptr::null_mut();
            // SAFETY: our handle, copied into the broker (which we hold with full access).
            check(unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    h,
                    broker,
                    &mut theirs,
                    0,
                    1,
                    DUPLICATE_SAME_ACCESS,
                )
            })?;
            me.handles.push(theirs);
        }
        Ok(me)
    }
}

impl Drop for InBroker {
    fn drop(&mut self) {
        for &h in &self.handles {
            // SAFETY: closes a handle in the broker's table that we put there.
            unsafe {
                DuplicateHandle(
                    self.broker,
                    h,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    DUPLICATE_CLOSE_SOURCE,
                )
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_the_bundled_binary_then_cmd() {
        let dir = std::env::temp_dir().join(format!("fin-broker-images-{}", std::process::id()));
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(dir.join("fin-workers.exe"), b"").unwrap();
        let v = default_images(&dir);
        assert_eq!(v[0].program, dir.join("fin-workers.exe"));
        assert_eq!(v[0].args, [OsString::from("--spawn-broker")]);
        assert!(v[1].program.ends_with("cmd.exe"), "{v:?}");
        assert!(v[1].program.is_absolute(), "{v:?}");
        // Without the bundled binary, cmd.exe alone.
        let only = default_images(&empty);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(only.len(), 1, "{only:?}");
        assert!(only[0].program.ends_with("cmd.exe"));
    }

    #[test]
    fn the_broker_descriptor_parses() {
        drop(Sd::broker().unwrap());
    }
}
