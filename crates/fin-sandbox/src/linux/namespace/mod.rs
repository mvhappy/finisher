//! Unix sockets hidden by a namespace (plan 7C Decision 26, owner decision 2026-11-08). Below
//! Landlock ABI 9, and with the seccomp filter deferred, nothing refuses `connect()` on a
//! pathname socket, so the launcher puts `agent` and `hook` processes in a user and mount
//! namespace where the usual socket locations are empty `tmpfs` mounts and only the policy's
//! granted paths keep their real content. A `hook` without network also gets an empty network
//! namespace.
//!
//! - [`layout`] (module) is the pure plan: which roots to hide, which granted paths to bind
//!   back, in mount order; [`plan`] resolves it for a policy on this host.
//! - [`probe`] tries the namespace in a forked child, so a half-made one (the AppArmor
//!   restriction lets `unshare` succeed and then refuses the uid map) never reaches the
//!   program; [`enter`] makes it for the calling process, which must have one thread.
//!
//! Test hook (`test-hooks` builds only): `FIN_TEST_SANDBOX_NAMESPACES=unavailable` makes
//! [`probe`] answer unavailable without trying.

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::error::ApplyError;
use crate::status::Namespaces;

pub mod caps;
pub mod layout;

pub use layout::{HIDDEN, Layout, Step, layout, plan, wants_net};

/// The test hook's variable (`test-hooks` builds only).
pub const TEST_ENV: &str = "FIN_TEST_SANDBOX_NAMESPACES";
/// The options of each hiding `tmpfs`.
const TMPFS_OPTS: &str = "mode=0755,size=64k";
/// The errors that mean "this host does not let this process make the namespace" (the
/// fallback of Decision 26); any other failure of the probe refuses the start, so a transient
/// one (`EAGAIN` from a fork near `RLIMIT_NPROC`, `EMFILE`, `ENOMEM`) cannot drop the namespace.
const NOT_PERMITTED: &[i32] = &[
    libc::EPERM,
    libc::EACCES,
    libc::EINVAL,
    libc::ENOSPC,
    libc::ENOSYS,
    libc::EUSERS,
];

/// What [`probe_outcome`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Available,
    /// The host refuses the namespace: run with Landlock alone (`SANDBOX_SOCKETS_EXPOSED`).
    NotPermitted(String),
    /// Anything else: refuse the start (`SANDBOX_UNAVAILABLE`).
    Failed(String),
}

fn cstr(p: &Path) -> Result<CString, ApplyError> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| ApplyError::Rule {
        path: p.to_owned(),
        error: "a NUL byte in the path".into(),
    })
}

/// The `unshare` flags of a layout.
fn flags(net: bool) -> libc::c_int {
    libc::CLONE_NEWUSER | libc::CLONE_NEWNS | if net { libc::CLONE_NEWNET } else { 0 }
}

/// The three files that map `pid`'s ids to themselves (`<id> <id> 1`, so ownership reads the
/// same inside), with their contents, built before any fork.
fn id_maps(pid: libc::pid_t) -> [(CString, Vec<u8>); 3] {
    // SAFETY: getuid and getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let file = |name: &str| CString::new(format!("/proc/{pid}/{name}")).expect("no NUL");
    [
        (file("setgroups"), b"deny".to_vec()),
        (file("uid_map"), format!("{uid} {uid} 1").into_bytes()),
        (file("gid_map"), format!("{gid} {gid} 1").into_bytes()),
    ]
}

/// Writes `data` to `path` with raw calls. Returns 0 or the errno.
fn write_file(path: &CString, data: &[u8]) -> i32 {
    // SAFETY: `path` is NUL-terminated; `data` is a valid buffer.
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return errno();
        }
        let n = libc::write(fd, data.as_ptr().cast(), data.len());
        let e = if n == data.len() as isize { 0 } else { errno() };
        libc::close(fd);
        e
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

/// The stages [`unshare_mapped`] and [`probe`] report, by number.
const STAGES: [&str; 7] = [
    "unshare",
    "setgroups",
    "uid_map",
    "gid_map",
    "mount --make-rprivate /",
    "mount tmpfs",
    "the map helper",
];
const HELPER: usize = 6;

fn pipe() -> Result<(OwnedFd, OwnedFd), i32> {
    let mut fds = [0; 2];
    // SAFETY: a valid array for the two new descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(errno());
    }
    // SAFETY: fresh descriptors owned here.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Moves the calling process (one thread) into a new user namespace (and the other `flags`)
/// with its ids mapped. The maps are written by a forked helper that stays outside: a process
/// may not map uid 0 into a namespace it made itself, and the helper's write is the same for
/// every uid. `Err((stage, errno))` names the failing step of [`STAGES`].
fn unshare_mapped(flags: libc::c_int) -> Result<(), (usize, i32)> {
    // SAFETY: getpid cannot fail.
    let maps = id_maps(unsafe { libc::getpid() });
    let (go_r, go_w) = pipe().map_err(|e| (HELPER, e))?;
    let (res_r, res_w) = pipe().map_err(|e| (HELPER, e))?;
    // SAFETY: the caller is single-threaded; the helper makes raw calls and `_exit`s.
    let helper = unsafe { libc::fork() };
    if helper < 0 {
        return Err((HELPER, errno()));
    }
    if helper == 0 {
        drop(go_w);
        let mut b = 0u8;
        // SAFETY: raw calls on valid buffers and descriptors, then the helper ends.
        unsafe {
            if libc::read(go_r.as_raw_fd(), (&raw mut b).cast(), 1) != 1 {
                libc::_exit(0) // the caller's `unshare` failed
            }
            let mut msg = [0i32; 2];
            for (i, (path, data)) in maps.iter().enumerate() {
                let e = write_file(path, data);
                if e != 0 {
                    msg = [i as i32 + 1, e];
                    break;
                }
            }
            libc::write(res_w.as_raw_fd(), msg.as_ptr().cast(), 8);
            libc::_exit(0)
        }
    }
    drop(go_r);
    drop(res_w);
    let reap = || {
        let mut st = 0;
        // SAFETY: our own child.
        unsafe { libc::waitpid(helper, &mut st, 0) };
    };
    // SAFETY: plain flags.
    if unsafe { libc::unshare(flags) } != 0 {
        let e = errno();
        drop(go_w); // the helper reads end of file and leaves
        reap();
        return Err((0, e));
    }
    let mut msg = [libc::EIO; 2];
    // SAFETY: valid buffers and descriptors.
    let ok = unsafe {
        libc::write(go_w.as_raw_fd(), b"g".as_ptr().cast(), 1) == 1
            && libc::read(res_r.as_raw_fd(), msg.as_mut_ptr().cast(), 8) == 8
    };
    reap();
    match (ok, msg) {
        (true, [0, _]) => Ok(()),
        (true, [stage, e]) => Err((stage as usize, e)),
        (false, _) => Err((HELPER, libc::EIO)),
    }
}

fn reason((stage, e): (usize, i32)) -> String {
    let stage = STAGES.get(stage).copied().unwrap_or("?");
    format!("{stage}: {}", std::io::Error::from_raw_os_error(e))
}

/// Can this process make the namespace? [`probe_outcome`] as `--probe` reports it.
pub fn probe() -> Namespaces {
    match probe_outcome() {
        Outcome::Available => Namespaces::Available,
        Outcome::NotPermitted(reason) | Outcome::Failed(reason) => {
            Namespaces::Unavailable { reason }
        }
    }
}

/// Tries the namespace in a forked child, exactly as [`enter`] makes it: the user, mount and
/// network namespaces, the ids mapped, a private `/` and one `tmpfs` over `/tmp`. The caller
/// must have one thread (the launcher), else this is `Failed`.
pub fn probe_outcome() -> Outcome {
    #[cfg(feature = "test-hooks")]
    if std::env::var(TEST_ENV).as_deref() == Ok("unavailable") {
        return Outcome::NotPermitted("stubbed".into());
    }
    if let Err(e) = single_threaded() {
        return Outcome::Failed(e.to_string());
    }
    let (r, w) = match pipe() {
        Ok(p) => p,
        Err(e) => {
            return Outcome::Failed(format!("pipe: {}", std::io::Error::from_raw_os_error(e)));
        }
    };
    // SAFETY: the caller is single-threaded, so the child may run ordinary code; it `_exit`s.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Outcome::Failed(format!("fork: {}", std::io::Error::last_os_error()));
    }
    if pid == 0 {
        drop(r);
        let result = unshare_mapped(flags(true)).and_then(|()| {
            let null = std::ptr::null::<libc::c_char>();
            // SAFETY: valid NUL-terminated arguments.
            unsafe {
                let root = c"/".as_ptr();
                if libc::mount(
                    null,
                    root,
                    null,
                    libc::MS_REC | libc::MS_PRIVATE,
                    null.cast(),
                ) != 0
                {
                    return Err((4, errno()));
                }
                let tmpfs = c"tmpfs".as_ptr();
                if libc::mount(tmpfs, c"/tmp".as_ptr(), tmpfs, 0, null.cast()) != 0 {
                    return Err((5, errno()));
                }
            }
            Ok(())
        });
        let msg = match result {
            Ok(()) => [-1, 0],
            Err((stage, e)) => [stage as i32, e],
        };
        // SAFETY: a valid buffer and descriptor, then the child ends.
        unsafe {
            libc::write(w.as_raw_fd(), msg.as_ptr().cast(), 8);
            libc::_exit(0)
        }
    }
    drop(w);
    let mut msg = [0i32; 2];
    // SAFETY: a valid buffer; `r` is open.
    let n = unsafe { libc::read(r.as_raw_fd(), msg.as_mut_ptr().cast(), 8) };
    let mut status = 0;
    // SAFETY: our own child.
    unsafe { libc::waitpid(pid, &mut status, 0) };
    match (n, msg) {
        (8, [-1, 0]) => Outcome::Available,
        (8, [stage, e]) if (stage as usize) < HELPER && NOT_PERMITTED.contains(&e) => {
            Outcome::NotPermitted(reason((stage as usize, e)))
        }
        (8, [stage, e]) => Outcome::Failed(reason((stage as usize, e))),
        _ => Outcome::Failed(format!("the probe child ended with status {status}")),
    }
}

/// The calling process has one thread (`unshare(CLONE_NEWUSER)` and a safe `fork` need it).
fn single_threaded() -> Result<(), ApplyError> {
    let text = std::fs::read_to_string("/proc/self/status").map_err(|e| ApplyError::Os {
        call: "/proc/self/status".into(),
        error: e.to_string(),
    })?;
    let threads: usize = text
        .lines()
        .find_map(|l| l.strip_prefix("Threads:"))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0);
    if threads == 1 {
        Ok(())
    } else {
        Err(ApplyError::Threads { threads })
    }
}

/// Puts the calling process (one thread) in the namespace of `layout`, then gives up every
/// capability it holds there (see [`caps`]). Any failure is an error: the caller probed
/// first, so this one refuses the start.
pub fn enter(layout: &Layout) -> Result<(), ApplyError> {
    single_threaded()?;
    let cwd = std::env::current_dir().ok();

    unshare_mapped(flags(layout.net)).map_err(|e| ApplyError::Os {
        call: "the socket namespace".into(),
        error: reason(e),
    })?;
    mount(
        None,
        Path::new("/"),
        None,
        libc::MS_REC | libc::MS_PRIVATE,
        None,
    )?;
    // Granted paths are opened before anything is hidden, inside the new mount namespace (a
    // bind source must be one of its mounts); they are bound back from these.
    let mut kept = Vec::new();
    for k in layout.kept() {
        let c = cstr(k)?;
        // SAFETY: a valid path; the descriptor is owned below.
        let fd = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(ApplyError::Rule {
                path: k.to_path_buf(),
                error: std::io::Error::last_os_error().to_string(),
            });
        }
        // SAFETY: fresh descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let dir = std::fs::metadata(format!("/proc/self/fd/{}", fd.as_raw_fd()))
            .map(|m| m.is_dir())
            .map_err(|e| ApplyError::Rule {
                path: k.to_path_buf(),
                error: e.to_string(),
            })?;
        kept.push((k.to_owned(), fd, dir));
    }
    let tmpfs = Path::new("tmpfs");
    let hidden_flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    let mut devs = Vec::new();
    let mut kept = kept.into_iter();
    for step in &layout.steps {
        match step {
            Step::Hide(h) => {
                mount(
                    Some(tmpfs),
                    h,
                    Some("tmpfs"),
                    hidden_flags,
                    Some(TMPFS_OPTS),
                )?;
                devs.push(dev_of(h)?);
            }
            Step::Keep(k) => {
                let (_, fd, dir) = kept.next().expect("one descriptor per kept path");
                mount_point(k, dir, &devs)?;
                let src = PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()));
                mount(Some(&src), k, None, libc::MS_BIND | libc::MS_REC, None)?;
            }
        }
    }
    for h in layout.hidden() {
        let ro = libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY | hidden_flags;
        mount(None, h, None, ro, None)?;
    }
    // Back to the working directory by path, so it is the namespace's view of it. Where it is
    // hidden now, `/`: the old directory is still the real one, and keeping it would leave its
    // subtree (sockets included) reachable by relative paths.
    let back = cwd.is_some_and(|c| std::env::set_current_dir(c).is_ok());
    if !back {
        std::env::set_current_dir("/").map_err(|e| ApplyError::Os {
            call: "chdir /".into(),
            error: e.to_string(),
        })?;
    }
    caps::no_nested_user_namespaces()?;
    caps::drop_all()
}

fn mount(
    src: Option<&Path>,
    target: &Path,
    fstype: Option<&str>,
    flags: libc::c_ulong,
    data: Option<&str>,
) -> Result<(), ApplyError> {
    let src = src.map(cstr).transpose()?;
    let t = cstr(target)?;
    let fstype = fstype.map(|f| CString::new(f).expect("no NUL"));
    let data = data.map(|d| CString::new(d).expect("no NUL"));
    let p = |c: &Option<CString>| c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
    // SAFETY: valid NUL-terminated strings or null.
    let rc = unsafe { libc::mount(p(&src), t.as_ptr(), p(&fstype), flags, p(&data).cast()) };
    if rc != 0 {
        return Err(ApplyError::Os {
            call: format!("mount {}", target.display()),
            error: std::io::Error::last_os_error().to_string(),
        });
    }
    Ok(())
}

fn dev_of(p: &Path) -> Result<u64, ApplyError> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(p)
        .map(|m| m.dev())
        .map_err(|e| ApplyError::Rule {
            path: p.to_owned(),
            error: e.to_string(),
        })
}

/// Creates `k`'s missing ancestors and `k` itself (an empty directory, or an empty file for a
/// file), each only inside one of the hiding `tmpfs` (`devs`), never on a real filesystem.
fn mount_point(k: &Path, dir: bool, devs: &[u64]) -> Result<(), ApplyError> {
    let mut missing = Vec::new();
    let mut p = k;
    while std::fs::symlink_metadata(p).is_err() {
        missing.push(p);
        p = p.parent().ok_or_else(|| ApplyError::Rule {
            path: k.to_owned(),
            error: "no existing ancestor".into(),
        })?;
    }
    for (i, m) in missing.iter().rev().enumerate() {
        let parent = m.parent().expect("has a parent");
        if !devs.contains(&dev_of(parent)?) {
            return Err(ApplyError::Rule {
                path: (*m).to_owned(),
                error: "a mount point outside the hiding tmpfs".into(),
            });
        }
        let leaf_file = i == missing.len() - 1 && !dir;
        let r = if leaf_file {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(m)
                .map(drop)
        } else {
            std::fs::create_dir(m)
        };
        r.map_err(|e| ApplyError::Rule {
            path: (*m).to_owned(),
            error: e.to_string(),
        })?;
    }
    Ok(())
}
