//! What the ACL broker grants for one launch (plan 7C Decisions 16 and 18; S9 Windows points
//! 1, 2 and 12). Pure: built from the profile's input and tested on every OS; `finisherd`
//! hands each entry to the broker (`aclGrant`) before the start and revokes the `call` and
//! `session` ones after it.
//!
//! An AppContainer only *adds* access, so every root a process may use is one allow ACE, and
//! the mask follows the policy (the project is read through a view capability, never the
//! shared container SID, review B2): a write root gets [`MODIFY`] (no `WRITE_DAC`, no
//! `WRITE_OWNER`), a read root [`READ`] (data) or [`READ_EXECUTE`] (toolchains, the project).
//! Each group of roots is enumerated against the policy first (plan Decision 2), so a denied
//! root inside a granted one is not reached and a protected root is never inside a write
//! grant.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::{self, TOOLCHAIN, capability_sid};
use super::plan;
use crate::enumerate::{Fs, Tree, read_roots, write_roots};
use crate::inputs::{AgentInput, HookInput};
use crate::policy::SandboxPolicy;

/// `FILE_GENERIC_READ`.
pub const READ: u32 = 0x0012_0089;
/// Read and execute.
pub const READ_EXECUTE: u32 = 0x0012_00A9;
/// Modify: read, write, execute and delete; no `WRITE_DAC`, no `WRITE_OWNER` (S9 point 1).
pub const MODIFY: u32 = 0x0013_01BF;

/// Whom an ACE names: a SID (a capability's, derived here) or a container by name (its SID
/// is Windows', [`super::appcontainer::sid`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
#[schemars(rename = "AclTrustee")]
pub enum Trustee {
    Sid(String),
    Container(String),
}

/// How long a grant lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "AclScope")]
pub enum Scope {
    /// Shared by every launch of its kind (the toolchain, a project's folder, a sign-in
    /// store): granted once, kept.
    Static,
    /// This session's or this call's own: revoked when it ends.
    Owner,
}

/// One allow ACE, inherited by the subtree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AclGrant {
    pub path: PathBuf,
    pub trustee: Trustee,
    pub mask: u32,
    pub scope: Scope,
}

/// One group of roots granted alike, before enumeration.
pub(super) struct Group {
    pub(super) roots: Vec<PathBuf>,
    pub(super) trustee: Trustee,
    pub(super) mask: u32,
    pub(super) scope: Scope,
}

pub(super) fn cap(name: String) -> Trustee {
    Trustee::Sid(capability_sid(&name))
}

/// The ACEs of `groups`: each group's roots enumerated against the policy (plan Decision 2),
/// so a denied root inside a granted one (Claude Code's project files) is left out, and a
/// write group also leaves out the protected roots.
pub(super) fn expand(
    policy: &SandboxPolicy,
    groups: Vec<Group>,
    tree: &dyn Tree,
) -> Result<Vec<AclGrant>, String> {
    let denied = policy.denied();
    let mut out = Vec::new();
    for gr in groups {
        let roots = if gr.mask == MODIFY {
            write_roots(&gr.roots, &policy.protect, &denied, tree)
        } else {
            read_roots(&gr.roots, &denied, tree)
        }
        .map_err(|e| e.to_string())?;
        out.extend(roots.into_iter().map(|path| AclGrant {
            path,
            trustee: gr.trustee.clone(),
            mask: gr.mask,
            scope: gr.scope,
        }));
    }
    Ok(out)
}

/// The grants of an `agent` launch, enumerated on the real filesystem.
pub fn agent(i: &AgentInput) -> Result<Vec<AclGrant>, String> {
    agent_in(i, &[], &Fs)
}

/// [`agent`] for a process that also reads `extra` (a harness's own files; each also in
/// `i.toolchains`, so the policy reads it): granted to the session's capability, revoked with
/// the session (Task 10 review 4), never a static toolchain grant.
pub fn agent_with_extra(i: &AgentInput, extra: &[PathBuf]) -> Result<Vec<AclGrant>, String> {
    agent_in(i, extra, &Fs)
}

/// [`agent_with_extra`] over `tree`.
pub fn agent_in(
    i: &AgentInput,
    extra: &[PathBuf],
    tree: &dyn Tree,
) -> Result<Vec<AclGrant>, String> {
    if let Some(e) = extra.iter().find(|e| !i.toolchains.contains(e)) {
        return Err(format!(
            "{} is read by this process but not in its toolchains",
            e.display()
        ));
    }
    let policy = crate::profile::agent(i).map_err(|e| e.to_string())?;
    let plan = plan::agent(i, &policy)?;
    let session = cap(capability::session(&i.session_id)?);
    let view = cap(capability::project_view(&i.project_dir, &policy.deny_read));
    let mut project = vec![i.project_dir.clone()];
    project.extend(i.library_roots.iter().cloned());
    // The policy's own write roots (the shared workspace only outside `plan` mode), and
    // `LOCALAPPDATA`, which Windows alone needs.
    let mut write: Vec<PathBuf> = policy
        .write
        .iter()
        .filter(|w| Some(*w) != i.sign_in_store.as_ref())
        .cloned()
        .collect();
    write.extend(plan.local_app_data.iter().cloned());
    let base: Vec<PathBuf> = i
        .toolchains
        .iter()
        .filter(|t| !extra.contains(t))
        .cloned()
        .collect();
    let mut groups = vec![
        toolchains(&base),
        Group {
            roots: extra.to_vec(),
            trustee: session.clone(),
            mask: READ_EXECUTE,
            scope: Scope::Owner,
        },
        Group {
            roots: project,
            trustee: view,
            mask: READ_EXECUTE,
            scope: Scope::Static,
        },
        Group {
            roots: ["credential.json", "settings", "git"]
                .iter()
                .map(|f| i.session_dir.join(f))
                .collect(),
            trustee: session.clone(),
            mask: READ,
            scope: Scope::Owner,
        },
        Group {
            roots: write,
            trustee: session,
            mask: MODIFY,
            scope: Scope::Owner,
        },
    ];
    if let Some(store) = &i.sign_in_store {
        let name = plan
            .capabilities
            .iter()
            .filter_map(|c| c.name.clone())
            .find(|n| n.starts_with("finisher-signin-"))
            .ok_or("a sign-in store without its capability")?;
        groups.push(Group {
            roots: vec![store.clone()],
            trustee: cap(name),
            mask: MODIFY,
            scope: Scope::Static,
        });
    }
    expand(&policy, groups, tree)
}

/// The grants of a `hook` launch, enumerated on the real filesystem.
pub fn hook(i: &HookInput) -> Result<Vec<AclGrant>, String> {
    hook_in(i, &Fs)
}

/// [`hook`] over `tree`.
pub fn hook_in(i: &HookInput, tree: &dyn Tree) -> Result<Vec<AclGrant>, String> {
    let policy = crate::profile::hook(i).map_err(|e| e.to_string())?;
    let call = cap(capability::hook(&i.tmp));
    let mut own = vec![];
    if i.project_dir
        .as_ref()
        .is_none_or(|p| !i.hook_file.starts_with(p))
    {
        own.push(i.hook_file.clone());
    }
    let mut groups = vec![toolchains(&i.toolchains)];
    // A server-scoped event's hook reads no project.
    if let Some(p) = &i.project_dir {
        groups.push(Group {
            roots: vec![p.clone()],
            trustee: cap(capability::project_view(p, &policy.deny_read)),
            mask: READ_EXECUTE,
            scope: Scope::Static,
        });
    }
    groups.extend([
        Group {
            roots: own,
            trustee: call.clone(),
            mask: READ_EXECUTE,
            scope: Scope::Owner,
        },
        Group {
            roots: i.fin.iter().map(|f| f.credential.clone()).collect(),
            trustee: call.clone(),
            mask: READ,
            scope: Scope::Owner,
        },
        Group {
            roots: vec![i.tmp.clone()],
            trustee: call,
            mask: MODIFY,
            scope: Scope::Owner,
        },
    ]);
    expand(&policy, groups, tree)
}

pub(super) fn toolchains(roots: &[PathBuf]) -> Group {
    Group {
        roots: roots.to_vec(),
        trustee: cap(TOOLCHAIN.into()),
        mask: READ_EXECUTE,
        scope: Scope::Static,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::io;
    use std::path::Path;

    use fin_core::events::payloads::agent::{AgentMode, Harness};

    use super::*;
    use crate::enumerate::Kind;

    /// A model tree: the project holds `children`, every other directory is empty.
    struct Model {
        project: PathBuf,
        children: Vec<(&'static str, Kind)>,
    }

    impl Tree for Model {
        fn children(&self, dir: &Path) -> io::Result<Vec<(PathBuf, Kind)>> {
            if dir == self.project {
                return Ok(self
                    .children
                    .iter()
                    .map(|(n, k)| (dir.join(n), *k))
                    .collect());
            }
            Ok(vec![])
        }
    }

    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\r" } else { "/r" })
    }

    fn input(harness: Harness, mode: AgentMode) -> AgentInput {
        let r = root();
        let data = r.join("data");
        let p = r.join("p");
        AgentInput {
            session_id: "s1".into(),
            mode,
            harness,
            project_dir: p.clone(),
            workspace_dir: p.join("workspace").join("s1"),
            shared_dir: Some(p.join("workspace").join("shared")),
            library_roots: vec![r.join("lib")],
            session_dir: data.join("agents").join("s1"),
            sign_in_store: (harness == Harness::Codex).then(|| {
                data.join("providers")
                    .join("codex")
                    .join("home")
                    .join("auth.json")
            }),
            toolchains: vec![r.join("tools")],
            server_data: data,
            home: r.join("home"),
            fin: None,
        }
    }

    fn model(children: Vec<(&'static str, Kind)>) -> Model {
        Model {
            project: root().join("p"),
            children,
        }
    }

    /// `(path relative to the root, trustee, mask, scope)` of each grant.
    fn table(grants: &[AclGrant]) -> BTreeSet<(String, String, u32, Scope)> {
        let r = root();
        grants
            .iter()
            .map(|g| {
                let who = match &g.trustee {
                    Trustee::Sid(s) => s.clone(),
                    Trustee::Container(c) => format!("container:{c}"),
                };
                let rel = g
                    .path
                    .strip_prefix(&r)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                (rel, who, g.mask, g.scope)
            })
            .collect()
    }

    fn sid(name: &str) -> String {
        capability_sid(name)
    }

    impl PartialOrd for Scope {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Scope {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            (*self as u8).cmp(&(*o as u8))
        }
    }

    #[test]
    fn agent_grants_follow_the_policy() {
        let i = input(Harness::Codex, AgentMode::Draft);
        let c = sid(&capability::project_view(
            &i.project_dir,
            &crate::profile::agent(&i).unwrap().deny_read,
        ));
        let s1 = sid("finisher-session-s1");
        let want: BTreeSet<_> = [
            ("tools", sid(TOOLCHAIN), READ_EXECUTE, Scope::Static),
            ("p", c.clone(), READ_EXECUTE, Scope::Static),
            ("lib", c, READ_EXECUTE, Scope::Static),
            (
                "data/agents/s1/credential.json",
                s1.clone(),
                READ,
                Scope::Owner,
            ),
            ("data/agents/s1/settings", s1.clone(), READ, Scope::Owner),
            ("data/agents/s1/git", s1.clone(), READ, Scope::Owner),
            ("p/workspace/s1", s1.clone(), MODIFY, Scope::Owner),
            ("p/workspace/shared", s1.clone(), MODIFY, Scope::Owner),
            ("data/agents/s1/tmp", s1.clone(), MODIFY, Scope::Owner),
            ("data/agents/s1/provider", s1.clone(), MODIFY, Scope::Owner),
            ("data/agents/s1/localappdata", s1, MODIFY, Scope::Owner),
            (
                "data/providers/codex/home/auth.json",
                sid("finisher-signin-codex"),
                MODIFY,
                Scope::Static,
            ),
        ]
        .into_iter()
        .map(|(p, w, m, s)| (p.to_string(), w, m, s))
        .collect();
        assert_eq!(table(&agent_in(&i, &[], &model(vec![])).unwrap()), want);
    }

    #[test]
    fn extra_reads_are_the_sessions_not_static_toolchains() {
        let mut i = input(Harness::Codex, AgentMode::Draft);
        let own = root().join("cli");
        i.toolchains.push(own.clone());
        let t = table(&agent_in(&i, std::slice::from_ref(&own), &model(vec![])).unwrap());
        let s1 = sid("finisher-session-s1");
        assert!(
            t.contains(&("cli".into(), s1, READ_EXECUTE, Scope::Owner)),
            "{t:?}"
        );
        assert!(
            !t.iter()
                .any(|(p, w, ..)| p == "cli" && *w == sid(TOOLCHAIN)),
            "{t:?}"
        );
        assert!(t.contains(&("tools".into(), sid(TOOLCHAIN), READ_EXECUTE, Scope::Static)));
        // An extra read the policy does not read is refused.
        let i = input(Harness::Codex, AgentMode::Draft);
        assert!(agent_in(&i, &[own], &model(vec![])).is_err());
    }

    #[test]
    fn plan_mode_does_not_write_the_shared_workspace() {
        let i = input(Harness::Codex, AgentMode::Plan);
        let t = table(&agent_in(&i, &[], &model(vec![])).unwrap());
        assert!(!t.iter().any(|(p, ..)| p == "p/workspace/shared"), "{t:?}");
    }

    #[test]
    fn denied_project_files_are_left_out_of_the_project_grant() {
        // Claude Code's project files are denied (spec §4.5): the project grant is expanded
        // into its other children, and the project folder itself is not granted.
        let i = input(Harness::ClaudeCode, AgentMode::Draft);
        let m = model(vec![
            ("CLAUDE.md", Kind::File),
            ("sources", Kind::Dir),
            ("workspace", Kind::Dir),
        ]);
        let t = table(&agent_in(&i, &[], &m).unwrap());
        let c = sid(&capability::project_view(
            &i.project_dir,
            &crate::profile::agent(&i).unwrap().deny_read,
        ));
        let project: BTreeSet<_> = t
            .iter()
            .filter(|(_, w, ..)| *w == c)
            .map(|(p, ..)| p.as_str())
            .collect();
        assert_eq!(project, BTreeSet::from(["lib", "p/sources", "p/workspace"]));
    }

    #[test]
    fn nothing_else_of_server_data_or_home_is_granted() {
        let i = input(Harness::Codex, AgentMode::Draft);
        for (p, _, m, _) in table(&agent_in(&i, &[], &model(vec![])).unwrap()) {
            assert!(!p.starts_with("home"), "{p}");
            assert!(
                p != "data" && p != "data/agents" && !p.starts_with("data/agents/s2"),
                "{p}"
            );
            assert!(!p.ends_with("blobs"), "{p}");
            if p.starts_with("data/agents/s1/")
                && (p.ends_with("settings") || p.ends_with("credential.json"))
            {
                assert_eq!(m, READ, "{p} is protected");
            }
        }
    }

    #[test]
    fn hook_grants() {
        let r = root();
        let i = HookInput {
            project_dir: Some(r.join("p")),
            hook_file: r.join("hooks").join("h.cmd"),
            toolchains: vec![r.join("tools")],
            tmp: r.join("data").join("hooks").join("t1"),
            server_data: r.join("data"),
            home: r.join("home"),
            fin: Some(crate::inputs::HookFin {
                credential: r.join("data").join("hooks").join("t1.cred"),
                endpoint: crate::inputs::FinEndpoint::Pipe,
            }),
            timeout_ms: None,
        };
        let call = sid(&capability::hook(&i.tmp));
        let c = sid(&capability::project_view(
            i.project_dir.as_ref().unwrap(),
            &crate::profile::hook(&i).unwrap().deny_read,
        ));
        let want: BTreeSet<_> = [
            ("tools", sid(TOOLCHAIN), READ_EXECUTE, Scope::Static),
            ("p", c, READ_EXECUTE, Scope::Static),
            ("hooks/h.cmd", call.clone(), READ_EXECUTE, Scope::Owner),
            ("data/hooks/t1.cred", call.clone(), READ, Scope::Owner),
            ("data/hooks/t1", call, MODIFY, Scope::Owner),
        ]
        .into_iter()
        .map(|(p, w, m, s)| (p.to_string(), w, m, s))
        .collect();
        assert_eq!(table(&hook_in(&i, &model(vec![])).unwrap()), want);
    }

    #[test]
    fn server_scoped_hook_grants_no_project() {
        let r = root();
        let i = HookInput {
            project_dir: None,
            hook_file: r.join("hooks").join("h.cmd"),
            toolchains: vec![r.join("tools")],
            tmp: r.join("data").join("hooks").join("t1"),
            server_data: r.join("data"),
            home: r.join("home"),
            fin: None,
            timeout_ms: None,
        };
        let got = hook_in(&i, &model(vec![])).unwrap();
        assert!(
            got.iter().all(|g| !g.path.starts_with(r.join("p"))),
            "{got:?}"
        );
        assert!(got.iter().any(|g| g.path == i.hook_file));
        let plan = crate::windows::plan::hook(&i).unwrap();
        assert_eq!(plan.container, capability::SERVER_HOOKS_CONTAINER);
        assert_eq!(plan.capabilities.len(), 2);
    }
}

#[cfg(test)]
mod view_tests {
    use fin_core::events::payloads::agent::Harness;

    use super::*;

    #[test]
    fn harnesses_that_see_the_project_differently_read_it_through_different_capabilities() {
        let r = PathBuf::from(if cfg!(windows) { r"C:\r" } else { "/r" });
        let p = r.join("p");
        let claude = vec![p.join("CLAUDE.md"), p.join(".claude"), r.join("data")];
        let codex = vec![r.join("data")];
        assert_ne!(
            capability::project_view(&p, &claude),
            capability::project_view(&p, &codex)
        );
        assert_eq!(
            capability::project_view(&p, &codex),
            capability::project_view(&p, &[r.join("elsewhere")]),
            "only paths inside the project count"
        );
        let _ = Harness::ClaudeCode;
    }
}
