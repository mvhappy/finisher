//! The `worker` profile (spec §8.10, sp6 §3.5; plan 7C Decisions 8 and 19).
//!
//! Cold: the manifest's sources, the caches and the job's `in/` read-only; the job's `out/` and
//! a private temp writable. Warm: the worker's own directory (its jobs are created under it),
//! never the jobs root, and one project's sources and library roots. No network, one process
//! (creation denied by the OS layers), no pathname socket. GPU rows come from sub-project 6.

use std::path::PathBuf;

use super::{finish, inside, not_under, path, paths};
use crate::error::PolicyError;
use crate::inputs::{WorkerInput, WorkerScope};
use crate::policy::{Limits, Net, Os, Profile, ReadPolicy, SandboxPolicy, under};

pub fn worker(i: &WorkerInput) -> Result<SandboxPolicy, PolicyError> {
    worker_on(Os::current(), i)
}

pub fn worker_on(os: Os, i: &WorkerInput) -> Result<SandboxPolicy, PolicyError> {
    path("tmp", &i.tmp)?;
    path("serverData", &i.server_data)?;
    paths("caches", &i.caches)?;
    paths("devices", i.devices.iter().map(|d| &d.path))?;
    let jobs = under(&i.server_data, "jobs");
    let deny = vec![i.server_data.clone()];

    let mut read: Vec<PathBuf> = i.caches.clone();
    let mut write = vec![i.tmp.clone()];
    match &i.scope {
        WorkerScope::Cold { job_dir, sources } => {
            path("scope.jobDir", job_dir)?;
            paths("scope.sources", sources)?;
            inside("scope.jobDir", job_dir, &jobs)?;
            not_under("scope.sources", sources, &deny)?;
            read.extend(sources.iter().cloned());
            read.push(job_dir.join("in"));
            write.push(job_dir.join("out"));
        }
        WorkerScope::Warm {
            worker_dir,
            library_roots,
        } => {
            path("scope.workerDir", worker_dir)?;
            paths("scope.libraryRoots", library_roots)?;
            // Never the jobs root itself (Decision 19, 5C-1 Task 11 review H3).
            inside("scope.workerDir", worker_dir, &jobs)?;
            not_under("scope.libraryRoots", library_roots, &deny)?;
            read.extend(library_roots.iter().cloned());
            write.push(worker_dir.clone());
        }
    }

    finish(SandboxPolicy {
        profile: Profile::Worker,
        write,
        read: ReadPolicy::Only(read),
        protect: vec![],
        deny_read: deny,
        unix_sockets: vec![],
        net: Net::None,
        // macOS: VideoToolbox and Metal (spike S9 B6); empty elsewhere.
        mach: crate::data::mach(os, Profile::Worker),
        devices: i.devices.clone(),
        limits: Limits {
            processes: Some(1),
            ..Limits::default()
        },
    })
}
