//! The one pure sandbox policy (spec §8.1, plan 7C Decision 1).
//!
//! A [`SandboxPolicy`] is built only by the profile builders of [`crate::profile`] and applied
//! by the OS layers. It names paths, never secrets. Its semantics are defined here, once, by
//! [`SandboxPolicy::readable`] and [`SandboxPolicy::writable`] (the reference model): the most
//! specific root wins, and on a tie the denial wins. So `deny_read` takes a subtree out of a
//! read root, and a read or write root *inside* a denied root (the session directory inside
//! server data) is a hole the policy grants explicitly. Landlock and AppContainer only add
//! access, so [`crate::enumerate`] turns the policy into roots that hold no exclusion; Seatbelt
//! (last match wins) states the same rules in order. Both are tested against this model.

use std::path::{Component, Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::PolicyError;

/// The four profiles (spec §8.10). `SandboxProfile` in TypeScript.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "SandboxProfile")]
pub enum Profile {
    Agent,
    Hook,
    Worker,
    Plugin,
}

impl Profile {
    pub const ALL: [Profile; 4] = [
        Profile::Agent,
        Profile::Hook,
        Profile::Worker,
        Profile::Plugin,
    ];
}

/// The three supported OSes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    /// The OS this crate was built for (every other Unix counts as Linux).
    pub const fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// What a process may read: always an allow-list (spec §8.1, revision 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReadPolicy {
    Only(Vec<PathBuf>),
    /// Everything but `except` (and `deny_read`): escape-suite controls only, never shipped.
    #[cfg(any(test, feature = "test-hooks"))]
    Everything {
        except: Vec<PathBuf>,
    },
}

/// IP networking. Pathname sockets are `unix_sockets`; Windows pipes are the pipe's DACL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Net {
    /// Outbound IP open (the `agent` profile, owner decision 5) except loopback, which reaches
    /// only these ports (Finisher's agent listener on Linux; spec §8.3, §8.7). Linux at Landlock
    /// ABI ≥ 4 also limits TCP `connect` to these ports plus 80 and 443.
    Open { loopback: Vec<u16> },
    /// No outbound IP but loopback to these ports (a `hook` that declares `fin`, Linux).
    Loopback { ports: Vec<u16> },
    /// No IP at all.
    None,
}

/// A device node a profile may open (spec §8.10 GPU rows).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceRule {
    pub path: PathBuf,
    pub access: DeviceAccess,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DeviceAccess {
    Read,
    ReadWrite,
}

/// Resource limits (plan 7C Decision 4a). `None` leaves the limit to the host's other
/// mechanisms (5C's group memory limit, liveness).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub processes: Option<u32>,
    pub memory: Option<u64>,
    pub open_files: Option<u32>,
    pub wall_ms: Option<u64>,
}

/// The policy of spec §8.1. Paths are absolute and normalised; the caller resolved links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SandboxPolicy {
    pub profile: Profile,
    /// Read and write (a write root is readable too).
    pub write: Vec<PathBuf>,
    pub read: ReadPolicy,
    /// Never writable and never linkable (to or from), wherever they sit.
    pub protect: Vec<PathBuf>,
    /// A second layer inside `read`: neither readable nor writable, except a root granted
    /// inside one (a hole, see the module documentation).
    pub deny_read: Vec<PathBuf>,
    /// The only pathname sockets the process may connect to (macOS: `run/fin.sock`).
    pub unix_sockets: Vec<PathBuf>,
    pub net: Net,
    /// macOS `mach-lookup` allow-list (empty: nothing).
    pub mach: Vec<String>,
    pub devices: Vec<DeviceRule>,
    pub limits: Limits,
}

/// True when `p` is absolute and lexically normalised: no `.` or `..`, no repeated or
/// trailing separator (on Windows, `/` separators count as not normalised).
pub fn is_normalised(p: &Path) -> bool {
    if !p.is_absolute()
        || p.components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return false;
    }
    let rebuilt: PathBuf = p.components().collect();
    rebuilt.as_os_str() == p.as_os_str()
}

/// `base` joined with `rel`, a `/`-separated relative path, one component at a time (so the
/// result uses the OS separator and stays normalised).
pub fn under(base: &Path, rel: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

pub(crate) fn check_path(field: &str, p: &Path) -> Result<(), PolicyError> {
    if !p.is_absolute() {
        return Err(PolicyError::NotAbsolute {
            field: field.into(),
            path: p.to_path_buf(),
        });
    }
    if !is_normalised(p) {
        return Err(PolicyError::NotNormalised {
            field: field.into(),
            path: p.to_path_buf(),
        });
    }
    Ok(())
}

/// The depth of the deepest entry of `roots` that is `p` or an ancestor of `p`.
fn deepest(roots: &[PathBuf], p: &Path) -> Option<usize> {
    roots
        .iter()
        .filter(|r| p.starts_with(r))
        .map(|r| r.components().count())
        .max()
}

/// Some entry of `roots` lies strictly inside `p`.
fn holds(roots: &[PathBuf], p: &Path) -> bool {
    roots.iter().any(|r| r.starts_with(p) && r != p)
}

/// `a` is strictly more specific than `b` (`None` is least specific).
fn wins(a: Option<usize>, b: Option<usize>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

impl SandboxPolicy {
    /// The read roots (`Only`), or for `Everything` the filesystem roots of the `except` paths.
    pub fn read_roots(&self) -> Vec<PathBuf> {
        match &self.read {
            ReadPolicy::Only(r) => r.clone(),
            #[cfg(any(test, feature = "test-hooks"))]
            ReadPolicy::Everything { .. } => vec![fs_root()],
        }
    }

    /// `deny_read` plus, for `Everything`, its `except` list.
    pub fn denied(&self) -> Vec<PathBuf> {
        #[cfg(any(test, feature = "test-hooks"))]
        if let ReadPolicy::Everything { except } = &self.read {
            return self.deny_read.iter().chain(except).cloned().collect();
        }
        self.deny_read.clone()
    }

    /// The reference model: may the process write `p` as a whole (for a directory: create,
    /// remove and rename its entries)? The most specific root wins, protected and denied roots
    /// both exclude writes, and a directory with an excluded path inside it is not writable
    /// itself (else the excluded entry could be renamed away), only what lies beside the path.
    pub fn writable(&self, p: &Path) -> bool {
        let mut excl = self.protect.clone();
        excl.extend(self.denied());
        wins(deepest(&self.write, p), deepest(&excl, p)) && !holds(&excl, p)
    }

    /// The reference model: may the process read `p` as a whole (for a directory: list it), through
    /// a read root or a write root? A directory with a denied path inside it is not readable
    /// itself, only what lies beside the denied path.
    pub fn readable(&self, p: &Path) -> bool {
        let denied = self.denied();
        (wins(deepest(&self.read_roots(), p), deepest(&denied, p)) && !holds(&denied, p))
            || self.writable(p)
    }

    /// Refuses a policy that is not well formed. Profile builders call it last; the OS layers
    /// call it again on a policy that arrived over a pipe.
    pub fn validate(&self) -> Result<(), PolicyError> {
        let read = self.read_roots();
        let denied = self.denied();
        let lists: [(&str, &[PathBuf]); 5] = [
            ("write", &self.write),
            ("read", &read),
            ("protect", &self.protect),
            ("denyRead", &denied),
            ("unixSockets", &self.unix_sockets),
        ];
        for (field, list) in lists {
            for p in list {
                check_path(field, p)?;
            }
        }
        for d in &self.devices {
            check_path("devices", &d.path)?;
        }
        for w in &self.write {
            if let Some(p) = self.protect.iter().find(|p| w.starts_with(p)) {
                return Err(PolicyError::WriteUnderProtect {
                    write: w.clone(),
                    protect: p.clone(),
                });
            }
        }
        for (field, list) in [("write", &self.write), ("read", &read)] {
            if let Some(p) = list.iter().find(|p| denied.contains(p)) {
                return Err(PolicyError::GrantedAndDenied {
                    field: field.into(),
                    path: p.clone(),
                });
            }
        }
        self.validate_profile()
    }

    fn validate_profile(&self) -> Result<(), PolicyError> {
        let profile = self.profile;
        let bad = |reason: &str| Err(PolicyError::profile(profile, reason));
        let ports = match &self.net {
            Net::Open { loopback } => loopback.as_slice(),
            Net::Loopback { ports } => ports.as_slice(),
            Net::None => &[],
        };
        if ports.contains(&0) {
            return bad("port 0 is not a port");
        }
        match (&self.net, profile) {
            (Net::Open { .. }, Profile::Agent) | (Net::None, _) => {}
            (Net::Loopback { ports }, Profile::Hook) if !ports.is_empty() => {}
            (Net::Loopback { .. }, Profile::Hook) => return bad("a loopback rule names no port"),
            _ => return bad("this network rule is not allowed for the profile"),
        }
        if matches!(profile, Profile::Worker | Profile::Plugin) && !self.unix_sockets.is_empty() {
            return bad("no pathname socket is allowed");
        }
        if matches!(profile, Profile::Agent | Profile::Hook) && !self.devices.is_empty() {
            return bad("no device is allowed");
        }
        if profile == Profile::Worker && self.limits.processes != Some(1) {
            return bad("a worker runs as one process (processes = 1)");
        }
        let l = &self.limits;
        if l.processes == Some(0)
            || l.memory == Some(0)
            || l.open_files == Some(0)
            || l.wall_ms == Some(0)
        {
            return bad("a limit of 0");
        }
        let service = |s: &String| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        };
        if !self.mach.iter().all(service) {
            return bad("a mach-lookup name holds a character outside [A-Za-z0-9._-]");
        }
        Ok(())
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn fs_root() -> PathBuf {
    // Test-only: the system drive on Windows, `/` elsewhere.
    if cfg!(windows) {
        PathBuf::from("C:\\")
    } else {
        PathBuf::from("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalised_forms() {
        assert!(is_normalised(Path::new(if cfg!(windows) {
            "C:\\a\\b"
        } else {
            "/a/b"
        })));
        for bad in ["a/b", "/a/../b", "/a/./b", "/a/", "//a", "/a//b"] {
            if cfg!(windows) {
                continue;
            }
            assert!(!is_normalised(Path::new(bad)), "{bad}");
        }
    }

    #[test]
    fn under_joins_components() {
        let base = PathBuf::from(if cfg!(windows) { "C:\\h" } else { "/h" });
        let p = under(&base, "a/b c/d");
        assert!(is_normalised(&p));
        assert_eq!(p.components().count(), base.components().count() + 3);
    }
}
