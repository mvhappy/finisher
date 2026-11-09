//! From the job host's grants to a profile input (plan 7C Task 7, "Design"). Pure: every
//! grant is placed in the profile's input or the request is refused, so nothing is widened or
//! dropped silently.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use fin_jobhost::spawner::{PathGrant, Role, SpawnRequest};
use fin_jobs::FileMode;

use crate::inputs::{PluginData, PluginInput, WorkerInput, WorkerScope};
use crate::policy::{DeviceAccess, DeviceRule, under};

/// The family whose workers get the GPU rows (sp6 §3.5.1).
pub const MEDIA_FAMILY: &str = "media";
/// The directories of `<serverData>` a worker may be granted to read (its caches): the font
/// cache and the media caches. Any other `read` grant inside server data is refused.
pub const CACHE_DIRS: &[&str] = &["fonts", "cache"];
/// Where a plugin's model files are (`<serverData>/plugins/models`, sp8 8C): the only `read`
/// grants inside server data a plugin may get besides its package.
pub const MODELS_DIR: &str = "plugins/models";
/// The device nodes a plugin may be granted: render nodes only (never `card*`, a tty or
/// `/dev/fuse`).
const RENDER_NODE_PREFIX: &str = "renderD";

fn is_render_node(p: &Path) -> bool {
    p.parent() == Some(Path::new("/dev/dri"))
        && p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(RENDER_NODE_PREFIX))
}

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

/// The prefix of a warm worker's directory name under the jobs root (`warm-<run>-<n>`,
/// `apps/finisherd/src/jobs/warm-worker.ts`).
pub const WARM_DIR_PREFIX: &str = "warm-";

/// Whether `req` is a warm worker's: `cwd` a `warm-…` directory directly in the jobs root,
/// with a read-write grant on its own `jobs/` (where each job it serves visits, Decision 19).
fn is_warm(req: &SpawnRequest, jobs: &Path) -> bool {
    req.cwd.parent() == Some(jobs)
        && req
            .cwd
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(WARM_DIR_PREFIX))
        && req
            .grants
            .iter()
            .any(|g| g.path == req.cwd.join("jobs") && g.mode == FileMode::ReadWrite)
}

/// The `worker` input of a request: a cold job's (`cwd` = the job directory, strictly inside
/// `<serverData>/jobs`) or a warm worker's (`cwd` = its own directory there, Decision 19: the
/// directory and its `jobs/`, never the jobs root, plus the read roots of its one project).
pub fn worker_input(req: &SpawnRequest, server_data: &Path) -> Result<WorkerInput, String> {
    if matches!(req.role, Role::Helper { .. }) {
        // A helper's directory is `<job>/helpers/<id>/` and its grants are its own files:
        // mapping them is a follow-up before the profile is ready (plan 7C Task 16).
        return Err("a helper process under the worker profile (not mapped yet)".into());
    }
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
    let warm = is_warm(req, &jobs);
    let mut caches = Vec::new();
    let mut sources = Vec::new();
    for g in &req.grants {
        if g.path == tmp
            || (warm && g.path == job_dir.join("jobs") && g.mode == FileMode::ReadWrite)
            || (!warm && covered(job_dir, g))
        {
            continue;
        }
        if g.path == jobs {
            return Err(refuse(
                g,
                "the jobs root (a warm worker gets its own directory, Decision 19)",
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
        scope: if warm {
            WorkerScope::Warm {
                worker_dir: job_dir.clone(),
                library_roots: sources,
            }
        } else {
            WorkerScope::Cold {
                job_dir: job_dir.clone(),
                sources,
            }
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
    let jobs = under(server_data, "jobs");
    if !call_dir.starts_with(server_data) || call_dir == server_data || call_dir == &jobs {
        return Err(format!(
            "the call's directory {} is not one directory of server data",
            call_dir.display()
        ));
    }
    let tmp = tmp(req)?;
    let store = under(server_data, "plugins/store");
    let models_dir = under(server_data, MODELS_DIR);
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
            FileMode::Read if g.path.starts_with(&models_dir) => models.push(g.path.clone()),
            FileMode::Read if g.path.starts_with(server_data) => {
                return Err(refuse(
                    g,
                    "inside server data but not the plugin's package or a model",
                ));
            }
            FileMode::Read => system.push(g.path.clone()),
            FileMode::ReadWrite if is_render_node(&g.path) => gpu.push(DeviceRule {
                path: g.path.clone(),
                access: DeviceAccess::ReadWrite,
            }),
            // The per-project data directory: never in server data (another job, the jobs
            // root, the store) and never a device.
            FileMode::ReadWrite
                if plugin_data.is_none()
                    && !g.path.starts_with(server_data)
                    && !g.path.starts_with("/dev") =>
            {
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
            (g("/srv/fin/jobs", FileMode::ReadWrite), "the jobs root"),
            (g("/srv/fin/jobs", FileMode::Read), "the jobs root"),
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
            if why == "the jobs root" {
                assert!(e.contains(why), "{e}");
            }
        }
        // A helper is not mapped yet.
        let mut h = req("t", "/srv/fin/jobs/j1/helpers/a", tmp, ok());
        h.role = Role::Helper {
            kind: fin_jobs::HelperKind::Codec,
            id: "a".into(),
        };
        assert!(worker_input(&h, sd).unwrap_err().contains("helper"));
    }

    #[test]
    fn a_warm_worker_gets_its_own_directory_and_its_projects_roots() {
        let sd = Path::new(SD);
        let w = "/srv/fin/jobs/warm-ab12-1";
        let tmp = "/srv/fin/jobs/warm-ab12-1/tmp";
        let base = || {
            vec![
                g(tmp, FileMode::ReadWrite),
                g("/srv/fin/jobs/warm-ab12-1/jobs", FileMode::ReadWrite),
                g("/srv/fin/fonts", FileMode::Read),
                g("/home/u/p/sources", FileMode::Read),
                g("/mnt/library", FileMode::Read),
            ]
        };
        let i = worker_input(&req("media", w, tmp, base()), sd).unwrap();
        assert_eq!(i.caches, [PathBuf::from("/srv/fin/fonts")]);
        assert_eq!(
            i.scope,
            WorkerScope::Warm {
                worker_dir: w.into(),
                library_roots: vec!["/home/u/p/sources".into(), "/mnt/library".into()],
            }
        );
        let policy = crate::profile::worker(&i).unwrap();
        assert!(policy.write.contains(&PathBuf::from(w)));
        assert!(!policy.write.contains(&PathBuf::from("/srv/fin/jobs")));
        // Without its `jobs/` grant, or not named `warm-…`, or not directly in the jobs root,
        // it is a cold request, whose `cwd` grant of `jobs/` is refused.
        let mut no_jobs = base();
        no_jobs.remove(1);
        assert!(matches!(
            worker_input(&req("media", w, tmp, no_jobs), sd)
                .unwrap()
                .scope,
            WorkerScope::Cold { .. }
        ));
        for cwd in ["/srv/fin/jobs/j1", "/srv/fin/jobs/x/warm-ab12-1"] {
            let gs = vec![
                g(&format!("{cwd}/tmp"), FileMode::ReadWrite),
                g(&format!("{cwd}/jobs"), FileMode::ReadWrite),
            ];
            assert!(
                worker_input(&req("media", cwd, &format!("{cwd}/tmp"), gs), sd).is_err(),
                "{cwd}"
            );
        }
        // Never the jobs root, another job, a write elsewhere or server data outside the caches.
        for bad in [
            g("/srv/fin/jobs", FileMode::ReadWrite),
            g("/srv/fin/jobs/j9/in", FileMode::Read),
            g("/srv/fin/jobs/warm-ab12-2/jobs", FileMode::ReadWrite),
            g("/home/u/p/sources", FileMode::ReadWrite),
            g("/srv/fin/auth", FileMode::Read),
            g("/srv/fin/jobs/warm-ab12-1", FileMode::Read),
        ] {
            let mut gs = base();
            gs.push(bad.clone());
            assert!(
                worker_input(&req("media", w, tmp, gs), sd).is_err(),
                "{bad:?}"
            );
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
                g("/srv/fin/plugins/models/m.onnx", FileMode::Read),
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
        assert_eq!(i.models, [PathBuf::from("/srv/fin/plugins/models/m.onnx")]);
        assert_eq!(i.gpu.len(), 1);
        assert_eq!(
            i.plugin_data,
            Some(PluginData {
                dir: "/home/u/p/.finisher/plugins/acme.tool".into(),
                write: true
            })
        );
        // No package, two packages, a second data directory, anything else of server data
        // (another job, the jobs root, `auth/`), a device other than a render node: refused.
        let base = r.grants[..1].to_vec();
        let pkg = g("/srv/fin/plugins/store/a@1", FileMode::Read);
        let with_pkg = |x: PathGrant| vec![pkg.clone(), x];
        for extra in [
            vec![],
            vec![pkg.clone(), g("/srv/fin/plugins/store/b@1", FileMode::Read)],
            vec![
                pkg.clone(),
                g("/home/u/a", FileMode::ReadWrite),
                g("/home/u/b", FileMode::ReadWrite),
            ],
            with_pkg(g("/srv/fin/auth", FileMode::Read)),
            with_pkg(g("/srv/fin/jobs/other/in", FileMode::Read)),
            with_pkg(g("/srv/fin/jobs", FileMode::ReadWrite)),
            with_pkg(g("/srv/fin/jobs/other/out", FileMode::ReadWrite)),
            with_pkg(g("/dev/tty1", FileMode::ReadWrite)),
            with_pkg(g("/dev/fuse", FileMode::ReadWrite)),
            with_pkg(g("/dev/dri/card0", FileMode::ReadWrite)),
        ] {
            let mut gs = base.clone();
            gs.extend(extra.clone());
            let r = req("x", "/srv/fin/jobs/c1", "/srv/fin/jobs/c1/tmp", gs);
            assert!(
                plugin_input(&r, Path::new(SD), Path::new("/home/u")).is_err(),
                "{extra:?}"
            );
        }
        // The call's directory is one directory of server data.
        for cwd in ["/home/u/x", "/srv/fin", "/srv/fin/jobs"] {
            let r = req("x", cwd, "/srv/fin/jobs/c1/tmp", r.grants.clone());
            assert!(
                plugin_input(&r, Path::new(SD), Path::new("/home/u")).is_err(),
                "{cwd}"
            );
        }
    }
}
