//! The `hook` profile (spec §8.10; plan 7C Decision 7): the project read-only, the hook's own
//! file, its toolchain and a private temp. No network and no credential unless the hook
//! declares `fin`; then only `fin`'s endpoint (Linux: the agent listener's port; macOS:
//! `fin.sock`; Windows: the pipe), never IP egress.

use super::{finish, not_under, path, paths, sensitive_home};
use crate::error::PolicyError;
use crate::inputs::{FinEndpoint, HookInput};
use crate::policy::{Limits, Net, Os, Profile, ReadPolicy, SandboxPolicy, headless_devices};

pub fn hook(i: &HookInput) -> Result<SandboxPolicy, PolicyError> {
    hook_on(Os::current(), i)
}

pub fn hook_on(os: Os, i: &HookInput) -> Result<SandboxPolicy, PolicyError> {
    for (field, p) in [
        ("projectDir", &i.project_dir),
        ("hookFile", &i.hook_file),
        ("tmp", &i.tmp),
        ("serverData", &i.server_data),
        ("home", &i.home),
    ] {
        path(field, p)?;
    }
    paths("toolchains", &i.toolchains)?;

    let mut deny = vec![i.server_data.clone()];
    deny.extend(sensitive_home(os, &i.home));
    not_under("projectDir", [&i.project_dir], &deny)?;
    not_under("toolchains", &i.toolchains, &deny)?;
    // The hook file, the temp and the credential may sit in server data (holes), never in a
    // sensitive home path.
    not_under("hookFile", [&i.hook_file], &deny[1..])?;
    not_under("tmp", [&i.tmp], &deny[1..])?;

    let mut read = i.toolchains.clone();
    read.push(i.project_dir.clone());
    read.push(i.hook_file.clone());
    let (net, unix_sockets) = match &i.fin {
        None => (Net::None, vec![]),
        Some(fin) => {
            path("fin.credential", &fin.credential)?;
            not_under("fin.credential", [&fin.credential], &deny[1..])?;
            read.push(fin.credential.clone());
            match &fin.endpoint {
                FinEndpoint::Tcp { port } => (Net::Loopback { ports: vec![*port] }, vec![]),
                FinEndpoint::UnixSocket { path: sock } => {
                    path("fin.endpoint.path", sock)?;
                    (Net::None, vec![sock.clone()])
                }
                FinEndpoint::Pipe => (Net::None, vec![]),
            }
        }
    };

    finish(SandboxPolicy {
        profile: Profile::Hook,
        write: vec![i.tmp.clone()],
        read: ReadPolicy::Only(read),
        protect: vec![],
        deny_read: deny,
        unix_sockets,
        net,
        mach: crate::data::mach(os, Profile::Hook),
        devices: headless_devices(os),
        limits: Limits::default(),
    })
}
