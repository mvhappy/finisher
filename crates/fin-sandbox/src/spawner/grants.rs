//! From the job host's grants to a profile input (plan 7C Task 7, "Design"). Pure: every
//! grant is placed in the profile's input or the request is refused, so nothing is widened or
//! dropped silently.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use fin_jobhost::spawner::{PathGrant, SpawnRequest};
use fin_jobs::FileMode;

use crate::inputs::{PluginData, PluginInput, WorkerInput, WorkerScope};
use crate::policy::{DeviceAccess, DeviceRule, under};

/// The family whose workers get the GPU rows (sp6 §3.5.1).
pub const MEDIA_FAMILY: &str = "media";
/// The directories of `<serverData>` a worker may be granted to read (its caches): the font
/// cache and the media caches. Any other `read` grant inside server data is refused.
pub const CACHE_DIRS: &[&str] = &["fonts", "cache"];

fn env<'a>(req: &'a SpawnRequest, name: &str) -> Option<&'a OsStr> {
    req.env
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_os_str())
}

/// The private temp: the `readWrite` grant that `TMPDIR` names.
fn tmp(req: &SpawnRequest) -> Result<PathBuf, String> {
    let t = env(req, "TMPDIR").ok_or("the request sets no TMPDIR")?;
    let t = Path::new(t);
    req.grants
        .iter()
        .find(|g| g.path == t && g.mode == FileMode::ReadWrite)
        .map(|g| g.path.clone())
        .ok_or_else(|| format!("TMPDIR {} is not granted read-write", t.display()))
}

fn writes(mode: FileMode) -> bool {
    mode != FileMode::Read
}

/// Covered by the profile's `<dir>/in` (read) or `<dir>/out` (write) rules.
fn covered(dir: &Path, g: &PathGrant) -> bool {
    match g.mode {
        FileMode::Read => g.path.starts_with(dir.join("in")),
        FileMode::Write | FileMode::ReadWrite => g.path.starts_with(dir.join("out")),
    }
}

fn refuse(g: &PathGrant, why: &str) -> String {
    format!("grant {} ({:?}): {why}", g.path.display(), g.mode)
}

/// The `worker` input of a cold job's request (`cwd` = the job directory, strictly inside
/// `<serverData>/jobs`). A warm worker's request (a grant on the jobs root) is refused: per-job
/// grants are Task 12.
pub fn worker_input(req: &SpawnRequest, server_data: &Path) -> Result<WorkerInput, String> {
    let jobs = under(server_data, "jobs");
    let job_dir = &req.cwd;
    if !job_dir.starts_with(&jobs) || job_dir == &jobs {
        return Err(format!(
            "the job directory {} is not inside {}",
            job_dir.display(),
            jobs.display()
        ));
    }
    let tmp = tmp(req)?;
    let mut caches = Vec::new();
    let mut sources = Vec::new();
    for g in &req.grants {
        if g.path == tmp || covered(job_dir, g) {
            continue;
        }
        if g.path.starts_with(&jobs) && writes(g.mode) && !g.path.starts_with(job_dir) {
            return Err(refuse(
                g,
                "a warm worker's grant on the jobs root: per-job grants come with plan 7C Task 12",
            ));
        }
        if writes(g.mode) || g.path.starts_with(job_dir) {
            return Err(refuse(g, "not a place the worker profile grants"));
        }
        if g.path.starts_with(server_data) {
            if !CACHE_DIRS
                .iter()
                .any(|c| g.path.starts_with(under(server_data, c)))
            {
                return Err(refuse(g, "inside server data but not a cache"));
            }
            caches.push(g.path.clone());
        } else {
            sources.push(g.path.clone());
        }
    }
    Ok(WorkerInput {
        tmp,
        caches,
        devices: if req.family == MEDIA_FAMILY {
            gpu_devices()
        } else {
            vec![]
        },
        server_data: server_data.to_owned(),
        scope: WorkerScope::Cold {
            job_dir: job_dir.clone(),
            sources,
        },
    })
}

#[cfg(target_os = "linux")]
fn gpu_devices() -> Vec<DeviceRule> {
    crate::linux::gpu::devices()
}

#[cfg(not(target_os = "linux"))]
fn gpu_devices() -> Vec<DeviceRule> {
    vec![]
}

/// The `plugin` input of a request (sp8 §16.1; sp8b Task 16's grants): `cwd` is the call's or
/// job's directory, the one `read` grant inside `<serverData>/plugins/store/` the package.
pub fn plugin_input(
    req: &SpawnRequest,
    server_data: &Path,
    home: &Path,
) -> Result<PluginInput, String> {
    let call_dir = &req.cwd;
    let tmp = tmp(req)?;
    let store = under(server_data, "plugins/store");
    let mut store_dir = None;
    let mut models = Vec::new();
    let mut system = Vec::new();
    let mut gpu = Vec::new();
    let mut plugin_data = None;
    for g in &req.grants {
        if g.path == tmp || covered(call_dir, g) {
            continue;
        }
        match g.mode {
            FileMode::Read if g.path.starts_with(&store) => {
                if store_dir.replace(g.path.clone()).is_some() {
                    return Err(refuse(g, "a second package directory in the store"));
                }
            }
            FileMode::Read if g.path.starts_with(call_dir) => {
                return Err(refuse(g, "only in/ of the call's directory is read"));
            }
            FileMode::Read if g.path.starts_with(server_data) => models.push(g.path.clone()),
            FileMode::Read => system.push(g.path.clone()),
            FileMode::ReadWrite if g.path.starts_with("/dev") => gpu.push(DeviceRule {
                path: g.path.clone(),
                access: DeviceAccess::ReadWrite,
            }),
            FileMode::ReadWrite if plugin_data.is_none() && !g.path.starts_with(call_dir) => {
                plugin_data = Some(PluginData {
                    dir: g.path.clone(),
                    write: true,
                });
            }
            _ => return Err(refuse(g, "not a place the plugin profile grants")),
        }
    }
    Ok(PluginInput {
        store_dir: store_dir.ok_or("no package directory in the plugin store is granted")?,
        runtime: vec![],
        system,
        onnx_runtime: None,
        models,
        call_dir: call_dir.clone(),
        tmp,
        plugin_data,
        gpu,
        memory_bytes: None,
        server_data: server_data.to_owned(),
        home: home.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use fin_jobhost::spawner::{Profile, Program, Qos, Role};

    use super::*;

    fn g(p: &str, mode: FileMode) -> PathGrant {
        PathGrant {
            path: p.into(),
            mode,
        }
    }

    fn req(family: &str, cwd: &str, tmp: &str, grants: Vec<PathGrant>) -> SpawnRequest {
        SpawnRequest {
            role: Role::Worker,
            profile: Profile::Worker,
            family: family.into(),
            program: Program::Bundled("fin-workers".into()),
            args: vec![],
            env: vec![(OsString::from("TMPDIR"), OsString::from(tmp))],
            cwd: cwd.into(),
            grants,
            qos: Qos::Normal,
            label: "t".into(),
        }
    }

    const SD: &str = "/srv/fin";

    #[test]
    fn a_cold_job_maps_onto_the_worker_input() {
        let r = req(
            "media",
            "/srv/fin/jobs/j1",
            "/srv/fin/jobs/j1/tmp",
            vec![
                g("/srv/fin/jobs/j1/tmp", FileMode::ReadWrite),
                g("/srv/fin/jobs/j1/in/manifest.json", FileMode::Read),
                g("/srv/fin/jobs/j1/out", FileMode::Write),
                g("/srv/fin/fonts", FileMode::Read),
                g("/home/u/p/sources/a.mp4", FileMode::Read),
            ],
        );
        let i = worker_input(&r, Path::new(SD)).unwrap();
        assert_eq!(i.tmp, Path::new("/srv/fin/jobs/j1/tmp"));
        assert_eq!(i.caches, [PathBuf::from("/srv/fin/fonts")]);
        assert_eq!(
            i.scope,
            WorkerScope::Cold {
                job_dir: "/srv/fin/jobs/j1".into(),
                sources: vec!["/home/u/p/sources/a.mp4".into()],
            }
        );
        #[cfg(target_os = "linux")]
        assert!(!i.devices.is_empty(), "media gets the GPU rows");
        let other = req(
            "library",
            "/srv/fin/jobs/j1",
            "/srv/fin/jobs/j1/tmp",
            r.grants.clone(),
        );
        assert!(
            worker_input(&other, Path::new(SD))
                .unwrap()
                .devices
                .is_empty()
        );
    }

    #[test]
    fn a_worker_request_the_profile_cannot_place_is_refused() {
        let ok = || {
            vec![
                g("/srv/fin/jobs/j1/tmp", FileMode::ReadWrite),
                g("/srv/fin/jobs/j1/in/x", FileMode::Read),
            ]
        };
        let sd = Path::new(SD);
        let tmp = "/srv/fin/jobs/j1/tmp";
        // Outside the jobs root, or the jobs root itself.
        for cwd in ["/srv/fin/elsewhere", "/srv/fin/jobs"] {
            assert!(
                worker_input(&req("t", cwd, tmp, ok()), sd).is_err(),
                "{cwd}"
            );
        }
        // No TMPDIR grant.
        assert!(worker_input(&req("t", "/srv/fin/jobs/j1", "/tmp", ok()), sd).is_err());
        for (bad, why) in [
            (g("/srv/fin/jobs", FileMode::ReadWrite), "warm"),
            (
                g("/srv/fin/jobs/j1/in/x", FileMode::Write),
                "in/ is read-only",
            ),
            (
                g("/srv/fin/jobs/j1/out/y", FileMode::Read),
                "out/ is not read",
            ),
            (
                g("/srv/fin/jobs/j1", FileMode::Read),
                "the job directory itself",
            ),
            (g("/home/u/p/x", FileMode::ReadWrite), "a write elsewhere"),
            (
                g("/srv/fin/auth/local.key", FileMode::Read),
                "server data, not a cache",
            ),
        ] {
            let mut gs = ok();
            gs.push(bad);
            let e = worker_input(&req("t", "/srv/fin/jobs/j1", tmp, gs), sd).unwrap_err();
            assert!(e.contains("grant"), "{why}: {e}");
        }
    }

    #[test]
    fn a_plugin_request_maps_onto_the_plugin_input() {
        let r = req(
            "acme.tool",
            "/srv/fin/jobs/c1",
            "/srv/fin/jobs/c1/tmp",
            vec![
                g("/srv/fin/jobs/c1/tmp", FileMode::ReadWrite),
                g("/srv/fin/plugins/store/acme.tool@1.0.0", FileMode::Read),
                g("/srv/fin/jobs/c1/in", FileMode::Read),
                g("/srv/fin/jobs/c1/out", FileMode::Write),
                g("/usr/lib", FileMode::Read),
                g("/srv/fin/models/m.onnx", FileMode::Read),
                g("/dev/dri/renderD128", FileMode::ReadWrite),
                g("/home/u/p/.finisher/plugins/acme.tool", FileMode::ReadWrite),
            ],
        );
        let i = plugin_input(&r, Path::new(SD), Path::new("/home/u")).unwrap();
        assert_eq!(
            i.store_dir,
            Path::new("/srv/fin/plugins/store/acme.tool@1.0.0")
        );
        assert_eq!(i.system, [PathBuf::from("/usr/lib")]);
        assert_eq!(i.models, [PathBuf::from("/srv/fin/models/m.onnx")]);
        assert_eq!(i.gpu.len(), 1);
        assert_eq!(
            i.plugin_data,
            Some(PluginData {
                dir: "/home/u/p/.finisher/plugins/acme.tool".into(),
                write: true
            })
        );
        // No package, two packages, a second data directory: refused.
        let base = r.grants[..1].to_vec();
        let pkg = g("/srv/fin/plugins/store/a@1", FileMode::Read);
        for extra in [
            vec![],
            vec![pkg.clone(), g("/srv/fin/plugins/store/b@1", FileMode::Read)],
            vec![
                pkg.clone(),
                g("/home/u/a", FileMode::ReadWrite),
                g("/home/u/b", FileMode::ReadWrite),
            ],
        ] {
            let mut gs = base.clone();
            gs.extend(extra);
            let r = req("x", "/srv/fin/jobs/c1", "/srv/fin/jobs/c1/tmp", gs);
            assert!(plugin_input(&r, Path::new(SD), Path::new("/home/u")).is_err());
        }
    }
}
