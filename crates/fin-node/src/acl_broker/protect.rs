//! What the broker never grants (plan 7C Decision 18), checked before anything is logged. The
//! grant tables (`fin_sandbox::windows::grants`) already follow each profile's policy; this is
//! the second layer, so a wrong table or a hostile caller cannot widen a sandbox through the
//! broker:
//!
//! 1. **Well-formed**: an absolute path without `.` or `..`; a mask that is not zero and lies
//!    within modify (`0x1301BF`: no `WRITE_DAC`, `WRITE_OWNER`, `ACCESS_SYSTEM_SECURITY` or
//!    generic rights); a SID `S-1-…`; an owner of ASCII letters, digits and `:._-`.
//! 2. **Server data and its ancestors**: never (an inheritable ACE on an ancestor would reach
//!    everything below it that does not block inheritance).
//! 3. **Inside server data**, only the places a profile is meant to receive (spec §8.5, §8.6;
//!    plan Decisions 6, 7, 19, 21), anything else (`auth/`, `run/`, the control store, a
//!    session's `blobs/` or the session directory itself, `agents/`, `providers/<h>/home/`
//!    itself) refused:
//!    - `agents/<id>/{tmp, provider, localappdata}` and below: any mask;
//!    - `agents/<id>/{credential.json, settings, git}` and below: read only (protected);
//!    - under `agents/<id>/`, only for the owner `session:<id>` (one session's grant never
//!      names another session's files);
//!    - `providers/<harness>/home/<store>` and below: any mask (the sign-in store);
//!    - `jobs/<dir>` and below: any mask (a job's directory, a warm worker's own);
//!    - `hooks/<call>` and below: any mask (a hook call's private temp and credential);
//!    - `plugins/store/<package>`, `plugins/models/<model>`, `fonts`, `cache` and below: read
//!      only (the store is read-only in every profile, Decision 21).
//! 4. **Project state**: a mask with any right beyond read and execute is refused on a
//!    project folder itself (a folder holding `finisher.project.json`) and on its
//!    `.finisher/`, `assets/`, `finisher.project.json`, `finisher.rules.json` and below (the
//!    profiles' `protect` roots). Residual: an inheritable write grant on a folder that holds a
//!    project somewhere below is not found (that would walk the subtree).
//!
//! **One form of each path** ([`Rules::resolve`]): the path is resolved once on entry (links,
//! `\\?\`, 8.3 names, `\\localhost\C$\…`), and that form is checked against the canonical
//! server data, and then locked, recorded and applied, so no spelling leads a grant past a
//! rule. A path that cannot be resolved is refused (`NOT_FOUND` when it does not exist), and
//! on Windows only a drive path (`C:\…`, `\\?\C:\…`) is accepted: a share (UNC) is refused.

use std::path::{Component, Path, PathBuf};

use fin_sandbox::profile::PROJECT_STATE;
use fin_sandbox::windows::grants::{MODIFY, READ_EXECUTE};

use super::{BrokerError, Grant};

/// The project file that marks a project folder.
const PROJECT_FILE: &str = "finisher.project.json";

/// The components of `p` below `base` (empty for `base` itself), or `None` when `p` is not
/// inside it. Windows compares without case.
pub fn relative(base: &Path, p: &Path) -> Option<Vec<String>> {
    let mut b = base.components();
    let mut q = p.components();
    loop {
        match (b.next(), q.next()) {
            (None, None) => return Some(vec![]),
            (None, Some(c)) => {
                let mut out = vec![c.as_os_str().to_string_lossy().into_owned()];
                out.extend(q.map(|c| c.as_os_str().to_string_lossy().into_owned()));
                return Some(out);
            }
            (Some(_), None) => return None,
            (Some(x), Some(y)) => {
                let (x, y) = (
                    x.as_os_str().to_string_lossy(),
                    y.as_os_str().to_string_lossy(),
                );
                let same = if cfg!(windows) {
                    x.eq_ignore_ascii_case(&y)
                } else {
                    x == y
                };
                if !same {
                    return None;
                }
            }
        }
    }
}

fn eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Whether `rel` (inside server data) may be granted, and why not.
fn in_server_data(rel: &[String], write: bool, owner: &str) -> Result<(), &'static str> {
    let at = |i: usize, name: &str| rel.get(i).is_some_and(|c| eq(c, name));
    let any = |i: usize, names: &[&str]| names.iter().any(|n| at(i, n));
    let read_only = |ok: bool| match (ok, write) {
        (false, _) => Err("not a place a sandbox profile receives"),
        (true, true) => Err("read only for every profile"),
        (true, false) => Ok(()),
    };
    if at(0, "agents") && rel.len() >= 3 {
        if !owner
            .strip_prefix("session:")
            .is_some_and(|id| eq(id, &rel[1]))
        {
            return Err("another session's files");
        }
        if any(2, &["tmp", "provider", "localappdata"]) {
            return Ok(());
        }
        return read_only(any(2, &["credential.json", "settings", "git"]));
    }
    if at(0, "providers") && at(2, "home") && rel.len() >= 4 {
        return Ok(());
    }
    if any(0, &["jobs", "hooks"]) && rel.len() >= 2 {
        return Ok(());
    }
    if at(0, "plugins") && any(1, &["store", "models"]) && rel.len() >= 3 {
        return read_only(true);
    }
    read_only(any(0, &["fonts", "cache"]))
}

/// The broker's view of server data.
pub struct Rules {
    data: PathBuf,
}

/// `p` resolved when it can be (a revoke by path of a path gone keeps its spelling).
pub fn resolved(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// On Windows only a drive path is accepted (`C:\…`; `\\?\C:\…` is what resolving gives).
fn local_drive(p: &Path) -> bool {
    use std::path::Prefix;
    match p.components().next() {
        Some(Component::Prefix(pre)) => {
            matches!(pre.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        }
        _ => !cfg!(windows),
    }
}

impl Rules {
    pub fn new(server_data: &Path) -> Rules {
        Rules {
            data: resolved(server_data),
        }
    }

    /// `g` with its path resolved, or why it is refused (nothing is logged for it): malformed,
    /// missing, not resolvable, not on a local drive, or protected.
    pub fn resolve(&self, g: &Grant) -> Result<Grant, BrokerError> {
        well_formed(g)?;
        let path = match std::fs::canonicalize(&g.path) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(BrokerError::NotFound(g.path.clone()));
            }
            Err(_) => {
                return Err(BrokerError::Protected {
                    path: g.path.clone(),
                    why: "a path that cannot be resolved",
                });
            }
        };
        if !local_drive(&path) {
            return Err(BrokerError::Protected {
                path: g.path.clone(),
                why: "not a path on a local drive",
            });
        }
        let g = Grant { path, ..g.clone() };
        self.check(&g)?;
        Ok(g)
    }

    /// Refuses `g`, its path taken as resolved, when it is malformed or protected.
    pub fn check(&self, g: &Grant) -> Result<(), BrokerError> {
        well_formed(g)?;
        let write = g.mask & !READ_EXECUTE != 0;
        let protected = |why| BrokerError::Protected {
            path: g.path.clone(),
            why,
        };
        if relative(&g.path, &self.data).is_some() {
            return Err(protected("server data or a folder that holds it"));
        }
        if let Some(rel) = relative(&self.data, &g.path) {
            in_server_data(&rel, write, &g.owner).map_err(protected)?;
        }
        if write {
            project_state(&g.path).map_err(protected)?;
        }
        Ok(())
    }
}

/// Refuses a write grant on a project folder or its state (rule 4).
fn project_state(path: &Path) -> Result<(), &'static str> {
    for dir in path.ancestors() {
        if std::fs::symlink_metadata(dir.join(PROJECT_FILE)).is_ok_and(|m| m.is_file()) {
            let rel = relative(dir, path).unwrap_or_default();
            let Some(first) = rel.first() else {
                return Err("a project folder (write)");
            };
            if PROJECT_STATE.iter().any(|s| eq(s, first)) {
                return Err("a project's state (write)");
            }
            return Ok(());
        }
    }
    Ok(())
}

fn well_formed(g: &Grant) -> Result<(), BrokerError> {
    let bad = |m: &str| Err(BrokerError::Invalid(m.to_owned()));
    if !g.path.is_absolute()
        || g.path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return bad("the path must be absolute, without . or .. segments");
    }
    if g.mask == 0 || g.mask & !MODIFY != 0 {
        return bad("the mask must be within modify (no WRITE_DAC, WRITE_OWNER or generic rights)");
    }
    let sid_ok = g.sid.len() <= 256
        && g.sid.starts_with("S-1-")
        && g.sid[4..]
            .split('-')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if !sid_ok {
        return bad("the SID is not S-1-…");
    }
    let owner_ok = !g.owner.is_empty()
        && g.owner.len() <= 160
        && g.owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'_' | b'-'));
    if !owner_ok {
        return bad("the owner is not a name (ASCII letters, digits, ':', '.', '_', '-')");
    }
    Ok(())
}
