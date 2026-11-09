//! The `agent` profile (spec §8.1, §8.5, §8.6; plan 7C Decision 6).
//!
//! Reads, by allow-list: the toolchains, the project folder (other sessions' workspaces of
//! the same project included: owner decision 2026-11-08), the referenced library roots, the
//! session directory's `credential.json`, `settings/`, `tmp/`, `provider/` and `git/`, and the
//! harness's sign-in store file. Writes: the session's workspace, `workspace/shared/` (not in
//! `plan`), `tmp/`, `provider/` and the sign-in store file. Protected: the project's state
//! (`.finisher/`, `assets/`, `finisher.project.json`, `finisher.rules.json`) and the session's
//! credential and settings. Denied as a second layer: server data (the session directory's
//! listed files and the store file are holes in it), the sensitive home paths and, for Claude
//! Code, the project's `CLAUDE.md`, `CLAUDE.local.md` and `.claude/`.

use std::path::PathBuf;

use fin_core::events::payloads::agent::{AgentMode, Harness};

use super::{component, exactly, finish, inside, not_under, path, paths, sensitive_home};
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
    if let Some(store) = &i.sign_in_store {
        inside(
            "signInStore",
            store,
            &under(&i.server_data, &format!("providers/{harness}/home")),
        )?;
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
    not_under("libraryRoots", &i.library_roots, &deny)?;

    let mut read: Vec<PathBuf> = i.toolchains.clone();
    read.push(i.project_dir.clone());
    read.extend(i.library_roots.iter().cloned());
    read.extend(SESSION_READ.iter().map(|f| under(&i.session_dir, f)));
    read.extend(i.sign_in_store.iter().cloned());

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
            sign_in_store: None,
            toolchains: vec![],
            server_data: data,
            home: root.join("home"),
            fin: None,
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
}
