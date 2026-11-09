//! What a process of each profile gets on Windows (plan 7C Decision 16): its container, its
//! capabilities and, for processes the launcher starts, where `LOCALAPPDATA` (and with it
//! `TEMP`) points. Pure: built from the profile's input, tested on every OS; the Windows
//! layers ([`super::apply`], the spawner) only carry it out.
//!
//! The grants themselves are the ACL broker's (plan 7C Task 10): the callers grant each
//! capability on the roots of the table in [`super::capability`] before the start.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::capability::{self, INTERNET_CLIENT, TOOLCHAIN, capability_sid};
use crate::inputs::{AgentInput, HookInput, PluginInput, WorkerInput, WorkerScope};
use crate::launch::LaunchRequest;
use crate::policy::{Net, SandboxPolicy};

/// One capability of a process: its name (none for a well-known one) and its SID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub name: Option<String>,
    pub sid: String,
}

impl Capability {
    fn named(name: String) -> Capability {
        let sid = capability_sid(&name);
        Capability {
            name: Some(name),
            sid,
        }
    }

    fn internet() -> Capability {
        Capability {
            name: None,
            sid: INTERNET_CLIENT.into(),
        }
    }
}

/// A process's AppContainer identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerPlan {
    /// The AppContainer profile name.
    pub container: String,
    pub capabilities: Vec<Capability>,
    /// The launcher's `LOCALAPPDATA` for the process (S9 point 7: Windows rewrites `TEMP` to
    /// `<LOCALAPPDATA>\Packages\<container>\AC\Temp`, so without it `TEMP` is per project).
    /// For workers and plugins it lies in their private temp too (review: otherwise every job
    /// of a family, across projects, and every call of a plugin would share one `TEMP`).
    pub local_app_data: Option<PathBuf>,
}

impl ContainerPlan {
    /// The temp directory Windows gives the process: `<LOCALAPPDATA>\Packages\<container>\AC\Temp`.
    pub fn temp(&self) -> Option<PathBuf> {
        self.local_app_data.as_ref().map(|l| {
            l.join("Packages")
                .join(&self.container)
                .join("AC")
                .join("Temp")
        })
    }

    /// The capability of the process's own owner (its session, hook call, job, warm worker or
    /// plugin call), which its process DACL names besides the user (review B1): not the
    /// toolchain's, a project view's, a sign-in store's or a well-known one.
    pub fn owner(&self) -> Option<&Capability> {
        self.capabilities.iter().find(|c| {
            c.name.as_deref().is_some_and(|n| {
                n != TOOLCHAIN
                    && !n.starts_with("finisher-signin-")
                    && !n.starts_with("finisher-pv-")
            })
        })
    }

    pub fn has_internet(&self) -> bool {
        self.capabilities.iter().any(|c| c.sid == INTERNET_CLIENT)
    }
}

/// The session directory's `LOCALAPPDATA` (`finisherd`'s `session-dirs.ts` makes it, plan 7C
/// Task 11).
pub const AGENT_APPDATA: &str = "localappdata";
/// The directory under a hook's private temp that becomes its `LOCALAPPDATA`.
const APPDATA: &str = "appdata";

/// The plan of a launch request; `policy` is the request's own (for the network).
pub fn launch(req: &LaunchRequest, policy: &SandboxPolicy) -> Result<ContainerPlan, String> {
    match req {
        LaunchRequest::Agent(i) => agent(i, policy),
        LaunchRequest::Hook(i) => hook(i),
    }
}

/// `agent`: the project's container; the session's capability, the toolchain's, the harness's
/// sign-in capability when it has a store, `internetClient` for open egress.
pub fn agent(i: &AgentInput, policy: &SandboxPolicy) -> Result<ContainerPlan, String> {
    let mut caps = vec![
        Capability::named(capability::session(&i.session_id)?),
        Capability::named(TOOLCHAIN.into()),
        Capability::named(capability::project_view(&i.project_dir, &policy.deny_read)),
    ];
    if i.sign_in_store.is_some() {
        let harness = serde_json::to_value(i.harness)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or("the harness has no name")?;
        caps.push(Capability::named(capability::sign_in(&harness)?));
    }
    if matches!(policy.net, Net::Open { .. }) {
        caps.push(Capability::internet());
    }
    Ok(ContainerPlan {
        container: capability::project_container(&i.project_dir),
        capabilities: caps,
        local_app_data: Some(i.session_dir.join(AGENT_APPDATA)),
    })
}

/// `hook`: the project's container; the toolchain's capability and the call's own. No
/// `internetClient` (a hook reaches `fin` through the pipe, whose DACL admits the container).
pub fn hook(i: &HookInput) -> Result<ContainerPlan, String> {
    let policy = crate::profile::hook(i).map_err(|e| e.to_string())?;
    Ok(ContainerPlan {
        container: capability::project_container(&i.project_dir),
        capabilities: vec![
            Capability::named(TOOLCHAIN.into()),
            Capability::named(capability::hook(&i.tmp)),
            Capability::named(capability::project_view(&i.project_dir, &policy.deny_read)),
        ],
        local_app_data: Some(i.tmp.join(APPDATA)),
    })
}

fn last(p: &Path) -> Result<&str, String> {
    p.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no name", p.display()))
}

/// `worker`: the family's container and one capability for its job (cold) or itself (warm).
/// No toolchain capability and no network (S9 points 9, 10: D3D12, DXVA decode and the
/// encoders need none).
pub fn worker(i: &WorkerInput, family: &str) -> Result<ContainerPlan, String> {
    let cap = match &i.scope {
        WorkerScope::Cold { job_dir, .. } => capability::job(last(job_dir)?)?,
        WorkerScope::Warm { worker_dir, .. } => capability::warm_worker(last(worker_dir)?)?,
    };
    Ok(ContainerPlan {
        container: capability::worker_container(family)?,
        capabilities: vec![Capability::named(cap)],
        local_app_data: Some(i.tmp.join(APPDATA)),
    })
}

/// `plugin`: the (plugin, project) container, the toolchain's capability (the runtime) and
/// the call's. No network.
pub fn plugin(i: &PluginInput) -> Result<ContainerPlan, String> {
    Ok(ContainerPlan {
        container: capability::plugin_container(
            &i.store_dir,
            i.plugin_data.as_ref().map(|d| d.dir.as_path()),
        ),
        capabilities: vec![
            Capability::named(TOOLCHAIN.into()),
            Capability::named(capability::call(last(&i.call_dir)?)?),
        ],
        local_app_data: Some(i.tmp.join(APPDATA)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Limits;

    fn policy(net: Net) -> SandboxPolicy {
        SandboxPolicy {
            profile: crate::policy::Profile::Agent,
            write: vec![],
            read: crate::policy::ReadPolicy::Only(vec![]),
            protect: vec![],
            deny_read: vec![],
            unix_sockets: vec![],
            net,
            mach: vec![],
            devices: vec![],
            limits: Limits::default(),
        }
    }

    #[test]
    fn only_an_agent_with_open_egress_gets_internet_client() {
        let r = PathBuf::from(if cfg!(windows) { r"C:\r" } else { "/r" });
        let i = AgentInput {
            session_id: "s1".into(),
            mode: fin_core::events::payloads::agent::AgentMode::Draft,
            harness: fin_core::events::payloads::agent::Harness::Codex,
            project_dir: r.join("p"),
            workspace_dir: r.join("p").join("workspace").join("s1"),
            shared_dir: None,
            library_roots: vec![],
            session_dir: r.join("data").join("agents").join("s1"),
            sign_in_store: None,
            toolchains: vec![],
            server_data: r.join("data"),
            home: r.join("home"),
            fin: None,
        };
        assert!(
            agent(&i, &policy(Net::Open { loopback: vec![] }))
                .unwrap()
                .has_internet()
        );
        assert!(!agent(&i, &policy(Net::None)).unwrap().has_internet());
        let h = HookInput {
            project_dir: r.join("p"),
            hook_file: r.join("p").join("h.cmd"),
            toolchains: vec![],
            tmp: r.join("t"),
            server_data: r.join("data"),
            home: r.join("home"),
            fin: None,
            timeout_ms: None,
        };
        assert!(!hook(&h).unwrap().has_internet());
        let p = PluginInput {
            store_dir: r.join("data").join("plugins").join("store").join("a"),
            runtime: vec![],
            system: vec![],
            onnx_runtime: None,
            models: vec![],
            call_dir: r.join("data").join("calls").join("c1"),
            tmp: r.join("data").join("calls").join("c1").join("tmp"),
            plugin_data: None,
            gpu: vec![],
            memory_bytes: None,
            server_data: r.join("data"),
            home: r.join("home"),
        };
        assert!(!plugin(&p).unwrap().has_internet());
        // Each process's own capability, for its DACL.
        let own = |c: &ContainerPlan| c.owner().and_then(|o| o.name.clone()).unwrap();
        assert_eq!(
            own(&agent(&i, &policy(Net::None)).unwrap()),
            "finisher-session-s1"
        );
        assert_eq!(own(&hook(&h).unwrap()), capability::hook(&h.tmp));
        assert_eq!(own(&plugin(&p).unwrap()), "finisher-call-c1");
    }
}
