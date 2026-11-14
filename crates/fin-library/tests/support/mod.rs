//! Helpers shared by the source, opener and mount tests (plan 5D Tasks 2–3; the mount and
//! namespace helpers are Unix-only).
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![allow(dead_code)]

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use fin_library::{Expect, RawRel};

#[cfg(target_os = "macos")]
pub mod image;

/// What the catalog would expect for the file at `path` (size, mtime, inode big-endian, and
/// its device as the OS's listing reports it).
#[cfg(unix)]
pub fn expect_of(path: &Path) -> Expect {
    let m = fs::symlink_metadata(path).unwrap();
    Expect {
        size: m.len(),
        mtime_ns: m.mtime() * 1_000_000_000 + m.mtime_nsec(),
        file_id: Some(m.ino().to_be_bytes().to_vec()),
        dev: Some(dev_of(m.dev())),
    }
}

/// `st_dev` as the source reports it (`ScanEntry.dev`): Linux `major:minor`, macOS decimal.
#[cfg(unix)]
pub fn dev_of(dev: u64) -> String {
    #[cfg(target_os = "linux")]
    {
        let d = dev as libc::dev_t;
        format!("{}:{}", libc::major(d), libc::minor(d))
    }
    #[cfg(not(target_os = "linux"))]
    {
        (dev as i64 as i32).to_string()
    }
}

/// What the catalog would expect for the file at `path`: on Windows, its entry as the
/// native source lists its directory (size, mtime, 128-bit id, volume serial).
#[cfg(windows)]
pub fn expect_of(path: &Path) -> Expect {
    use fin_library::Source;
    let src = fin_library::source::NativeSource::new(path.parent().unwrap());
    let name = fin_library::raw::from_wide(
        &std::os::windows::ffi::OsStrExt::encode_wide(path.file_name().unwrap())
            .collect::<Vec<_>>(),
    );
    let dir = src.open_dir(&RawRel::root()).unwrap();
    let e = src.stat_child(&dir, &name).unwrap().expect("listed");
    Expect {
        size: e.size,
        mtime_ns: e.mtime_ns,
        file_id: e.file_id,
        dev: e.dev,
    }
}

/// A raw chain from byte components.
pub fn rel(parts: &[&[u8]]) -> RawRel {
    RawRel::new(parts.iter().map(|p| p.to_vec()).collect()).unwrap()
}

#[cfg(unix)]
pub fn euid() -> u32 {
    // SAFETY: no preconditions.
    unsafe { libc::geteuid() }
}

/// Prints why a test did not run its OS-facility part (the facility is named).
pub fn skip(test: &str, why: &str) {
    eprintln!("SKIP {test}: {why}");
}

/// Set (CI on a volume that keeps raw names, e.g. ext4 or tmpfs) to fail [`raw_names`] tests
/// instead of skipping them.
#[cfg(unix)]
pub const REQUIRE_RAW_NAMES: &str = "FIN_REQUIRE_RAW_NAMES";

/// Whether the volume under `dir` accepts every one of `names` as a file name, byte for byte.
/// APFS refuses names that are not UTF-8 (`EILSEQ`, or `EINVAL`), so a test that builds such a
/// tree cannot run on macOS: this answers `false` after printing a skip notice naming the
/// volume's refusal. With `FIN_REQUIRE_RAW_NAMES=1` a refusal fails the test instead of
/// skipping it. Any other error is a real failure.
#[cfg(unix)]
pub fn raw_names(test: &str, dir: &Path, names: &[&[u8]]) -> bool {
    use std::os::unix::ffi::OsStrExt;
    for name in names {
        let probe = dir.join(std::ffi::OsStr::from_bytes(name));
        match fs::create_dir(&probe) {
            Ok(()) => fs::remove_dir(&probe).unwrap(),
            Err(e) if matches!(e.raw_os_error(), Some(libc::EILSEQ | libc::EINVAL)) => {
                let why = format!(
                    "the volume under {} refuses the non-UTF-8 name {name:?} ({e})",
                    dir.display()
                );
                assert!(
                    std::env::var(REQUIRE_RAW_NAMES).as_deref() != Ok("1"),
                    "{REQUIRE_RAW_NAMES}=1 but {why}"
                );
                skip(test, &format!("{why}; {REQUIRE_RAW_NAMES}=1 fails instead"));
                return false;
            }
            Err(e) => panic!("creating the probe name {name:?} in {}: {e}", dir.display()),
        }
    }
    true
}

const INSIDE: &str = "FIN_TEST_USERNS";

/// Runs `test` (this test binary's test of that name) again inside a user and mount
/// namespace where it may mount tmpfs and bind mounts: `unshare -Urm` where unprivileged user
/// namespaces are allowed, else `bwrap`. Each is first asked to mount a tmpfs: under Ubuntu's
/// `kernel.apparmor_restrict_unprivileged_userns = 1` `unshare -Urm` cannot write its uid map,
/// and `bwrap` starts but its children get no capabilities, so every mount is `EPERM`. Answers
/// `true` inside the namespace (the caller then does the work); outside it runs the child,
/// asserts it passed and answers `false`, or prints a skip notice naming the facility. With
/// `FIN_REQUIRE_USERNS=1` (CI where user namespaces may mount) a missing facility fails the
/// test instead of skipping it.
#[cfg(target_os = "linux")]
pub fn userns(test: &str) -> bool {
    if std::env::var(INSIDE).as_deref() == Ok(test) {
        return true;
    }
    let exe = std::env::current_exe().unwrap();
    let mut cmd = if may_mount("unshare", &["-Urm"]) {
        let mut c = Command::new("unshare");
        c.arg("-Urm").arg(&exe);
        c
    } else if may_mount("bwrap", BWRAP) {
        let mut c = Command::new("bwrap");
        c.args(BWRAP).arg(&exe);
        c
    } else {
        let why = "no unprivileged user namespace that may mount (neither `unshare -Urm` nor \
                   `bwrap` can mount a tmpfs here, e.g. under \
                   kernel.apparmor_restrict_unprivileged_userns = 1)";
        assert!(
            std::env::var(REQUIRE_USERNS).as_deref() != Ok("1"),
            "{REQUIRE_USERNS}=1 but {why}"
        );
        skip(test, &format!("{why}; {REQUIRE_USERNS}=1 fails instead"));
        return false;
    };
    let out = cmd
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(INSIDE, test)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "{test} failed in the namespace:\n{text}"
    );
    assert!(
        text.contains("1 passed"),
        "{test} did not run in the namespace:\n{text}"
    );
    eprintln!("{test}: passed in a user namespace");
    false
}

/// Set (CI where unprivileged user namespaces may mount) to fail [`userns`] tests instead of
/// skipping them.
#[cfg(target_os = "linux")]
pub const REQUIRE_USERNS: &str = "FIN_REQUIRE_USERNS";

/// Whether `wrapper args…` gives its child a namespace in which a tmpfs mount succeeds.
#[cfg(target_os = "linux")]
fn may_mount(wrapper: &str, args: &[&str]) -> bool {
    let Ok(dir) = tempfile::tempdir() else {
        return false;
    };
    Command::new(wrapper)
        .args(args)
        .args(["sh", "-c", "mount -t tmpfs t \"$1\" && umount \"$1\"", "sh"])
        .arg(dir.path())
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(target_os = "linux")]
const BWRAP: &[&str] = &[
    "--dev-bind",
    "/",
    "/",
    "--unshare-user",
    "--uid",
    "0",
    "--gid",
    "0",
    "--cap-add",
    "ALL",
];

#[cfg(target_os = "linux")]
fn c(p: &Path) -> std::ffi::CString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap()
}

/// `mount --bind src dst` (inside [`userns`]).
#[cfg(target_os = "linux")]
pub fn bind(src: &Path, dst: &Path) {
    // SAFETY: valid C strings; null fstype and data are allowed for MS_BIND.
    let r = unsafe {
        libc::mount(
            c(src).as_ptr(),
            c(dst).as_ptr(),
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        )
    };
    assert_eq!(r, 0, "bind mount: {}", std::io::Error::last_os_error());
}

/// `mount -t tmpfs t dst` (inside [`userns`]).
#[cfg(target_os = "linux")]
pub fn tmpfs(dst: &Path) {
    // SAFETY: valid C strings.
    let r = unsafe {
        libc::mount(
            c"t".as_ptr(),
            c(dst).as_ptr(),
            c"tmpfs".as_ptr(),
            0,
            std::ptr::null(),
        )
    };
    assert_eq!(r, 0, "tmpfs mount: {}", std::io::Error::last_os_error());
}

#[cfg(target_os = "linux")]
pub fn umount(dst: &Path) {
    // SAFETY: a valid C string.
    let r = unsafe { libc::umount(c(dst).as_ptr()) };
    assert_eq!(r, 0, "umount: {}", std::io::Error::last_os_error());
}

/// Runs a command; `None` when it cannot start or fails (the facility is missing).
pub fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
