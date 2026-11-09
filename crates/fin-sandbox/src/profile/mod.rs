//! The profile builders (spec §8.10, plan 7C Decisions 6–9). Each takes its typed input and
//! returns a validated [`SandboxPolicy`]; nothing else builds one.

mod agent;
mod hook;
mod plugin;
mod worker;

use std::path::{Path, PathBuf};

pub use agent::agent;
pub use hook::hook;
pub use plugin::plugin;
pub use worker::worker;

/// The builders for a named OS, so the macOS goldens (plan 7C Task 8) are checked on every OS.
#[doc(hidden)]
pub mod for_os {
    pub use super::agent::agent_on as agent;
    pub use super::hook::hook_on as hook;
    pub use super::plugin::plugin_on as plugin;
    pub use super::worker::worker_on as worker;
}

use crate::error::PolicyError;
use crate::policy::{Os, SandboxPolicy, check_path, under};

/// Home paths no profile reads or writes (spec §8.1 `deny_read`): credentials, browser
/// profiles, password managers, and the user's own Claude Code and Codex configuration.
/// Relative to the home directory, `/`-separated. A grant inside one is refused.
const SENSITIVE_UNIX: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".docker",
    ".netrc",
    ".git-credentials",
    ".config/git/credentials",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".config/gh",
    ".config/hub",
    ".claude",
    ".claude.json",
    ".codex",
    ".password-store",
    ".pki",
];
const SENSITIVE_LINUX: &[&str] = &[
    ".mozilla",
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    ".config/microsoft-edge",
    ".config/vivaldi",
    ".config/opera",
    "snap/firefox",
    "snap/chromium",
    ".var/app",
    ".local/share/keyrings",
    ".config/1Password",
    ".config/Bitwarden",
    ".config/keepassxc",
];
const SENSITIVE_MACOS: &[&str] = &[
    "Library/Keychains",
    "Library/Cookies",
    "Library/Safari",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Chromium",
    "Library/Application Support/Firefox",
    "Library/Application Support/BraveSoftware",
    "Library/Application Support/Microsoft Edge",
    "Library/Application Support/1Password",
    "Library/Application Support/Bitwarden",
    "Library/Group Containers",
    "Library/Containers",
];
const SENSITIVE_WINDOWS: &[&str] = &[
    ".ssh",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".git-credentials",
    ".npmrc",
    ".claude",
    ".claude.json",
    ".codex",
    "_netrc",
    "AppData/Roaming/gnupg",
    "AppData/Roaming/gcloud",
    "AppData/Roaming/GitHub CLI",
    "AppData/Local/Google/Chrome/User Data",
    "AppData/Local/Chromium/User Data",
    "AppData/Local/Microsoft/Edge/User Data",
    "AppData/Local/BraveSoftware",
    "AppData/Roaming/Mozilla",
    "AppData/Local/1Password",
    "AppData/Roaming/Bitwarden",
    "AppData/Local/Microsoft/Credentials",
    "AppData/Roaming/Microsoft/Credentials",
    "AppData/Roaming/Microsoft/Protect",
    "AppData/Local/Packages",
];
/// The user's document folders (macOS: TCC-protected), denied to plugins (sp8 §3.2, §16.1).
const PERSONAL_FOLDERS: &[&str] = &[
    "Documents",
    "Desktop",
    "Downloads",
    "Pictures",
    "Movies",
    "Music",
    "Videos",
];
const PERSONAL_MACOS: &[&str] = &[
    "Library/Mobile Documents",
    "Library/Mail",
    "Library/Messages",
    "Library/Calendars",
    "Library/Application Support/AddressBook",
];

pub(crate) fn sensitive_home(os: Os, home: &Path) -> Vec<PathBuf> {
    let lists: &[&[&str]] = match os {
        Os::Linux => &[SENSITIVE_UNIX, SENSITIVE_LINUX],
        Os::Macos => &[SENSITIVE_UNIX, SENSITIVE_MACOS],
        Os::Windows => &[SENSITIVE_WINDOWS],
    };
    lists
        .iter()
        .flat_map(|l| l.iter())
        .map(|rel| under(home, rel))
        .collect()
}

pub(crate) fn personal_folders(os: Os, home: &Path) -> Vec<PathBuf> {
    let extra: &[&str] = if os == Os::Macos { PERSONAL_MACOS } else { &[] };
    PERSONAL_FOLDERS
        .iter()
        .chain(extra)
        .map(|rel| under(home, rel))
        .collect()
}

/// Refuses a single path component that could leave its directory.
pub(crate) fn component(field: &str, s: &str) -> Result<(), PolicyError> {
    let bad = s.is_empty()
        || s == "."
        || s == ".."
        || s.chars()
            .any(|c| matches!(c, '/' | '\\' | ':' | '\0') || c.is_control());
    if bad {
        Err(PolicyError::input(
            field,
            format!("{s:?} is not a single path component"),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn path(field: &str, p: &Path) -> Result<(), PolicyError> {
    check_path(field, p)
}

pub(crate) fn paths<'a>(
    field: &str,
    ps: impl IntoIterator<Item = &'a PathBuf>,
) -> Result<(), PolicyError> {
    ps.into_iter()
        .enumerate()
        .try_for_each(|(n, p)| check_path(&format!("{field}[{n}]"), p))
}

/// `p` must be exactly `expected` (a directory the layout fixes).
pub(crate) fn exactly(field: &str, p: &Path, expected: &Path) -> Result<(), PolicyError> {
    if p == expected {
        Ok(())
    } else {
        Err(PolicyError::input(
            field,
            format!("{} is not {}", p.display(), expected.display()),
        ))
    }
}

/// `p` must lie strictly inside `dir`.
pub(crate) fn inside(field: &str, p: &Path, dir: &Path) -> Result<(), PolicyError> {
    if p.starts_with(dir) && p != dir {
        Ok(())
    } else {
        Err(PolicyError::input(
            field,
            format!("{} is not inside {}", p.display(), dir.display()),
        ))
    }
}

/// No path of `ps` may be, or lie inside, one of `denied`.
pub(crate) fn not_under<'a>(
    field: &str,
    ps: impl IntoIterator<Item = &'a PathBuf>,
    denied: &[PathBuf],
) -> Result<(), PolicyError> {
    for p in ps {
        if let Some(d) = denied.iter().find(|d| p.starts_with(d)) {
            return Err(PolicyError::input(
                field,
                format!("{} is inside the denied path {}", p.display(), d.display()),
            ));
        }
    }
    Ok(())
}

/// Sorts and deduplicates every list, then validates.
pub(crate) fn finish(mut p: SandboxPolicy) -> Result<SandboxPolicy, PolicyError> {
    for list in [
        &mut p.write,
        &mut p.protect,
        &mut p.deny_read,
        &mut p.unix_sockets,
    ] {
        list.sort();
        list.dedup();
    }
    #[allow(irrefutable_let_patterns)] // `Everything` exists in test builds only
    if let crate::policy::ReadPolicy::Only(r) = &mut p.read {
        r.sort();
        r.dedup();
    }
    p.devices.sort();
    p.devices.dedup();
    p.mach.sort();
    p.mach.dedup();
    p.validate()?;
    Ok(p)
}
