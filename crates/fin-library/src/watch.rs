//! Watches of a local library root on Linux (plan 5D Decision 30; the Tasks 9–11 review): one
//! inotify watch per **directory**, on the root's mount only, in a thread of its own.
//!
//! - The walk lists names with `getdents64` and stats each with `statx(AT_SYMLINK_NOFOLLOW |
//!   AT_NO_AUTOMOUNT)`, so no link is followed and no automount point is triggered; a
//!   directory on another mount (a nested mount, a bind mount) is not descended into, and
//!   dot-directories are never watched (Decision 16). Each directory is opened `O_NOFOLLOW`
//!   from its parent and watched through its descriptor (`/proc/self/fd/<fd>`), never by a
//!   path string.
//! - At most `max_watches` directories are watched: one more fails the watch with
//!   `WATCH_LIMIT`. An `inotify_add_watch` the kernel refuses (`ENOSPC` past
//!   `max_user_watches`, `EACCES`) and any other walk failure fail it with that code; a watch
//!   never goes on partially.
//! - Events name the directory whose entries changed ([`WatchMsg::Dirs`], one message per
//!   read of the inotify queue, names starting with `.` dropped). A directory created or moved
//!   in is walked and watched; one moved out loses its watches. A queue overflow re-walks the
//!   tree and asks for a whole-library poll ([`WatchMsg::All`]). The root removed, moved or
//!   unmounted fails the watch (`WATCH_ROOT_GONE`).
//!
//! Nothing here opens a file or writes anything under the root.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;

use crate::raw::RawRel;
use crate::source::linux::{ENTRY_FLAGS, attr, file_type, names, same_mount, statx, statx_fd};
use crate::source::unix::{cname, cvt, open_dir_at, open_root, retry};
use crate::types::EntryType;

/// What a watch tells its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchMsg {
    /// The first walk finished: this many directories are watched.
    Ready { watches: usize },
    /// Directories whose entries changed (no duplicates).
    Dirs(Vec<RawRel>),
    /// Events were lost (the queue overflowed): poll the whole library.
    All,
    /// The watch failed and stopped (`WATCH_LIMIT`, `ENOSPC`, `EACCES`, `WATCH_ROOT_GONE`, …).
    Failed(String),
}

impl WatchMsg {
    /// The JSON the binding hands to JS: `{ready}`, `{dirs: [RawRel]}`, `{all: true}`, `{failed}`.
    pub fn to_json(&self) -> String {
        match self {
            WatchMsg::Ready { watches } => json!({ "ready": watches }),
            WatchMsg::Dirs(dirs) => json!({ "dirs": dirs }),
            WatchMsg::All => json!({ "all": true }),
            WatchMsg::Failed(code) => json!({ "failed": code }),
        }
        .to_string()
    }
}

/// A running watch; [`DirWatch::close`] (or drop) stops it without waiting for its thread.
#[derive(Debug)]
pub struct DirWatch {
    stop: Arc<AtomicBool>,
    wake: Arc<OwnedFd>,
}

impl DirWatch {
    /// Starts watching `root` (the library's canonical root) on a new thread. Everything that
    /// can fail (the root's open, the walk) is reported through `sink` as [`WatchMsg::Failed`].
    pub fn start(
        root: PathBuf,
        max_watches: usize,
        sink: Box<dyn FnMut(WatchMsg) + Send>,
    ) -> io::Result<Self> {
        // SAFETY: plain syscall; the descriptor is owned below.
        let wake = cvt(unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) })?;
        // SAFETY: `wake` was just created and is owned by nobody else.
        let wake = Arc::new(unsafe { OwnedFd::from_raw_fd(wake) });
        let stop = Arc::new(AtomicBool::new(false));
        let (s, w) = (stop.clone(), wake.clone());
        std::thread::Builder::new()
            .name("fin-library-watch".into())
            .spawn(move || run(root, max_watches, s, w, sink))?;
        Ok(DirWatch { stop, wake })
    }

    /// Stops the watch: the thread exits at its next step and nothing more is sent.
    pub fn close(&self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        let one: u64 = 1;
        // SAFETY: writes 8 bytes from a valid u64 to our eventfd.
        unsafe { libc::write(self.wake.as_raw_fd(), (&one as *const u64).cast(), 8) };
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        self.close();
    }
}

const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_MODIFY
    | libc::IN_CLOSE_WRITE
    | libc::IN_ATTRIB
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR
    | libc::IN_EXCL_UNLINK;

/// The code a failure is reported with: the errno's name for the usual ones.
fn code_of(e: &io::Error) -> String {
    let name = match e.raw_os_error() {
        Some(libc::ENOSPC) => "ENOSPC",
        Some(libc::EACCES) => "EACCES",
        Some(libc::EPERM) => "EPERM",
        Some(libc::EMFILE) => "EMFILE",
        Some(libc::ENFILE) => "ENFILE",
        Some(libc::ENOMEM) => "ENOMEM",
        Some(libc::EIO) => "EIO",
        Some(libc::ENOENT) => "WATCH_ROOT_GONE",
        _ => "WATCH_ERROR",
    };
    name.into()
}

/// Gone since it was listed (or no longer a directory): skipped, never a failure.
fn vanished(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP)
    )
}

struct Tree {
    inotify: OwnedFd,
    root: PathBuf,
    root_st: libc::statx,
    wds: HashMap<i32, RawRel>,
    max: usize,
    stop: Arc<AtomicBool>,
    noatime: AtomicBool,
}

fn run(
    root: PathBuf,
    max: usize,
    stop: Arc<AtomicBool>,
    wake: Arc<OwnedFd>,
    mut sink: Box<dyn FnMut(WatchMsg) + Send>,
) {
    let mut send = |m: WatchMsg, stop: &AtomicBool| {
        if !stop.load(Ordering::SeqCst) {
            sink(m);
        }
    };
    let mut tree = match Tree::open(root, max, stop.clone()) {
        Ok(t) => t,
        Err(e) => return send(WatchMsg::Failed(code_of(&e)), &stop),
    };
    if let Err(code) = tree.walk(RawRel::root()) {
        return send(WatchMsg::Failed(code), &stop);
    }
    send(
        WatchMsg::Ready {
            watches: tree.wds.len(),
        },
        &stop,
    );
    let mut buf = vec![0u8; 64 * 1024];
    while !stop.load(Ordering::SeqCst) {
        let mut fds = [
            libc::pollfd {
                fd: tree.inotify.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: `fds` is a valid array of two pollfds.
        if let Err(e) = retry(|| cvt(unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) })) {
            return send(WatchMsg::Failed(code_of(&e)), &stop);
        }
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if fds[0].revents & libc::POLLIN == 0 {
            continue;
        }
        // SAFETY: reads into `buf`, which is valid for `buf.len()` bytes.
        let n = unsafe { libc::read(tree.inotify.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if matches!(e.raw_os_error(), Some(libc::EAGAIN | libc::EINTR)) {
                continue;
            }
            return send(WatchMsg::Failed(code_of(&e)), &stop);
        }
        match tree.events(&buf[..n as usize]) {
            Ok(msgs) => msgs.into_iter().for_each(|m| send(m, &stop)),
            Err(code) => return send(WatchMsg::Failed(code), &stop),
        }
    }
}

impl Tree {
    fn open(root: PathBuf, max: usize, stop: Arc<AtomicBool>) -> io::Result<Self> {
        // SAFETY: plain syscall; the descriptor is owned below.
        let fd = cvt(unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) })?;
        // SAFETY: `fd` was just created and is owned by nobody else.
        let inotify = unsafe { OwnedFd::from_raw_fd(fd) };
        let root_st = statx_fd(open_root(&root)?.as_raw_fd())?;
        Ok(Tree {
            inotify,
            root,
            root_st,
            wds: HashMap::new(),
            max,
            stop,
            noatime: AtomicBool::new(false),
        })
    }

    /// Opens the directory at `rel` from the root, component by component, following no link;
    /// `None` when it is gone or is not on the root's mount. The root is opened by its path
    /// each time (a held descriptor would keep a removed root's inode, and its watch, alive);
    /// one that is no longer the same directory is gone (`ENOENT`).
    fn open_rel(&self, rel: &RawRel) -> io::Result<Option<OwnedFd>> {
        let root = open_root(&self.root)?;
        let st = statx_fd(root.as_raw_fd())?;
        if st.stx_ino != self.root_st.stx_ino || !same_mount(&self.root_st, &st) {
            return Err(io::Error::from_raw_os_error(libc::ENOENT));
        }
        let mut fd = root;
        for c in rel.components() {
            match open_dir_at(fd.as_raw_fd(), c, &self.noatime) {
                Ok(f) => fd = f,
                Err(e) if vanished(&e) => return Ok(None),
                Err(e) => return Err(e),
            }
        }
        let st = statx_fd(fd.as_raw_fd())?;
        Ok(same_mount(&self.root_st, &st).then_some(fd))
    }

    fn add(&mut self, fd: &OwnedFd, rel: &RawRel) -> Result<(), String> {
        let path = CString::new(format!("/proc/self/fd/{}", fd.as_raw_fd()))
            .map_err(|_| "WATCH_ERROR".to_string())?;
        // SAFETY: `path` is a valid C string; the watch descriptor is tracked in `wds`.
        let wd =
            cvt(unsafe { libc::inotify_add_watch(self.inotify.as_raw_fd(), path.as_ptr(), MASK) })
                .map_err(|e| code_of(&e))?;
        if self.wds.insert(wd, rel.clone()).is_none() && self.wds.len() > self.max {
            return Err("WATCH_LIMIT".into());
        }
        Ok(())
    }

    /// Watches `start` and every directory below it on the root's mount (not dot-directories).
    fn walk(&mut self, start: RawRel) -> Result<(), String> {
        let mut stack = vec![start];
        while let Some(rel) = stack.pop() {
            if self.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            let fd = match self.open_rel(&rel) {
                Ok(Some(fd)) => fd,
                Ok(None) if rel.is_root() => return Err("WATCH_ROOT_GONE".into()),
                Ok(None) => continue,
                Err(e) => return Err(code_of(&e)),
            };
            self.add(&fd, &rel)?;
            let listed = match names(&fd) {
                Ok(n) => n,
                Err(e) if vanished(&e) && !rel.is_root() => continue,
                Err(e) => return Err(code_of(&e)),
            };
            for name in listed {
                if name.first() == Some(&b'.') {
                    continue;
                }
                let Ok(c) = cname(&name) else { continue };
                let st = match statx(fd.as_raw_fd(), &c, ENTRY_FLAGS) {
                    Ok(st) => st,
                    Err(e) if vanished(&e) => continue,
                    Err(e) => return Err(code_of(&e)),
                };
                if file_type(&st) != EntryType::Dir
                    || attr(&st, libc::STATX_ATTR_AUTOMOUNT)
                    || !same_mount(&self.root_st, &st)
                {
                    continue;
                }
                stack.push(rel.join(&name));
            }
        }
        Ok(())
    }

    /// Stops watching `rel` and everything below it (a directory moved out).
    fn forget(&mut self, rel: &RawRel) {
        let gone: Vec<i32> = self
            .wds
            .iter()
            .filter(|(_, r)| r.starts_with(rel))
            .map(|(wd, _)| *wd)
            .collect();
        for wd in gone {
            self.wds.remove(&wd);
            // SAFETY: plain syscall on our inotify descriptor.
            unsafe { libc::inotify_rm_watch(self.inotify.as_raw_fd(), wd) };
        }
    }

    /// One read of the queue: what to tell the owner.
    fn events(&mut self, buf: &[u8]) -> Result<Vec<WatchMsg>, String> {
        const HEAD: usize = std::mem::size_of::<libc::inotify_event>();
        let mut dirs: Vec<RawRel> = Vec::new();
        let mut seen: HashSet<RawRel> = HashSet::new();
        let mut walk: Vec<RawRel> = Vec::new();
        let mut overflow = false;
        let mut at = 0;
        while at + HEAD <= buf.len() {
            // SAFETY: the kernel writes whole events; `read_unaligned` copies the header out.
            let ev: libc::inotify_event =
                unsafe { std::ptr::read_unaligned(buf[at..].as_ptr().cast()) };
            let len = ev.len as usize;
            let raw = &buf[at + HEAD..(at + HEAD + len).min(buf.len())];
            at += HEAD + len;
            let name = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
            if ev.mask & libc::IN_Q_OVERFLOW != 0 {
                overflow = true;
                continue;
            }
            let Some(rel) = self.wds.get(&ev.wd).cloned() else {
                continue;
            };
            if ev.mask & libc::IN_IGNORED != 0 {
                self.wds.remove(&ev.wd);
                if rel.is_root() {
                    return Err("WATCH_ROOT_GONE".into());
                }
                continue;
            }
            if ev.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_UNMOUNT) != 0 {
                if rel.is_root() {
                    return Err("WATCH_ROOT_GONE".into());
                }
                continue; // its parent reports it
            }
            if name.first() == Some(&b'.') {
                continue;
            }
            if seen.insert(rel.clone()) {
                dirs.push(rel.clone());
            }
            if ev.mask & libc::IN_ISDIR != 0 && !name.is_empty() {
                let child = rel.join(name);
                if ev.mask & libc::IN_MOVED_FROM != 0 {
                    self.forget(&child);
                }
                if ev.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
                    walk.push(child);
                }
            }
        }
        for child in walk {
            self.walk(child)?;
        }
        if overflow {
            self.walk(RawRel::root())?;
            return Ok(vec![WatchMsg::All]);
        }
        Ok(if dirs.is_empty() {
            vec![]
        } else {
            vec![WatchMsg::Dirs(dirs)]
        })
    }
}
