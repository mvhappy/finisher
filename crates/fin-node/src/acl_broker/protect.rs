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
//! Rules 2 and 3 are checked on the path as given and, when it exists, on its canonical form
//! against the canonical server data, so a link does not lead a grant in.

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
fn in_server_data(rel: &[String], write: bool) -> Result<(), &'static str> {
    let at = |i: usize, name: &str| rel.get(i).is_some_and(|c| eq(c, name));
    let any = |i: usize, names: &[&str]| names.iter().any(|n| at(i, n));
    let read_only = |ok: bool| match (ok, write) {
        (false, _) => Err("not a place a sandbox profile receives"),
        (true, true) => Err("read only for every profile"),
        (true, false) => Ok(()),
    };
    if at(0, "agents") && rel.len() >= 3 {
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
    canonical: Option<PathBuf>,
}

impl Rules {
    pub fn new(server_data: &Path) -> Rules {
        Rules {
            data: server_data.to_path_buf(),
            canonical: std::fs::canonicalize(server_data).ok(),
        }
    }

    /// Refuses `g` when it is malformed or protected (nothing is logged for it).
    pub fn check(&self, g: &Grant) -> Result<(), BrokerError> {
        well_formed(g)?;
        let write = g.mask & !READ_EXECUTE != 0;
        let protected = |why| BrokerError::Protected {
            path: g.path.clone(),
            why,
        };
        let canonical = std::fs::canonicalize(&g.path).ok();
        let mut forms = vec![(g.path.as_path(), self.data.as_path())];
        if let (Some(p), Some(d)) = (canonical.as_deref(), self.canonical.as_deref()) {
            forms.push((p, d));
        }
        for (path, data) in forms {
            if relative(path, data).is_some() {
                return Err(protected("server data or a folder that holds it"));
            }
            if let Some(rel) = relative(data, path) {
                in_server_data(&rel, write).map_err(protected)?;
            }
        }
        if write {
            project_state(canonical.as_deref().unwrap_or(&g.path)).map_err(protected)?;
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
