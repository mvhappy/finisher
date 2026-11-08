//! Client-side file arguments under an agent session's credential (sub-project 7 §5.6; plan
//! 7A Decision 14c). Some commands read a file named on the command line in `fin` itself, or
//! send its path to the server (`caption import --words`, `asset add`, a `batch` line's
//! `asset.add`, …). Under an agent credential every such argument is made absolute, its
//! symlinks resolved (case-folded on Windows and macOS), and it must lie under the session's
//! `workspace/<sessionId>/` or the project folder; anything else is `PATH_NOT_ALLOWED` (exit 7)
//! **before the file is opened**. Without an agent credential nothing changes.
//!
//! [`PATH_ARGS`] is the registry's `argKinds: path`: every argument of a command that names a
//! client-side file. A drift test checks it against the command line's value names, and the
//! built-in agent's `fin` tool reads it (`fin schema commands`) for the same check before it
//! spawns `fin`.

use std::path::{Component, Path, PathBuf};

use clap::ArgMatches;
use fin_core::CoreError;
use serde_json::json;

/// `(command, argument)`: the arguments that name a client-side file (`argKinds: path`). The
/// argument is the clap id (the field name).
pub const PATH_ARGS: &[(&str, &str)] = &[
    ("asset.add", "path"),
    ("caption.import", "words"),
    ("caption.import", "srt"),
    ("project.create", "dir"),
    ("project.open", "dir"),
    ("project.recover", "import"),
    ("registry.add", "dir"),
    ("registry.add", "root"),
];

/// The path arguments of `command`.
pub fn path_args(command: &str) -> impl Iterator<Item = &'static str> + '_ {
    PATH_ARGS
        .iter()
        .filter(move |(c, _)| *c == command)
        .map(|(_, a)| *a)
}

/// Whether paths compare case-insensitively here (Windows; macOS volumes are by default).
const FOLD: bool = cfg!(any(windows, target_os = "macos"));

/// `path` with its symlinks resolved: the deepest existing ancestor canonicalised, the rest (which
/// does not exist, so holds no link) appended. Never opens the file.
fn real(path: &Path) -> PathBuf {
    let abs = crate::paths::resolve(path);
    let mut existing = abs.as_path();
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(r) = std::fs::canonicalize(existing) {
            let mut out = strip_verbatim(r);
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return abs,
        }
    }
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = p.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    p
}

fn parts(p: &Path, fold: bool) -> Vec<String> {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| {
            let s = c.as_os_str().to_string_lossy().into_owned();
            if fold { s.to_lowercase() } else { s }
        })
        .collect()
}

fn under(path: &Path, root: &Path, fold: bool) -> bool {
    let (p, r) = (parts(path, fold), parts(root, fold));
    p.len() >= r.len() && p[..r.len()] == r[..]
}

pub fn not_allowed(given: &str, reason: &str) -> CoreError {
    CoreError::new(
        "PATH_NOT_ALLOWED",
        format!(
            "{given}: an agent session reads files in its workspace or the project folder only"
        ),
    )
    .with_details(json!({ "path": given, "reason": reason }))
    .with_fix(
        "copy or write the file into the session's workspace (the working directory) first, \
         then name it by a path inside it",
    )
}

/// The real path of `path` when it lies under `workspace` or `project` (both resolved the
/// same way); `PATH_NOT_ALLOWED` otherwise. Nothing is opened.
pub fn confine(path: &Path, workspace: &Path, project: &Path) -> Result<PathBuf, CoreError> {
    confine_with(path, &[workspace, project], FOLD)
}

pub(crate) fn confine_with(path: &Path, roots: &[&Path], fold: bool) -> Result<PathBuf, CoreError> {
    let given = path.display().to_string();
    if path.as_os_str().is_empty() {
        return Err(not_allowed(&given, "empty"));
    }
    let r = real(path);
    if roots.iter().any(|root| under(&r, &real(root), fold)) {
        Ok(r)
    } else {
        Err(not_allowed(&given, "outside"))
    }
}

/// The roots of this run under an agent credential: the session's workspace and the project
/// folder `fin` found (`--project`, else the nearest ancestor holding `finisher.project.json`).
/// `None` without an agent credential.
pub fn roots(project: Option<&Path>) -> Result<Option<(PathBuf, PathBuf)>, CoreError> {
    let Some(session) = crate::agent_session::session_id() else {
        return Ok(None);
    };
    let Some(dir) = crate::backend::project_dir(project)? else {
        // No project folder: nothing to read from but the workspace, which is inside one.
        return Err(not_allowed(".", "noProject"));
    };
    let workspace = dir.join("workspace").join(&session);
    Ok(Some((workspace, dir)))
}

/// Confines `path` when this run has an agent credential; nothing otherwise.
pub fn check(path: &str, project: Option<&Path>) -> Result<(), CoreError> {
    if let Some((workspace, dir)) = roots(project)? {
        confine(
            &crate::paths::expand_home(Path::new(path)),
            &workspace,
            &dir,
        )?;
    }
    Ok(())
}

/// Every [`PATH_ARGS`] value of the parsed command, confined (an agent credential only).
pub fn check_matches(
    matches: &ArgMatches,
    command: Option<&str>,
    project: Option<&Path>,
) -> Result<(), CoreError> {
    let Some(command) = command else {
        return Ok(());
    };
    let mut names = path_args(command).peekable();
    if names.peek().is_none() {
        return Ok(());
    }
    let Some((workspace, dir)) = roots(project)? else {
        return Ok(());
    };
    let mut leaf = matches;
    while let Some((_, sub)) = leaf.subcommand() {
        leaf = sub;
    }
    for name in names {
        let Ok(Some(values)) = leaf.try_get_raw(name) else {
            continue;
        };
        for v in values {
            let p = crate::paths::expand_home(Path::new(v));
            confine(&p, &workspace, &dir)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "agent_paths_tests.rs"]
mod tests;
