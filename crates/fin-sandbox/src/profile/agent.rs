//! The `agent` profile (spec §8.1, §8.5, §8.6; plan 7C Decision 6).
//!
//! Reads, by allow-list: the toolchains, the project folder (other sessions' workspaces of
//! the same project included: owner decision 2026-11-08), the referenced library roots, the
//! session directory's `credential.json`, `settings/`, `tmp/`, `provider/` and `git/`, and the
//! harness's sign-in store (detection's host sandbox also reads the whole sign-in home, plan 7D
//! Decision 19). Writes: the session's workspace, `workspace/shared/` (not in
//! `plan`), `tmp/`, `provider/` and the sign-in store file. Protected: the project's state
//! (`.finisher/`, `assets/`, `finisher.project.json`, `finisher.rules.json`) and the session's
//! credential and settings. Denied as a second layer: server data (the session directory's
//! listed files and the store file are holes in it), the sensitive home paths and, for Claude
//! Code, the project's `CLAUDE.md`, `CLAUDE.local.md` and `.claude/`.

use std::path::{Path, PathBuf};

use fin_core::events::payloads::agent::{AgentMode, Harness};

use super::{component, exactly, finish, inside, not_over, not_under, path, paths, sensitive_home};
use crate::error::PolicyError;
use crate::inputs::{AgentInput, FinEndpoint};
use crate::policy::{Limits, Net, Os, Profile, ReadPolicy, SandboxPolicy, headless_devices, under};

/// The files and directories of the project only `finisherd` writes (arch §27.6, 7B rules).
pub const PROJECT_STATE: &[&str] = &[
    ".finisher",
    "assets",
    "finisher.project.json",
    "finisher.rules.json",
];
/// What Claude Code would load from the project folder by itself (spec §4.5).
pub const CLAUDE_PROJECT_FILES: &[&str] = &["CLAUDE.md", "CLAUDE.local.md", ".claude"];
/// A host sandbox's id prefix (`agents/host-<ulid>/`, plan 7D Task 3b; `session-dirs.ts`).
const HOST_PREFIX: &str = "host-";
/// The session directory's entries the sandbox reads (spec §8.5); `blobs/` is not one.
const SESSION_READ: &[&str] = &["credential.json", "settings", "tmp", "provider", "git"];

pub fn agent(i: &AgentInput) -> Result<SandboxPolicy, PolicyError> {
    agent_on(Os::current(), i)
}

fn harness_dir(h: Harness) -> Result<&'static str, PolicyError> {
    match h {
        Harness::ClaudeCode => Ok("claudeCode"),
        Harness::Codex => Ok("codex"),
        Harness::Fake => Ok("fake"),
        Harness::Builtin | Harness::Plugin => Err(PolicyError::input(
            "harness",
            format!("{h:?} has no agent sandbox profile (the built-in agent needs none)"),
        )),
    }
}

pub fn agent_on(os: Os, i: &AgentInput) -> Result<SandboxPolicy, PolicyError> {
    let harness = harness_dir(i.harness)?;
    component("sessionId", &i.session_id)?;
    if i.session_id == "shared" {
        return Err(PolicyError::input(
            "sessionId",
            "\"shared\" names the shared workspace",
        ));
    }
    for (field, p) in [
        ("projectDir", &i.project_dir),
        ("workspaceDir", &i.workspace_dir),
        ("sessionDir", &i.session_dir),
        ("serverData", &i.server_data),
        ("home", &i.home),
    ] {
        path(field, p)?;
    }
    paths("sharedDir", &i.shared_dir)?;
    paths("signInStore", &i.sign_in_store)?;
    paths("signInHome", &i.sign_in_home)?;
    paths("toolchains", &i.toolchains)?;
    paths("libraryRoots", &i.library_roots)?;

    let workspaces = under(&i.project_dir, "workspace");
    exactly(
        "workspaceDir",
        &i.workspace_dir,
        &workspaces.join(&i.session_id),
    )?;
    if let Some(s) = &i.shared_dir {
        exactly("sharedDir", s, &workspaces.join("shared"))?;
    }
    exactly(
        "sessionDir",
        &i.session_dir,
        &under(&i.server_data, "agents").join(&i.session_id),
    )?;
    let home = under(&i.server_data, &format!("providers/{harness}/home"));
    if let Some(store) = &i.sign_in_store {
        inside("signInStore", store, &home)?;
    }
    if let Some(h) = &i.sign_in_home {
        detect_home(os, i, h, &home)?;
    }

    let mut deny = vec![i.server_data.clone()];
    deny.extend(sensitive_home(os, &i.home));
    if i.harness == Harness::ClaudeCode {
        deny.extend(
            CLAUDE_PROJECT_FILES
                .iter()
                .map(|f| under(&i.project_dir, f)),
        );
    }
    // Only the session's own files and the store are holes in the denied paths.
    not_under("projectDir", [&i.project_dir], &deny)?;
    not_under("toolchains", &i.toolchains, &deny)?;
    // Nor above one, or the home: a process's `extraRead` (the CLI's install, plan 7D Task 14)
    // is a toolchain, and `$HOME` or `/` there would grant all of it but `deny_read`.
    let over: Vec<PathBuf> = deny.iter().chain([&i.home]).cloned().collect();
    not_over("toolchains", &i.toolchains, &over)?;
    let own = harness_reads(os, i.harness);
    not_under("harness toolchains", &own, &deny)?;
    not_under("libraryRoots", &i.library_roots, &deny)?;

    let mut read: Vec<PathBuf> = i.toolchains.clone();
    read.extend(own);
    read.push(i.project_dir.clone());
    read.extend(i.library_roots.iter().cloned());
    read.extend(SESSION_READ.iter().map(|f| under(&i.session_dir, f)));
    read.extend(i.sign_in_store.iter().cloned());
    read.extend(i.sign_in_home.iter().cloned());

    let mut write = vec![
        i.workspace_dir.clone(),
        under(&i.session_dir, "tmp"),
        under(&i.session_dir, "provider"),
    ];
    if i.mode != AgentMode::Plan {
        write.extend(i.shared_dir.iter().cloned());
    }
    write.extend(i.sign_in_store.iter().cloned());

    let mut protect: Vec<PathBuf> = PROJECT_STATE
        .iter()
        .map(|f| under(&i.project_dir, f))
        .collect();
    protect.push(under(&i.session_dir, "credential.json"));
    protect.push(under(&i.session_dir, "settings"));

    let (net, unix_sockets) = match &i.fin {
        Some(FinEndpoint::Tcp { port }) => (
            Net::Open {
                loopback: vec![*port],
            },
            vec![],
        ),
        Some(FinEndpoint::UnixSocket { path: sock }) => {
            path("fin.path", sock)?;
            (Net::Open { loopback: vec![] }, vec![sock.clone()])
        }
        Some(FinEndpoint::Pipe) | None => (Net::Open { loopback: vec![] }, vec![]),
    };

    finish(SandboxPolicy {
        profile: Profile::Agent,
        write,
        read: ReadPolicy::Only(read),
        protect,
        deny_read: deny,
        unix_sockets,
        net,
        // The macOS allow-list, data from spike S9 B6 (plan 7C Task 8; empty elsewhere).
        mach: crate::data::mach(os, Profile::Agent),
        devices: headless_devices(os),
        limits: Limits::default(),
    })
}

/// The harness's own CLI reads (`data/<os>/toolchain-<harness>.txt`, plan 7D Task 14), as the
/// policy takes them. An entry that is not absolute on this host (a Linux list built for a
/// golden on Windows) or starts with a Windows variable (`%…%`, expanded by the broker's
/// caller) is left out; a Linux entry that is not on this host is skipped by the kernel layer
/// like any root.
fn harness_reads(os: Os, h: Harness) -> Vec<PathBuf> {
    crate::data::toolchain_harness(os, h)
        .into_iter()
        .filter(|e| !e.starts_with('%'))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .collect()
}

/// A ULID: 26 characters of Crockford's base 32, upper case, the first at most `7`.
fn ulid(s: &str) -> bool {
    s.len() == 26
        && s.as_bytes()[0] <= b'7'
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_uppercase() && !b"ILOU".contains(&b)))
}

/// Plan 7D Decision 19 (review of the follow-up, S1): only detection's host sandbox reads the
/// sign-in home (Claude Code's `auth status` finds the account's profile there; verified on
/// 2.1.321 that a read-only home still answers): a Claude Code `host-<ULID>`, `plan`, no `fin`,
/// the home exactly. Never a session's; not on Windows, where a store is granted to the
/// harness's sign-in capability that every session holds (TODO(7D Task 13)).
fn detect_home(os: Os, i: &AgentInput, given: &Path, home: &Path) -> Result<(), PolicyError> {
    let host = i.session_id.strip_prefix(HOST_PREFIX).is_some_and(ulid);
    let why = if i.harness != Harness::ClaudeCode {
        "only Claude Code's detection reads its sign-in home"
    } else if !host || i.mode != AgentMode::Plan || i.fin.is_some() {
        "only detection's host sandbox (host-<ULID>, plan, no fin) reads the sign-in home"
    } else if os == Os::Windows {
        "not on Windows (TODO(7D Task 13))"
    } else if given != home {
        "must be <serverData>/providers/claudeCode/home"
    } else {
        return Ok(());
    };
    Err(PolicyError::input("signInHome", why))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn input(root: &Path) -> AgentInput {
        let project = root.join("p");
        let data = root.join("data");
        AgentInput {
            session_id: "s1".into(),
            mode: AgentMode::Draft,
            harness: Harness::Codex,
            workspace_dir: project.join("workspace").join("s1"),
            shared_dir: Some(project.join("workspace").join("shared")),
            project_dir: project,
            library_roots: vec![],
            session_dir: data.join("agents").join("s1"),
            sign_in_home: None,
            sign_in_store: None,
            toolchains: vec![],
            server_data: data,
            home: root.join("home"),
            fin: None,
        }
    }

    #[test]
    fn a_toolchain_root_over_the_home_or_a_denied_path_is_refused() {
        // A process's `extraRead` (the CLI's install, plan 7D Task 14 review) joins the
        // toolchains: a root that holds the home or a denied path would hand the session
        // everything else beside it (deny_read takes out only the listed paths).
        let root = PathBuf::from(if cfg!(windows) { "C:\\r" } else { "/r" });
        let top = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
        let home = root.join("home");
        let mut i = input(&root);
        i.harness = Harness::ClaudeCode;
        for bad in [
            top,
            root.clone(),
            home.clone(),
            home.join(".config"),
            home.join(".local").join("share"),
            root.join("p"),
            root.join("data").join(".."),
        ] {
            i.toolchains = vec![bad.clone()];
            let e = agent_on(Os::Linux, &i).unwrap_err().to_string();
            assert!(e.contains("toolchains"), "{bad:?}: {e}");
        }
        for ok in [
            home.join(".local")
                .join("share")
                .join("claude")
                .join("versions"),
            home.join("claude"),
            root.join("opt").join("claude"),
        ] {
            i.toolchains = vec![ok.clone()];
            assert!(agent_on(Os::Linux, &i).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn every_os_denies_its_sensitive_paths() {
        let root = PathBuf::from(if cfg!(windows) { "C:\\r" } else { "/r" });
        for (os, rel) in [
            (Os::Linux, ".mozilla"),
            (Os::Macos, "Library/Keychains"),
            (Os::Windows, "AppData/Roaming/Microsoft/Protect"),
        ] {
            // Each OS's device paths are absolute only on its own family.
            if cfg!(windows) != (os == Os::Windows) {
                continue;
            }
            let p = agent_on(os, &input(&root)).unwrap();
            assert!(
                p.deny_read.contains(&under(&root.join("home"), rel)),
                "{os:?}"
            );
            assert!(
                p.deny_read.contains(&under(&root.join("home"), ".codex")),
                "{os:?}"
            );
        }
    }

    #[test]
    fn claude_code_reads_its_own_list_and_no_other_harness_does() {
        let root = PathBuf::from(if cfg!(windows) { "C:\\r" } else { "/r" });
        let mut claude = input(&root);
        claude.harness = Harness::ClaudeCode;
        for mode in [AgentMode::Draft, AgentMode::Plan] {
            claude.mode = mode;
            let p = agent_on(Os::Macos, &claude).unwrap();
            assert!(
                !p.read_roots().iter().any(|r| r.starts_with("/proc")),
                "macOS lists nothing yet"
            );
        }
        if cfg!(windows) {
            return; // a Linux list's paths are not absolute on Windows
        }
        let own: Vec<PathBuf> = crate::data::toolchain_harness(Os::Linux, Harness::ClaudeCode)
            .into_iter()
            .map(PathBuf::from)
            .collect();
        assert!(own.contains(&PathBuf::from("/proc/stat")));
        let p = agent_on(Os::Linux, &claude).unwrap();
        for e in &own {
            assert!(p.read_roots().contains(e), "{}", e.display());
        }
        let other = agent_on(Os::Linux, &input(&root)).unwrap();
        assert!(!other.read_roots().contains(&PathBuf::from("/proc/stat")));
    }
}
