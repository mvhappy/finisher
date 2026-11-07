//! The `test` family (feature `test-hooks`, spec §6.9, 5C-1 plan Task 2) and its two
//! one-kind siblings: `testbad` (whose `hello` claims another envelope) and `testplugin` (a
//! plugin-like process: minor 1, no envelope). Several kinds break the worker rules on
//! purpose (`test.spawn` starts processes, `test.garbage` breaks the protocol): they exist to
//! test the host. This directory is outside the worker spawn gate (Decision 24).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

use fin_jobs::test_kinds::{CrashHow, Detach};
use fin_jobs::worker::{Family, JobCtx};
use fin_jobs::{Hello, JobError, KindVersion, StartJob};
use serde_json::Value;

mod batch;
pub mod helper;
mod kinds;

static TEST_KINDS: &[KindVersion] = &[
    KindVersion::new("test.sleep", 1),
    KindVersion::new("test.echo", 1),
    KindVersion::new("test.progressFlood", 1),
    KindVersion::new("test.warm", 1),
    KindVersion::new("test.crash", 1),
    KindVersion::new("test.hang", 1),
    KindVersion::new("test.stall", 1),
    KindVersion::new("test.spawn", 1),
    KindVersion::new("test.files", 1),
    KindVersion::new("test.checkpoint", 1),
    KindVersion::new("test.oom", 1),
    KindVersion::new("test.call", 1),
    KindVersion::new("test.diag", 1),
    KindVersion::new("test.garbage", 1),
    KindVersion::new("test.exitAfterResult", 1),
    KindVersion::new("test.helper", 1),
];
static TESTBAD_KINDS: &[KindVersion] = &[KindVersion::new("test.badHello", 1)];
static TESTPLUGIN_KINDS: &[KindVersion] = &[KindVersion::new("test.pluginMinor", 1)];

/// The envelope `testbad` claims.
pub const BAD_ENVELOPE: &str = "0000000000000000";

/// One of the three test families.
pub struct TestFamily {
    name: &'static str,
    warm: bool,
    /// Jobs this process has run (`test.warm`'s `served`).
    served: u64,
}

impl TestFamily {
    pub fn new(name: &str, warm: bool) -> TestFamily {
        let name = match name {
            "testbad" => "testbad",
            "testplugin" => "testplugin",
            _ => "test",
        };
        TestFamily {
            name,
            warm,
            served: 0,
        }
    }
}

impl Family for TestFamily {
    fn name(&self) -> &'static str {
        self.name
    }

    fn kinds(&self) -> &'static [KindVersion] {
        match self.name {
            "testbad" => TESTBAD_KINDS,
            "testplugin" => TESTPLUGIN_KINDS,
            _ => TEST_KINDS,
        }
    }

    fn warm(&self) -> bool {
        self.warm
    }

    fn run(&mut self, job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
        self.served += 1;
        if !self
            .kinds()
            .iter()
            .any(|k| k.kind == job.kind && k.version == job.kind_version)
        {
            return Err(JobError::new(
                "JOB_KIND_UNKNOWN",
                format!(
                    "family {} has no kind {} v{}",
                    self.name, job.kind, job.kind_version
                ),
            ));
        }
        kinds::run(job, ctx, self.served)
    }

    fn hello_for_test(&self, hello: &mut Hello) {
        match self.name {
            "testbad" => hello.envelope = Some(BAD_ENVELOPE.to_owned()),
            "testplugin" => {
                hello.minor = 1;
                hello.envelope = None;
            }
            _ => {}
        }
    }
}

/// Decodes `job.params` as `T`; `JOB_PARAMS_INVALID` otherwise.
pub(crate) fn params<T: serde::de::DeserializeOwned>(job: &StartJob) -> Result<T, JobError> {
    serde_json::from_value(job.params.clone())
        .map_err(|e| JobError::new("JOB_PARAMS_INVALID", format!("{}: {e}", job.kind)))
}

/// Sleeps `ms` in short steps; `JOB_CANCELLED` as soon as `ctx` is cancelled.
pub(crate) fn sleep_or_cancel(
    token: &fin_jobs::worker::CancelToken,
    ms: u64,
) -> Result<(), JobError> {
    let mut left = ms;
    while left > 0 {
        if token.is_cancelled() {
            return Err(JobError::cancelled());
        }
        let step = left.min(10);
        std::thread::sleep(Duration::from_millis(step));
        left -= step;
    }
    if token.is_cancelled() {
        return Err(JobError::cancelled());
    }
    Ok(())
}

/// Dies as `how` asks: a signal on Unix, an exception or `__fastfail` on Windows, `exit(3)`.
pub(crate) fn die(how: CrashHow) -> ! {
    match how {
        CrashHow::Exit => std::process::exit(3),
        CrashHow::Abort => std::process::abort(),
        CrashHow::Segv => segv(),
        CrashHow::Bus => bus(),
        CrashHow::Fastfail => fastfail(),
    }
}

#[cfg(unix)]
fn raise_default(sig: libc::c_int) -> ! {
    // SAFETY: resets one signal to its default action and raises it; the process dies.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
    std::process::abort()
}

#[cfg(unix)]
fn segv() -> ! {
    raise_default(libc::SIGSEGV)
}

#[cfg(unix)]
fn bus() -> ! {
    raise_default(libc::SIGBUS)
}

/// No `__fastfail` outside Windows: an abort.
#[cfg(unix)]
fn fastfail() -> ! {
    std::process::abort()
}

#[cfg(windows)]
fn segv() -> ! {
    // SAFETY: a deliberate access violation (0xC0000005); the process dies.
    unsafe { std::ptr::null_mut::<u8>().write_volatile(1) };
    std::process::abort()
}

#[cfg(windows)]
fn bus() -> ! {
    use windows_sys::Win32::Foundation::EXCEPTION_IN_PAGE_ERROR;
    use windows_sys::Win32::System::Diagnostics::Debug::RaiseException;
    // SAFETY: raises a non-continuable EXCEPTION_IN_PAGE_ERROR (0xC0000006); the process dies.
    unsafe { RaiseException(EXCEPTION_IN_PAGE_ERROR as u32, 1, 0, std::ptr::null()) };
    std::process::abort()
}

#[cfg(windows)]
fn fastfail() -> ! {
    // SAFETY: `__fastfail(FAST_FAIL_FATAL_APP_EXIT)`: the process ends with 0xC0000409.
    unsafe {
        #[cfg(target_arch = "x86_64")]
        std::arch::asm!("int 0x29", in("ecx") 7u32, options(noreturn));
        #[cfg(target_arch = "aarch64")]
        std::arch::asm!("brk 0xF003", in("x0") 7u64, options(noreturn));
    }
}

/// Starts the double-forked sleeper of `test.spawn` and the `spawn` helper: a middle process
/// (`--test-spawn-mid`) that leaves the group with `setsid` if asked, starts the grandchild
/// (`--test-sleep`), prints its pid and exits. Returns `(mid, grandchild)`.
pub(crate) fn spawn_grandchild(detach: Detach, sleep_ms: u64) -> std::io::Result<(u32, u32)> {
    let exe = std::env::current_exe()?;
    let detach = match detach {
        Detach::None => "none",
        Detach::Setsid => "setsid",
    };
    let mut mid = Command::new(exe)
        .args(["--test-spawn-mid", detach, &sleep_ms.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut line = String::new();
    BufReader::new(mid.stdout.take().expect("piped")).read_line(&mut line)?;
    let mid_pid = mid.id();
    mid.wait()?;
    let grandchild = line
        .trim()
        .parse()
        .map_err(|_| std::io::Error::other(format!("no grandchild pid in {line:?}")))?;
    Ok((mid_pid, grandchild))
}

/// The hidden entries of the test binary: `--test-spawn-mid <none|setsid> <ms>` and
/// `--test-sleep <ms>`. `None` for anything else.
pub fn hidden_main(args: &[String]) -> Option<i32> {
    match args.first().map(String::as_str)? {
        "--test-spawn-mid" => {
            #[cfg(unix)]
            if args.get(1).map(String::as_str) == Some("setsid") {
                // SAFETY: the middle process is not a group leader; it leaves its group.
                unsafe { libc::setsid() };
            }
            let ms = args.get(2).cloned().unwrap_or_else(|| "600000".into());
            let exe = std::env::current_exe().ok()?;
            let child = Command::new(exe)
                .args(["--test-sleep", &ms])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            Some(match child {
                Ok(c) => {
                    println!("{}", c.id());
                    0
                }
                Err(_) => 1,
            })
        }
        "--test-sleep" => {
            let ms = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(600_000);
            std::thread::sleep(Duration::from_millis(ms));
            Some(0)
        }
        _ => None,
    }
}
