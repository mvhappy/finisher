//! The namespace's mount plan (plan 7C Decision 26), pure: which roots get an empty `tmpfs`
//! and which granted paths are bound back on top, in the order the launcher mounts them.

use std::path::{Path, PathBuf};

use crate::policy::{Net, Profile, SandboxPolicy};

/// The roots hidden on every host where they exist, before the launcher's
/// `$XDG_RUNTIME_DIR`, the input's home directory and the policy's denied directories are
/// added: runtime and temporary directories, and the container managers' sockets (the
/// `lxd`/`incus` groups' sockets are root-equivalent) and WSLg's.
pub const HIDDEN: &[&str] = &[
    "/run",
    "/var/run",
    "/tmp",
    "/var/tmp",
    "/dev/shm",
    "/var/lib/incus",
    "/var/lib/lxd",
    "/var/snap/lxd/common/lxd",
    "/mnt/wslg",
];

/// One mount of the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// An empty `tmpfs` over the path.
    Hide(PathBuf),
    /// The real path bound back over a hiding `tmpfs`.
    Keep(PathBuf),
}

impl Step {
    pub fn path(&self) -> &Path {
        match self {
            Step::Hide(p) | Step::Keep(p) => p,
        }
    }
}

/// What the namespace mounts, shallowest first, and whether it adds a network namespace.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub steps: Vec<Step>,
    pub net: bool,
}

impl Layout {
    pub fn hidden(&self) -> impl Iterator<Item = &Path> {
        self.steps.iter().filter_map(|s| match s {
            Step::Hide(p) => Some(p.as_path()),
            Step::Keep(_) => None,
        })
    }

    pub fn kept(&self) -> impl Iterator<Item = &Path> {
        self.steps.iter().filter_map(|s| match s {
            Step::Keep(p) => Some(p.as_path()),
            Step::Hide(_) => None,
        })
    }
}

/// The plan (all paths canonical and existing):
/// - `candidates` (socket locations) are hidden unless they are at or inside a granted root,
///   whose content is granted anyway;
/// - `denied` directories are hidden wherever they are, inside a granted root too (server data
///   in a library root, `~/.ssh` when the home directory is granted): they are unreadable
///   anyway, and they may hold sockets (`fin.sock`, ControlMaster sockets);
/// - each `granted` path is bound back when the nearest mount above it is a hide.
///
/// A hide below another hide, or a keep with no hide above it, is dropped (nothing to do).
pub fn layout(
    candidates: &[PathBuf],
    denied: &[PathBuf],
    granted: &[PathBuf],
    net: bool,
) -> Layout {
    let mut all: Vec<Step> = candidates
        .iter()
        .filter(|c| !granted.iter().any(|g| c.starts_with(g)))
        .chain(denied)
        .map(|p| Step::Hide(p.clone()))
        .chain(granted.iter().map(|p| Step::Keep(p.clone())))
        .collect();
    all.sort_by(|a, b| {
        let key = |s: &Step| (s.path().components().count(), s.path().to_owned());
        key(a).cmp(&key(b))
    });
    all.dedup();
    let mut steps: Vec<Step> = Vec::new();
    for s in all {
        let above = steps
            .iter()
            .filter(|o| o.path() != s.path() && s.path().starts_with(o.path()))
            .max_by_key(|o| o.path().components().count());
        let hidden_above = matches!(above, Some(Step::Hide(_)));
        let wanted = match s {
            Step::Hide(_) => !hidden_above,
            Step::Keep(_) => hidden_above,
        };
        // A path both hidden and kept is refused by `validate` (granted and denied); a
        // candidate equal to a grant was dropped above.
        if wanted && !steps.iter().any(|o| o.path() == s.path()) {
            steps.push(s);
        }
    }
    Layout { steps, net }
}

/// Does `policy` get an empty network namespace? A `hook` with no network at all (spec
/// §8.10); pathname sockets (`unix_sockets`) still work across network namespaces.
pub fn wants_net(policy: &SandboxPolicy) -> bool {
    policy.profile == Profile::Hook && policy.net == Net::None
}

/// The layout for `policy` on this host: [`HIDDEN`], `xdg_runtime`, `home` and the policy's
/// denied directories, resolved (missing ones skipped); the granted paths are the read and
/// write roots, the sockets and the devices, resolved.
pub fn plan(policy: &SandboxPolicy, home: Option<&Path>, xdg_runtime: Option<&Path>) -> Layout {
    let resolve = |p: &Path| std::fs::canonicalize(p).ok();
    let mut candidates: Vec<PathBuf> = HIDDEN.iter().map(Path::new).filter_map(resolve).collect();
    candidates.extend(xdg_runtime.filter(|p| p.is_absolute()).and_then(resolve));
    candidates.extend(home.and_then(resolve));
    let denied: Vec<PathBuf> = policy
        .denied()
        .iter()
        .filter_map(|p| resolve(p))
        .filter(|p| p.is_dir())
        .collect();
    let granted: Vec<PathBuf> = policy
        .read_roots()
        .iter()
        .chain(&policy.write)
        .chain(&policy.unix_sockets)
        .chain(policy.devices.iter().map(|d| &d.path))
        .filter_map(|p| resolve(p))
        .collect();
    layout(&candidates, &denied, &granted, wants_net(policy))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Vec<PathBuf> {
        v.iter().map(PathBuf::from).collect()
    }

    fn steps(l: &Layout) -> Vec<String> {
        l.steps
            .iter()
            .map(|s| match s {
                Step::Hide(p) => format!("hide {}", p.display()),
                Step::Keep(p) => format!("keep {}", p.display()),
            })
            .collect()
    }

    #[test]
    fn hidden_roots_skip_nested_and_granted() {
        let l = layout(
            &p(&["/run", "/run/user/1000", "/tmp", "/home/u", "/srv/x"]),
            &[],
            &p(&["/srv", "/usr"]),
            false,
        );
        assert_eq!(steps(&l), ["hide /run", "hide /tmp", "hide /home/u"]);
    }

    #[test]
    fn kept_paths_are_only_under_hidden_roots_shallowest_first() {
        let l = layout(
            &p(&["/run", "/home/u"]),
            &[],
            &p(&[
                "/usr/bin",
                "/home/u/proj/workspace/s1",
                "/home/u/proj",
                "/run/systemd/resolve/stub-resolv.conf",
                "/home/u/.local/share/finisher/agents/s1/tmp",
            ]),
            true,
        );
        assert_eq!(
            steps(&l),
            [
                "hide /run",
                "hide /home/u",
                "keep /home/u/proj",
                "keep /run/systemd/resolve/stub-resolv.conf",
                "keep /home/u/.local/share/finisher/agents/s1/tmp",
            ]
        );
        assert!(l.net);
    }

    #[test]
    fn denied_directories_are_hidden_inside_grants_and_their_holes_kept() {
        // A home directory granted as a library root: its sensitive paths and the server data
        // inside it are still hidden, and the session's own files in server data come back.
        let l = layout(
            &p(&["/tmp", "/home/u"]),
            &p(&["/home/u/.ssh", "/home/u/.local/share/finisher"]),
            &p(&["/home/u", "/home/u/.local/share/finisher/agents/s1/tmp"]),
            false,
        );
        assert_eq!(
            steps(&l),
            [
                "hide /tmp",
                "hide /home/u/.ssh",
                "hide /home/u/.local/share/finisher",
                "keep /home/u/.local/share/finisher/agents/s1/tmp",
            ]
        );
        // A candidate equal to a grant is not hidden.
        assert_eq!(
            layout(&p(&["/tmp"]), &[], &p(&["/tmp"]), false),
            Layout::default()
        );
    }
}
