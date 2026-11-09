//! Policy paths as Seatbelt matches them (plan 7C Task 8): Seatbelt compares the resolved path
//! of every access, so a grant must name resolved paths and must not pass through a link (a
//! link planted in a granted directory must not widen the grant), while an exclusion may follow
//! links (it only narrows).

use std::path::{Path, PathBuf};

use crate::error::ApplyError;
use crate::policy::SandboxPolicy;

/// What a path is granted for, which decides whether it may be a link ([`resolve_real`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// A read root: its last component may be a link (the rule then covers the link itself),
    /// when the link's target is itself readable ([`check_read_links`]).
    Read,
    /// A protected or denied path: links are followed (an exclusion only narrows, and
    /// Seatbelt matches what is reached through a link by its target).
    Exclude,
    /// Anything else (a grant): no component may be a link.
    Other,
}

/// Maps a policy path to the form the kernel matches, or refuses it ([`resolve_real`] on a
/// Mac; tests pass the identity).
pub type Resolve = dyn Fn(&Path, Use) -> Result<PathBuf, ApplyError>;

/// `p` with its existing part canonicalised (links followed), the rest appended.
fn follow(p: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(mut real) = std::fs::canonicalize(cur) {
            real.extend(tail.iter().rev());
            return real;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_owned());
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// A read root that is a link is granted as the link alone ([`Use::Read`]); its target must be
/// readable through another read root (`/private/var/select/sh` → `/bin/bash`, under `/bin`),
/// else the root is refused rather than silently granting nothing (a linked library root).
pub fn check_read_links(policy: &SandboxPolicy) -> Result<(), ApplyError> {
    let reads = policy.read_roots();
    let real: Vec<PathBuf> = reads.iter().map(|r| follow(&lexical(r))).collect();
    for r in &reads {
        let l = lexical(r);
        if !l
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            continue;
        }
        let target = follow(&l);
        let covered = real
            .iter()
            .zip(&reads)
            .any(|(t, other)| other != r && target.starts_with(t));
        if !covered {
            return Err(ApplyError::Rule {
                path: r.clone(),
                error: format!(
                    "is a symbolic link to {}, which no other read root covers",
                    target.display()
                ),
            });
        }
    }
    Ok(())
}

/// The links macOS ships at the top of the tree; a path through one is resolved, any other
/// link refuses the path (as Landlock refuses a linked root: a link planted in a granted
/// directory must not widen the grant to its target).
pub const SYSTEM_LINKS: &[(&str, &str)] = &[
    ("/etc", "/private/etc"),
    ("/tmp", "/private/tmp"),
    ("/var", "/private/var"),
];

fn lexical(p: &Path) -> PathBuf {
    for (link, target) in SYSTEM_LINKS {
        if let Ok(rest) = p.strip_prefix(link) {
            return Path::new(target).join(rest);
        }
    }
    p.to_path_buf()
}

/// `p` as the kernel matches it: `/etc`, `/tmp` and `/var` resolved to `/private/…`. The
/// existing part of the path is canonicalised and must equal that form, so a path that is, or
/// lies below, any other link is refused (a dangling one too); a part that does not exist yet
/// keeps its name. One exception, for [`Use::Read`]: the path itself may be a link (the
/// toolchain's `/private/var/select/sh`, which `/bin/sh` reads). Seatbelt matches the resolved
/// path of every access through a link, so a rule on the link covers the link alone (`lstat`,
/// `readlink`), never its target.
pub fn resolve_real(p: &Path, u: Use) -> Result<PathBuf, ApplyError> {
    let want = lexical(p);
    if u == Use::Exclude {
        return Ok(follow(&want));
    }
    let refuse = |what: String| ApplyError::Rule {
        path: p.to_path_buf(),
        error: format!("is or lies below a symbolic link ({what})"),
    };
    let is_link = |q: &Path| {
        q.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
    };
    if u == Use::Read && is_link(&want) {
        let parent = want.parent().ok_or_else(|| refuse("/".into()))?;
        if resolve_real(parent, Use::Other)? == parent {
            return Ok(want);
        }
    }
    let mut tail = Vec::new();
    let mut cur = want.as_path();
    loop {
        match std::fs::canonicalize(cur) {
            Ok(real) if real != cur => return Err(refuse(format!("to {}", real.display()))),
            Ok(real) => {
                let mut out = real;
                out.extend(tail.iter().rev());
                return Ok(out);
            }
            Err(_) if is_link(cur) => return Err(refuse("dangling".into())),
            Err(_) => {}
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_owned());
                cur = parent;
            }
            _ => return Ok(want),
        }
    }
}
