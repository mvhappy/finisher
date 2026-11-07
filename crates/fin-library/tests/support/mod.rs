//! Helpers shared by the Unix source, opener and mount tests (plan 5D Task 2).
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use fin_library::{Expect, RawRel};

#[cfg(target_os = "macos")]
pub mod image;

/// What the catalog would expect for the file at `path` (size, mtime, inode big-endian).
pub fn expect_of(path: &Path) -> Expect {
    let m = fs::symlink_metadata(path).unwrap();
    Expect {
        size: m.len(),
        mtime_ns: m.mtime() * 1_000_000_000 + m.mtime_nsec(),
        file_id: Some(m.ino().to_be_bytes().to_vec()),
    }
}

/// A raw chain from byte components.
pub fn rel(parts: &[&[u8]]) -> RawRel {
    RawRel::new(parts.iter().map(|p| p.to_vec()).collect()).unwrap()
}

pub fn euid() -> u32 {
    // SAFETY: no preconditions.
    unsafe { libc::geteuid() }
}

/// Prints why a test did not run its OS-facility part (the facility is named).
pub fn skip(test: &str, why: &str) {
    eprintln!("SKIP {test}: {why}");
}

const INSIDE: &str = "FIN_TEST_USERNS";

/// Runs `test` (this test binary's test of that name) again inside a user and mount
/// namespace where it may mount tmpfs and bind mounts: `unshare -Urm` where unprivileged user
/// namespaces are allowed, else `bwrap` (Ubuntu's AppArmor restriction permits it). Answers
/// `true` inside the namespace (the caller then does the work); outside it runs the child,
/// asserts it passed and answers `false`, or prints a skip notice naming the facility.
#[cfg(target_os = "linux")]
pub fn userns(test: &str) -> bool {
    if std::env::var(INSIDE).as_deref() == Ok(test) {
        return true;
    }
    let exe = std::env::current_exe().unwrap();
    let ok = |c: &mut Command| c.output().is_ok_and(|o| o.status.success());
    let mut cmd = if ok(Command::new("unshare").args(["-Urm", "true"])) {
        let mut c = Command::new("unshare");
        c.arg("-Urm").arg(&exe);
        c
    } else if ok(Command::new("bwrap").args(BWRAP).arg("true")) {
        let mut c = Command::new("bwrap");
        c.args(BWRAP).arg(&exe);
        c
    } else {
        skip(
            test,
            "no unprivileged user namespace (neither `unshare -Urm` nor `bwrap` works here)",
        );
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
