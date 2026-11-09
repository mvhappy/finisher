//! What the ACL broker grants for one job host process on Windows (plan 7C Decisions 16, 18
//! and 19; the Windows side of Task 12). Pure: built from the profile's input and tested on
//! every OS. `finisherd` hands each entry to the broker before the spawn (`EngineHost.spawn`),
//! revokes the process's own entries when it exits, and grants a job visiting a warm worker
//! [`visit`] for the attempt.
//!
//! - `worker`: its own capability (the job's when cold, the worker's when warm) on its write
//!   roots ([`MODIFY`]: its private temp, the job's `out/` or the worker's `jobs/`) and read
//!   roots ([`READ`]: the job's `in/` and sources, or its project's library roots); the family
//!   container on the server's caches (shared by every job of the family, [`READ`]) and on the
//!   program ([`READ_EXECUTE`]), both kept ([`Scope::Static`]).
//! - `plugin`: the call's capability on its directory's `in/` ([`READ`]), `out/` and its temp
//!   ([`MODIFY`]); the (plugin, project) container on the package ([`READ_EXECUTE`]), the
//!   models ([`READ`]) and the plugin's data (at its mode), kept; the toolchain capability on
//!   the runtime, the system roots, the ONNX runtime and the program ([`READ_EXECUTE`]), kept.
//!
//! Each group is enumerated against the profile's policy, as for agents and hooks.

use std::path::{Path, PathBuf};

use super::capability;
use super::grants::{
    AclGrant, Group, MODIFY, READ, READ_EXECUTE, Scope, Trustee, cap, expand, toolchains,
};
use super::plan;
use crate::enumerate::{Fs, Tree};
use crate::inputs::{PluginInput, WorkerInput, WorkerScope};

fn group(roots: Vec<PathBuf>, trustee: &Trustee, mask: u32, scope: Scope) -> Group {
    Group {
        roots,
        trustee: trustee.clone(),
        mask,
        scope,
    }
}

/// The grants of a `worker` process of `family` running `program`, on the real filesystem.
pub fn worker(i: &WorkerInput, family: &str, program: &Path) -> Result<Vec<AclGrant>, String> {
    worker_in(i, family, program, &Fs)
}

/// [`worker`] over `tree`.
pub fn worker_in(
    i: &WorkerInput,
    family: &str,
    program: &Path,
    tree: &dyn Tree,
) -> Result<Vec<AclGrant>, String> {
    let policy = crate::profile::worker(i).map_err(|e| e.to_string())?;
    let plan = plan::worker(i, family)?;
    let own = cap(plan
        .owner()
        .and_then(|c| c.name.clone())
        .ok_or("the worker's plan names no capability of its own")?);
    let container = Trustee::Container(plan.container.clone());
    let (write, read) = match &i.scope {
        WorkerScope::Cold { job_dir, sources } => {
            let mut read = vec![job_dir.join("in")];
            read.extend(sources.iter().cloned());
            (vec![i.tmp.clone(), job_dir.join("out")], read)
        }
        WorkerScope::Warm {
            worker_dir,
            library_roots,
        } => (
            vec![i.tmp.clone(), worker_dir.join("jobs")],
            library_roots.clone(),
        ),
    };
    let groups = vec![
        group(write, &own, MODIFY, Scope::Owner),
        group(read, &own, READ, Scope::Owner),
        group(i.caches.clone(), &container, READ, Scope::Static),
        group(
            vec![program.to_owned()],
            &container,
            READ_EXECUTE,
            Scope::Static,
        ),
    ];
    expand(&policy, groups, tree)
}

/// The grants of a `plugin` process running `program`, on the real filesystem.
pub fn plugin(i: &PluginInput, program: &Path) -> Result<Vec<AclGrant>, String> {
    plugin_in(i, program, &Fs)
}

/// [`plugin`] over `tree`.
pub fn plugin_in(
    i: &PluginInput,
    program: &Path,
    tree: &dyn Tree,
) -> Result<Vec<AclGrant>, String> {
    let policy = crate::profile::plugin(i).map_err(|e| e.to_string())?;
    let plan = plan::plugin(i)?;
    let call = cap(plan
        .owner()
        .and_then(|c| c.name.clone())
        .ok_or("the plugin's plan names no capability of its own")?);
    let container = Trustee::Container(plan.container.clone());
    let mut runtime = vec![program.to_owned()];
    runtime.extend(i.runtime.iter().cloned());
    runtime.extend(i.system.iter().cloned());
    runtime.extend(i.onnx_runtime.iter().cloned());
    let mut groups = vec![
        toolchains(&runtime),
        group(vec![i.call_dir.join("in")], &call, READ, Scope::Owner),
        group(
            vec![i.call_dir.join("out"), i.tmp.clone()],
            &call,
            MODIFY,
            Scope::Owner,
        ),
        group(
            vec![i.store_dir.clone()],
            &container,
            READ_EXECUTE,
            Scope::Static,
        ),
        group(i.models.clone(), &container, READ, Scope::Static),
    ];
    if let Some(d) = &i.plugin_data {
        let mask = if d.write { MODIFY } else { READ };
        groups.push(group(vec![d.dir.clone()], &container, mask, Scope::Static));
    }
    expand(&policy, groups, tree)
}

/// The grant of a job visiting a warm worker (Decision 19): its directory, moved into the
/// worker's `jobs/` for the attempt, keeps the access it had (a move does not re-inherit), so
/// the worker's capability is granted on it ([`MODIFY`]) until the attempt ends.
pub fn visit(worker_dir: &Path, job_dir: &Path) -> Result<AclGrant, String> {
    let name = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .ok_or_else(|| format!("{} has no name", p.display()))
    };
    if job_dir.parent() != Some(&worker_dir.join("jobs")) {
        return Err(format!(
            "{} is not a job visiting {}",
            job_dir.display(),
            worker_dir.display()
        ));
    }
    capability::check_component("jobId", &name(job_dir)?)?;
    Ok(AclGrant {
        path: job_dir.to_owned(),
        trustee: cap(capability::warm_worker(&name(worker_dir)?)?),
        mask: MODIFY,
        scope: Scope::Owner,
    })
}

#[cfg(test)]
#[path = "job_grants_tests.rs"]
mod tests;
