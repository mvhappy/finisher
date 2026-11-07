//! The delegated cgroup v2 kill domain (Decision 16, Linux).
//!
//! Used only when `/proc/self/cgroup` names a v2 cgroup `<cg>` that we can write, whose
//! `cgroup.procs` holds only this process, and that has `cgroup.kill` (kernel ≥ 5.14): a
//! systemd user service with `Delegate=yes`, or a container with its own cgroup namespace.
//! A terminal's scope (the shell lives there too) is never rearranged. The host then:
//! - moves itself into `<cg>/host` and enables the available `+memory +pids` controllers on
//!   `<cg>` and `<cg>/jobs` (v2 forbids processes in a cgroup whose controllers are delegated
//!   to children, hence the `host` leaf);
//! - at the first start in this process, kills (`cgroup.kill`) and removes leftover
//!   `<cg>/jobs/g*` directories of a previous run;
//! - per group makes `<cg>/jobs/g<n>` (`memory.max` from the group's limit, `memory.swap.max`
//!   0 so the limit cannot be passed by swapping, `memory.oom.group` 1) and per process a
//!   leaf `g<n>/p<k>` (`pids.max` from `job_limits.active_processes`). The child writes `0`
//!   into its leaf's `cgroup.procs` before `exec`.
//! - `pids.max` counts tasks (threads too), unlike a Job's active-process limit, so a limit of
//!   `n` processes becomes `n × TASKS_PER_PROCESS` tasks: it stops a fork storm, not the
//!   `n + 1`-th process.
//!
//! Kill is `cgroup.kill` on the group, which takes processes that left their process group.
//! An OOM is the group's `memory.events` `oom_kill > 0`. The watchdog gets `+c`/`-c` lines.
//! `<n>` is unique in this process (two hosts in one process do not collide).

use std::fs;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::domain::{ExitFacts, GroupHandle, KillDomain, LeafHandle, Placement};
use crate::spawner::JobLimits;
use crate::unix::foreign::Seen;
use crate::unix::watchdog::Watchdog;
use crate::{GroupId, GroupSpec, KillTree, LogFn};

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);
/// The tasks allowed per process of `job_limits.active_processes` (see the module doc).
pub(crate) const TASKS_PER_PROCESS: u32 = 256;
static SWEPT: std::sync::Once = std::sync::Once::new();

/// A delegated cgroup we may arrange.
#[derive(Debug, Clone)]
pub(crate) struct CgroupRoot {
    pub dir: PathBuf,
    pub memory: bool,
    pub pids: bool,
}

/// Where the cgroup2 hierarchy is mounted.
fn mount_point() -> PathBuf {
    fs::read_to_string("/proc/self/mounts")
        .ok()
        .and_then(|m| {
            m.lines()
                .map(|l| l.split(' ').collect::<Vec<_>>())
                .find(|f| f.len() > 2 && f[2] == "cgroup2")
                .map(|f| PathBuf::from(f[1]))
        })
        .unwrap_or_else(|| PathBuf::from("/sys/fs/cgroup"))
}

fn procs(dir: &Path) -> io::Result<Vec<u32>> {
    Ok(fs::read_to_string(dir.join("cgroup.procs"))?
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect())
}

fn parent_of(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?.1.split(' ').nth(1)?.parse().ok()
}

fn writable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: access only reads the path.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Decision 16's conditions, or why they do not hold.
pub(crate) fn detect() -> Result<CgroupRoot, String> {
    let own = fs::read_to_string("/proc/self/cgroup").map_err(|e| format!("no cgroup: {e}"))?;
    let rel = own
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .ok_or("not on cgroup v2")?;
    let here = mount_point().join(rel.trim_start_matches('/'));
    let me = std::process::id();
    let mine = |dir: &Path| -> Result<bool, String> {
        let p = procs(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(p.iter().all(|&p| p == me || parent_of(p) == Some(me)) && p.contains(&me))
    };
    // A second host in this process finds itself in `<cg>/host` already.
    let dir = match here.parent() {
        Some(parent) if here.ends_with("host") && parent.join("jobs").is_dir() => {
            if !mine(&here)? || !procs(parent).map_err(|e| e.to_string())?.is_empty() {
                return Err(format!("{} is shared", here.display()));
            }
            parent.to_path_buf()
        }
        _ => {
            if procs(&here).map_err(|e| e.to_string())? != [me] {
                return Err(format!("{} holds other processes", here.display()));
            }
            here
        }
    };
    if !dir.join("cgroup.kill").exists() {
        return Err("no cgroup.kill (kernel < 5.14)".into());
    }
    let files = ["cgroup.procs", "cgroup.subtree_control"];
    if !writable(&dir) || files.iter().any(|f| !writable(&dir.join(f))) {
        return Err(format!("{} is not delegated to us", dir.display()));
    }
    let controllers = fs::read_to_string(dir.join("cgroup.controllers")).unwrap_or_default();
    let has = |c: &str| controllers.split_whitespace().any(|x| x == c);
    Ok(CgroupRoot {
        dir,
        memory: has("memory"),
        pids: has("pids"),
    })
}

/// `detect` with its reason logged; `None` means process groups.
pub(crate) fn detect_cgroup(log: &LogFn) -> Option<CgroupRoot> {
    detect()
        .inspect_err(|why| log(&format!("cgroups not used: {why}")))
        .ok()
}

fn mkdir(dir: &Path) -> io::Result<()> {
    match fs::create_dir(dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

impl CgroupRoot {
    fn jobs(&self) -> PathBuf {
        self.dir.join("jobs")
    }

    fn controllers(&self) -> String {
        let c = [(self.memory, "+memory"), (self.pids, "+pids")];
        c.iter()
            .filter(|c| c.0)
            .map(|c| c.1)
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn enable(&self, dir: &Path) -> io::Result<()> {
        let c = self.controllers();
        if c.is_empty() {
            return Ok(());
        }
        fs::write(dir.join("cgroup.subtree_control"), c)
    }

    /// Moves this process into `<cg>/host`, enables the controllers on `<cg>` and `<cg>/jobs`.
    pub fn setup(&self) -> io::Result<()> {
        let host = self.dir.join("host");
        mkdir(&host)?;
        fs::write(host.join("cgroup.procs"), std::process::id().to_string())?;
        self.enable(&self.dir)?;
        mkdir(&self.jobs())?;
        self.enable(&self.jobs())
    }
}

/// Kills and removes `dir` and its sub-cgroups (deepest first). A cgroup that keeps a
/// process past `wait` stays.
fn kill_and_remove(dir: &Path, wait: Duration) -> io::Result<()> {
    let _ = fs::write(dir.join("cgroup.kill"), b"1");
    let until = Instant::now() + wait;
    for sub in fs::read_dir(dir)?.flatten() {
        if sub.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = kill_and_remove(&sub.path(), until.saturating_duration_since(Instant::now()));
        }
    }
    loop {
        match fs::remove_dir(dir) {
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(5));
            }
            r => return r,
        }
    }
}

/// Kills and removes every leftover `<cg>/jobs/g*`; returns how many were removed.
pub(crate) fn sweep_leftover_groups(root: &CgroupRoot) -> io::Result<usize> {
    let mut n = 0;
    for e in fs::read_dir(root.jobs())?.flatten() {
        let name = e.file_name();
        if name.to_string_lossy().starts_with('g') && e.file_type().is_ok_and(|t| t.is_dir()) {
            kill_and_remove(&e.path(), Duration::from_secs(2))?;
            n += 1;
        }
    }
    Ok(n)
}

pub(crate) struct Cgroups {
    root: CgroupRoot,
    watchdog: Option<Arc<Watchdog>>,
    log: LogFn,
}

struct CgGroup {
    dir: PathBuf,
    seen: Seen,
    next_leaf: AtomicU64,
}

struct CgLeaf {
    dir: PathBuf,
}

fn group(g: &GroupHandle) -> &CgGroup {
    g.os.downcast_ref().expect("a cgroup handle")
}

fn read_key(file: &Path, key: &str) -> Option<u64> {
    fs::read_to_string(file).ok()?.lines().find_map(|l| {
        let (k, v) = l.split_once(' ')?;
        (k == key).then(|| v.trim().parse().ok()).flatten()
    })
}

fn read_u64(file: &Path) -> Option<u64> {
    fs::read_to_string(file).ok()?.trim().parse().ok()
}

/// Every pid in `dir` and below.
fn members(dir: &Path, out: &mut Vec<u32>) {
    out.extend(procs(dir).unwrap_or_default());
    for sub in fs::read_dir(dir).into_iter().flatten().flatten() {
        if sub.file_type().is_ok_and(|t| t.is_dir()) {
            members(&sub.path(), out);
        }
    }
}

impl Cgroups {
    /// Sets the root up (Decision 16) and sweeps leftovers once per process.
    pub fn new(root: CgroupRoot, log: LogFn) -> io::Result<Cgroups> {
        root.setup()?;
        let mut swept = Ok(0);
        SWEPT.call_once(|| swept = sweep_leftover_groups(&root));
        match swept {
            Ok(n) if n > 0 => log(&format!("removed {n} leftover cgroup(s)")),
            Ok(_) => {}
            Err(e) => log(&format!("leftover cgroups not removed: {e}")),
        }
        Ok(Cgroups {
            root,
            watchdog: None,
            log,
        })
    }

    pub fn with_watchdog(mut self, w: Option<Arc<Watchdog>>) -> Cgroups {
        self.watchdog = w;
        self
    }

    fn scan(&self, g: &GroupHandle) -> u32 {
        let st = group(g);
        let mut pids = Vec::new();
        members(&st.dir, &mut pids);
        st.seen.note(pids)
    }
}

impl KillDomain for Cgroups {
    fn kill_tree(&self) -> KillTree {
        KillTree::Cgroup
    }

    fn leaf_limits(&self) -> bool {
        self.root.pids
    }

    fn kernel_memory(&self) -> bool {
        self.root.memory
    }

    fn watchdog(&self) -> Option<&Watchdog> {
        self.watchdog.as_deref()
    }

    fn open_group(&self, id: GroupId, spec: &GroupSpec) -> io::Result<GroupHandle> {
        let dir = self
            .root
            .jobs()
            .join(format!("g{}", NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&dir)?;
        let made = (|| {
            if let (true, Some(bytes)) = (self.root.memory, spec.memory_bytes) {
                fs::write(dir.join("memory.max"), bytes.to_string())?;
                // Absent without swap accounting.
                let _ = fs::write(dir.join("memory.swap.max"), "0");
                fs::write(dir.join("memory.oom.group"), "1")?;
            }
            self.root.enable(&dir)
        })();
        if let Err(e) = made {
            let _ = fs::remove_dir(&dir);
            return Err(e);
        }
        if let Some(w) = &self.watchdog {
            w.add_cgroup(&dir);
        }
        let st = CgGroup {
            dir,
            seen: Seen::default(),
            next_leaf: AtomicU64::new(1),
        };
        Ok(GroupHandle::new(id, spec, Box::new(st)))
    }

    fn open_leaf(&self, g: &GroupHandle, limits: &JobLimits) -> io::Result<LeafHandle> {
        let st = group(g);
        let dir = st
            .dir
            .join(format!("p{}", st.next_leaf.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&dir)?;
        if let (true, Some(n)) = (self.root.pids, limits.active_processes)
            && let Err(e) = fs::write(
                dir.join("pids.max"),
                n.saturating_mul(TASKS_PER_PROCESS).to_string(),
            )
        {
            let _ = fs::remove_dir(&dir);
            return Err(e);
        }
        Ok(LeafHandle {
            os: Box::new(CgLeaf { dir }),
        })
    }

    fn place(&self, _g: &GroupHandle, leaf: &LeafHandle, p: &mut Placement) -> io::Result<()> {
        let leaf: &CgLeaf = leaf.os.downcast_ref().expect("a cgroup leaf");
        let fd: OwnedFd = fs::OpenOptions::new()
            .write(true)
            .open(leaf.dir.join("cgroup.procs"))?
            .into();
        p.child_writes.push((fd, b"0"));
        Ok(())
    }

    fn placed(&self, g: &GroupHandle, _leaf: &LeafHandle, pid: u32) -> io::Result<()> {
        group(g).seen.spawned(pid);
        Ok(())
    }

    fn kill(&self, g: &GroupHandle) -> io::Result<()> {
        self.scan(g);
        fs::write(group(g).dir.join("cgroup.kill"), b"1")
    }

    fn memory(&self, g: &GroupHandle) -> Option<u64> {
        if !self.root.memory {
            return None;
        }
        read_u64(&group(g).dir.join("memory.current"))
    }

    fn foreign(&self, g: &GroupHandle, _spawned: u32) -> Option<u32> {
        Some(self.scan(g))
    }

    fn exit_facts(&self, g: &GroupHandle) -> ExitFacts {
        let dir = &group(g).dir;
        ExitFacts {
            oom: read_key(&dir.join("memory.events"), "oom_kill").is_some_and(|n| n > 0),
            total_processes: None,
            memory_peak: read_u64(&dir.join("memory.peak")),
        }
    }

    fn close_group(&self, g: GroupHandle) {
        let dir = group(&g).dir.clone();
        match kill_and_remove(&dir, Duration::from_millis(200)) {
            Ok(()) => {
                if let Some(w) = &self.watchdog {
                    w.remove_cgroup(&dir);
                }
            }
            // Still listed: the watchdog kills it if the host dies; the next start sweeps it.
            Err(e) => (self.log)(&format!("{}: not removed: {e}", dir.display())),
        }
    }
}
