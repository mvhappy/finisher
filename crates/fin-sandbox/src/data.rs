//! Data files shipped with the crate (`data/<os>/`), parsed here so every list has one reader.

use fin_core::events::payloads::agent::Harness;

use crate::policy::{Os, Profile};

const TOOLCHAIN_LINUX: &str = include_str!("../data/linux/toolchain-base.txt");
const TOOLCHAIN_MACOS: &str = include_str!("../data/macos/toolchain-base.txt");
const TOOLCHAIN_WINDOWS: &str = include_str!("../data/windows/toolchain-base.txt");
const CLAUDE_LINUX: &str = include_str!("../data/linux/toolchain-claudeCode.txt");
const CLAUDE_MACOS: &str = include_str!("../data/macos/toolchain-claudeCode.txt");
const CLAUDE_WINDOWS: &str = include_str!("../data/windows/toolchain-claudeCode.txt");
const GIT_CONFIG: &str = include_str!("../data/linux/git-config");
const MACH_AGENT: &str = include_str!("../data/macos/mach-agent.txt");
const MACH_HOOK: &str = include_str!("../data/macos/mach-hook.txt");
const MACH_WORKER: &str = include_str!("../data/macos/mach-worker.txt");
const MACH_PLUGIN: &str = include_str!("../data/macos/mach-plugin.txt");

/// One entry per line; `#` starts a comment.
fn entries(text: &'static str) -> impl Iterator<Item = &'static str> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
}

/// The macOS `mach-lookup` allow-list of `profile` (spec §8.2; spike S9 B6): the services a
/// process of the profile may look up, everything else denied. Empty on other OSes.
pub fn mach(os: Os, profile: Profile) -> Vec<String> {
    if os != Os::Macos {
        return Vec::new();
    }
    let text = match profile {
        Profile::Agent => MACH_AGENT,
        Profile::Hook => MACH_HOOK,
        Profile::Worker => MACH_WORKER,
        Profile::Plugin => MACH_PLUGIN,
    };
    entries(text).map(str::to_owned).collect()
}

/// The session's git configuration (spec §8.3 validation list): the file `GIT_CONFIG_GLOBAL`
/// names, with `GIT_CONFIG_NOSYSTEM=1`. No fsmonitor (a daemon, and a program named by
/// repository configuration), no credential helper (credential-cache is a daemon on a Unix
/// socket), no signing (gpg-agent is one too).
pub fn session_git_config() -> &'static str {
    GIT_CONFIG
}

/// The base toolchain read list of `os` (spec §8.1 item 1): one absolute path per line;
/// `#` starts a comment. Windows entries may start with `%SystemRoot%` or `%ProgramFiles%`,
/// which the caller expands. Each file's header names its source (spike S9, plan 7C Task 0).
pub fn toolchain_base(os: Os) -> Vec<&'static str> {
    let text = match os {
        Os::Linux => TOOLCHAIN_LINUX,
        Os::Macos => TOOLCHAIN_MACOS,
        Os::Windows => TOOLCHAIN_WINDOWS,
    };
    entries(text).collect()
}

/// The read list of `harness`'s own CLI on `os`, on top of [`toolchain_base`] (plan 7D Decision 6
/// and Task 14): one absolute path per line, each file's header naming its evidence. Only Claude
/// Code has one (empty on macOS and Windows until 7D Task 18 records a start there). The CLI's
/// install tree is never in it: it moves with the person's install, so the adapter passes it
/// as the process's `extraRead`. `profile::agent` adds this list to the policy's reads.
pub fn toolchain_harness(os: Os, harness: Harness) -> Vec<&'static str> {
    if harness != Harness::ClaudeCode {
        return Vec::new();
    }
    let text = match os {
        Os::Linux => CLAUDE_LINUX,
        Os::Macos => CLAUDE_MACOS,
        Os::Windows => CLAUDE_WINDOWS,
    };
    entries(text).collect()
}

/// [`toolchain_base`] of Windows with its leading `%SystemRoot%` or `%ProgramFiles%` expanded
/// by `var` (the environment, case-insensitive names; pure, so tested on every OS). An entry
/// whose variable is unset, or that is not a drive path (`X:\…`) once expanded, is dropped:
/// a toolchain root is never granted by a relative or unexpanded name (Task 10 review B1).
pub fn toolchain_base_windows(var: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    toolchain_base(Os::Windows)
        .into_iter()
        .filter_map(|e| expand_windows(e, var))
        .collect()
}

/// [`toolchain_base_windows`] from this process's environment, entries that do not exist
/// dropped (`%ProgramFiles%\PowerShell` where PowerShell 7 is not installed).
#[cfg(windows)]
pub fn toolchain_base_present() -> Vec<String> {
    toolchain_base_windows(&|n| std::env::var(n).ok())
        .into_iter()
        .filter(|p| std::path::Path::new(p).exists())
        .collect()
}

fn expand_windows(entry: &str, var: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let out = match entry.strip_prefix('%') {
        Some(rest) => {
            let (name, tail) = rest.split_once('%')?;
            let value = var(name)?;
            format!("{}{tail}", value.trim_end_matches(['\\', '/']))
        }
        None => entry.to_owned(),
    };
    let b = out.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    (drive && !out.contains('%') && !out.contains('/')).then_some(out)
}

/// [`toolchain_base`] as a policy takes it (Unix): each entry's links resolved, entries that do
/// not exist dropped, entries inside another dropped, sorted. On a merged-`/usr` host `/bin`
/// becomes `/usr/bin`, which is already listed. On macOS a link entry in a canonical directory
/// is also kept as itself.
#[cfg(unix)]
pub fn toolchain_base_resolved(os: Os) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    for p in toolchain_base(os) {
        let Ok(real) = std::fs::canonicalize(p) else {
            continue;
        };
        // macOS: a link entry is kept as well as its target (Seatbelt grants the link itself
        // only; `/bin/sh` reads `/private/var/select/sh`). Its directory must be canonical.
        let p = std::path::Path::new(p);
        let parent_real = p.parent().and_then(|d| std::fs::canonicalize(d).ok());
        if os == Os::Macos && real != p && parent_real.as_deref() == p.parent() {
            out.push(p.to_path_buf());
        }
        out.push(real);
    }
    out.sort();
    out.dedup();
    let all = out.clone();
    out.retain(|p| !all.iter().any(|q| q != p && p.starts_with(q)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_is_absolute_for_its_os() {
        for p in toolchain_base(Os::Linux)
            .into_iter()
            .chain(toolchain_base(Os::Macos))
        {
            assert!(
                p.starts_with('/') && !p.ends_with('/') && !p.contains("//") && !p.contains("/."),
                "{p}"
            );
        }
        for p in toolchain_base(Os::Windows) {
            assert!(
                p.starts_with('%') || p.as_bytes().get(1) == Some(&b':'),
                "{p}"
            );
        }
        for os in [Os::Linux, Os::Macos, Os::Windows] {
            assert!(!toolchain_base(os).is_empty(), "{os:?}");
        }
    }

    #[test]
    fn windows_entries_are_expanded_or_dropped() {
        let env = |n: &str| match n.to_ascii_uppercase().as_str() {
            "SYSTEMROOT" => Some(r"C:\Windows".to_owned()),
            "PROGRAMFILES" => Some(r"C:\Program Files\".to_owned()),
            _ => None,
        };
        let list = toolchain_base_windows(&env);
        assert!(
            list.contains(&r"C:\Windows\System32".to_owned()),
            "{list:?}"
        );
        assert!(
            list.contains(&r"C:\Program Files\Git".to_owned()),
            "{list:?}"
        );
        assert_eq!(list.len(), toolchain_base(Os::Windows).len());
        assert!(list.iter().all(|p| !p.contains('%')), "{list:?}");
        // Unset, or not a drive path once expanded: dropped.
        assert!(toolchain_base_windows(&|_| None).is_empty());
        assert!(toolchain_base_windows(&|_| Some("relative".into())).is_empty());
        assert_eq!(
            expand_windows(r"D:\tools", &|_| None).as_deref(),
            Some(r"D:\tools")
        );
    }

    #[test]
    fn the_macos_toolchain_leaves_homebrews_data_and_configuration_out() {
        let list = toolchain_base(Os::Macos);
        for data in [
            "/usr/local/var",
            "/usr/local/etc",
            "/opt/homebrew/var",
            "/opt/homebrew/etc",
        ] {
            assert!(
                !list.iter().any(|e| data.starts_with(e)),
                "{data} is covered by the list"
            );
        }
    }

    #[test]
    fn mach_lists_are_macos_only_and_never_name_the_forbidden_services() {
        assert!(mach(Os::Macos, Profile::Agent).contains(&"com.apple.cfprefsd.daemon".into()));
        assert!(mach(Os::Macos, Profile::Hook).is_empty());
        assert!(mach(Os::Macos, Profile::Plugin).is_empty());
        let forbidden = [
            "com.apple.SecurityServer",
            "com.apple.securityd",
            "com.apple.lsd",
            "com.apple.coreservices",
            "com.apple.CoreServices",
            "com.apple.windowserver",
            "com.apple.tccd",
            "com.apple.pboard",
            "com.apple.FSEvents",
        ];
        for p in Profile::ALL {
            assert!(mach(Os::Linux, p).is_empty() && mach(Os::Windows, p).is_empty());
            for name in mach(Os::Macos, p) {
                assert!(
                    !forbidden.iter().any(|f| name.starts_with(f)),
                    "{p:?}: {name}"
                );
            }
        }
    }
}
