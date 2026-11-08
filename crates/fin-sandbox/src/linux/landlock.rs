//! Landlock (spec §8.3, plan 7C Decision 10): a policy's enumerated roots become one ruleset
//! at the kernel's ABI, which must be 3 or newer (kernel 6.2, owner decision D31).
//!
//! The rights, per root kind:
//! - **read roots**: `READ_FILE`, `READ_DIR`, and `EXECUTE` for every profile but `worker`
//!   (a worker applies its sandbox after its own `exec` and never starts a program);
//! - **write roots**: the read rights, `WRITE_FILE`, `TRUNCATE`, `REMOVE_*`, `MAKE_DIR`,
//!   `MAKE_REG`, `MAKE_SYM` and `REFER`; `MAKE_SOCK` and `MAKE_FIFO` for `agent` and `hook`;
//!   `EXECUTE` for `agent` only (scripts in its workspace). `REFER` exists only here, so a link
//!   or rename can only move a file between write roots, never in from a read-only, protected
//!   or unlisted tree (the kernel also refuses any move that would gain a right);
//! - **devices**: `READ_FILE` (and `READ_DIR` for a directory), plus `WRITE_FILE` and
//!   `IOCTL_DEV` (ABI ≥ 5) for read-write nodes;
//! - **pathname sockets** (ABI ≥ 9): `RESOLVE_UNIX` on `unix_sockets` only.
//!
//! Every right the ABI knows is handled, so `MAKE_CHAR`, `MAKE_BLOCK` and anything not granted
//! above is refused everywhere. At ABI ≥ 4 TCP is handled too: `connect` only to the ports of
//! [`Net`] (and 80 and 443 for `agent`), `bind` never (for `agent`, `bind` is left to the
//! launcher's task). At ABI ≥ 6 abstract sockets and signals are scoped to the domain.
//!
//! A root is opened `O_PATH | O_NOFOLLOW`; a link is refused (the caller resolved links), and
//! a root that does not exist is skipped: Landlock cannot grant a path that is not there, and
//! nothing can create it later, because its parent is not granted.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use ::landlock::{
    ABI, Access, AccessFs, AccessNet, BitFlags, CompatLevel, Compatible, NetPort, PathBeneath,
    Ruleset, RulesetAttr, RulesetCreated, RulesetCreatedAttr, Scope,
};

use crate::enumerate::{Enumerated, Fs, enumerate};
use crate::error::ApplyError;
use crate::policy::{DeviceAccess, Net, Profile, SandboxPolicy};

/// The lowest ABI a profile is applied at.
pub const MIN_ABI: u32 = 3;
/// What a host below [`MIN_ABI`] lacks (`SANDBOX_UNAVAILABLE.need`).
pub const NEED: &str = "kernel ≥ 6.2";
/// The ABI from which `LANDLOCK_RESTRICT_SELF_TSYNC` restricts every thread.
pub const TSYNC_ABI: u32 = 8;
/// The ports `agent` reaches besides its loopback ports (owner decision 5: open egress).
pub const AGENT_WEB_PORTS: [u16; 2] = [80, 443];

/// A Landlock ABI version as the kernel reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Abi(pub u32);

/// What the kernel's version query answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Abi(Abi),
    /// Landlock is not built in or not enabled (`EOPNOTSUPP`, `ENOSYS`), or a seccomp profile
    /// refuses the call (`EPERM`, Docker's default): F2, spec §8.9.
    Disabled {
        errno: i32,
    },
    /// Any other answer: not understood, so the sandbox fails closed.
    Unknown {
        ret: i64,
        errno: i32,
    },
}

impl Support {
    /// The ABI, or why there is none.
    pub fn abi(self) -> Result<Abi, ApplyError> {
        match self {
            Support::Abi(a) => Ok(a),
            Support::Disabled { errno } => Err(ApplyError::Disabled {
                reason: format!("landlockDisabled: {}", io::Error::from_raw_os_error(errno)),
            }),
            Support::Unknown { ret, errno } => Err(ApplyError::Unavailable {
                need: format!(
                    "a Landlock version query that answers (returned {ret}, errno {errno})"
                ),
            }),
        }
    }
}

/// `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)`. This is the only
/// status source: `PR_GET_SECCOMP` is not consulted (Docker reports 2 there while Landlock
/// works; S9).
pub fn query() -> Support {
    // SAFETY: with a null attribute, size 0 and the VERSION flag the call reads nothing and
    // only returns the ABI or an error.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            1u32, // LANDLOCK_CREATE_RULESET_VERSION
        )
    };
    let errno = if ret < 0 {
        io::Error::last_os_error().raw_os_error().unwrap_or(0)
    } else {
        0
    };
    classify(ret, errno)
}

/// The query's answer, classified (pure, for tests).
pub fn classify(ret: i64, errno: i32) -> Support {
    if ret > 0 {
        return match u32::try_from(ret) {
            Ok(v) => Support::Abi(Abi(v)),
            Err(_) => Support::Unknown { ret, errno },
        };
    }
    match errno {
        libc::EPERM | libc::ENOSYS | libc::EOPNOTSUPP => Support::Disabled { errno },
        _ => Support::Unknown { ret, errno },
    }
}

fn os(call: &str) -> impl FnOnce(::landlock::RulesetError) -> ApplyError + '_ {
    move |e| ApplyError::Os {
        call: call.into(),
        error: e.to_string(),
    }
}

/// The ruleset of `policy` at `abi`: validates and enumerates the policy against the real
/// filesystem, then [`ruleset_from`].
pub fn ruleset(policy: &SandboxPolicy, abi: Abi) -> Result<RulesetCreated, ApplyError> {
    if abi.0 < MIN_ABI {
        return Err(unavailable());
    }
    let roots = enumerate(policy, &Fs)?;
    ruleset_from(policy, &roots, abi)
}

fn unavailable() -> ApplyError {
    ApplyError::Unavailable { need: NEED.into() }
}

/// The ruleset for already enumerated roots. Every requested right must be supported at
/// `abi` (`HardRequirement`): an ABI the kernel does not have fails here, never silently.
pub fn ruleset_from(
    policy: &SandboxPolicy,
    roots: &Enumerated,
    abi: Abi,
) -> Result<RulesetCreated, ApplyError> {
    if abi.0 < MIN_ABI {
        return Err(unavailable());
    }
    let v = ABI::from(i32::try_from(abi.0).unwrap_or(i32::MAX));
    let handled = AccessFs::from_all(v);
    let mut rs = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(handled)
        .map_err(os("handle filesystem access"))?;
    if abi.0 >= 4 {
        rs = rs
            .handle_access(net_handled(policy.profile))
            .map_err(os("handle TCP access"))?;
    }
    if abi.0 >= 6 {
        rs = rs.scope(Scope::from_all(v)).map_err(os("scope"))?;
    }
    let mut rs = rs.create().map_err(os("landlock_create_ruleset"))?;
    let read = read_access(policy.profile) & handled;
    let write = write_access(policy.profile) & handled;
    for p in &roots.read {
        rs = add_path(rs, p, read, v)?;
    }
    for p in &roots.write {
        rs = add_path(rs, p, write, v)?;
    }
    for d in &policy.devices {
        rs = add_path(rs, &d.path, device_access(d.access) & handled, v)?;
    }
    if abi.0 >= 9 {
        for s in &policy.unix_sockets {
            rs = add_path(rs, s, AccessFs::ResolveUnix.into(), v)?;
        }
    }
    if abi.0 >= 4 {
        for port in connect_ports(policy) {
            rs = rs
                .add_rule(NetPort::new(port, AccessNet::ConnectTcp))
                .map_err(os("add a TCP rule"))?;
        }
    }
    Ok(rs)
}

/// Read roots: read, and execute for the profiles that start programs.
pub fn read_access(profile: Profile) -> BitFlags<AccessFs> {
    let r = AccessFs::ReadFile | AccessFs::ReadDir;
    if profile == Profile::Worker {
        r
    } else {
        r | AccessFs::Execute
    }
}

/// Write roots (see the module documentation).
pub fn write_access(profile: Profile) -> BitFlags<AccessFs> {
    let mut w = AccessFs::ReadFile
        | AccessFs::ReadDir
        | AccessFs::WriteFile
        | AccessFs::Truncate
        | AccessFs::RemoveDir
        | AccessFs::RemoveFile
        | AccessFs::MakeDir
        | AccessFs::MakeReg
        | AccessFs::MakeSym
        | AccessFs::Refer;
    if matches!(profile, Profile::Agent | Profile::Hook) {
        w |= AccessFs::MakeSock | AccessFs::MakeFifo;
    }
    if profile == Profile::Agent {
        w |= AccessFs::Execute;
    }
    w
}

fn device_access(a: DeviceAccess) -> BitFlags<AccessFs> {
    let r = AccessFs::ReadFile | AccessFs::ReadDir;
    match a {
        DeviceAccess::Read => r,
        DeviceAccess::ReadWrite => r | AccessFs::WriteFile | AccessFs::IoctlDev,
    }
}

fn net_handled(profile: Profile) -> BitFlags<AccessNet> {
    if profile == Profile::Agent {
        AccessNet::ConnectTcp.into()
    } else {
        AccessNet::BindTcp | AccessNet::ConnectTcp
    }
}

/// The TCP ports `connect` may reach (ABI ≥ 4).
pub fn connect_ports(policy: &SandboxPolicy) -> Vec<u16> {
    let mut ports = match &policy.net {
        Net::Open { loopback } => loopback.iter().copied().chain(AGENT_WEB_PORTS).collect(),
        Net::Loopback { ports } => ports.clone(),
        Net::None => Vec::new(),
    };
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Adds `path` with `access` (masked to a file's rights when it is not a directory).
fn add_path(
    rs: RulesetCreated,
    path: &Path,
    access: BitFlags<AccessFs>,
    v: ABI,
) -> Result<RulesetCreated, ApplyError> {
    let rule_err = |error: String| ApplyError::Rule {
        path: path.to_path_buf(),
        error,
    };
    let fd = match open_path(path) {
        Ok(fd) => fd,
        Err(e)
            if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENOTDIR) =>
        {
            return Ok(rs);
        }
        Err(e) => return Err(rule_err(e.to_string())),
    };
    // SAFETY: `st` is written by fstat before it is read.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: a valid descriptor and a valid out pointer.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(rule_err(io::Error::last_os_error().to_string()));
    }
    let access = match st.st_mode & libc::S_IFMT {
        libc::S_IFDIR => access,
        libc::S_IFLNK => return Err(rule_err("is a symbolic link".into())),
        _ => access & AccessFs::from_file(v),
    };
    if access.is_empty() {
        return Ok(rs);
    }
    rs.add_rule(PathBeneath::new(fd, access))
        .map_err(|e| rule_err(e.to_string()))
}

/// `open(path, O_PATH | O_NOFOLLOW | O_CLOEXEC)`.
fn open_path(path: &Path) -> io::Result<OwnedFd> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL in the path"))?;
    // SAFETY: a valid C string; the descriptor is owned below.
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh descriptor nobody else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
