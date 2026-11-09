use std::io;
use std::path::{Path, PathBuf};

use super::*;
use crate::enumerate::Kind;
use crate::inputs::PluginData;
use crate::windows::capability::{TOOLCHAIN, capability_sid};

/// Every directory is empty (nothing to expand around).
struct Empty;

impl Tree for Empty {
    fn children(&self, _dir: &Path) -> io::Result<Vec<(PathBuf, Kind)>> {
        Ok(vec![])
    }
}

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\r" } else { "/r" })
}

fn g(path: PathBuf, trustee: Trustee, mask: u32, scope: Scope) -> AclGrant {
    AclGrant {
        path,
        trustee,
        mask,
        scope,
    }
}

fn sid(name: &str) -> Trustee {
    Trustee::Sid(capability_sid(name))
}

fn sorted(mut v: Vec<AclGrant>) -> Vec<AclGrant> {
    v.sort_by(|a, b| a.path.cmp(&b.path).then(a.mask.cmp(&b.mask)));
    v
}

#[test]
fn a_cold_worker_gets_its_jobs_roots_and_the_family_its_program_and_caches() {
    let r = root();
    let data = r.join("data");
    let job = data.join("jobs").join("j1");
    let i = WorkerInput {
        tmp: job.join("tmp"),
        caches: vec![data.join("fonts")],
        devices: vec![],
        server_data: data.clone(),
        scope: WorkerScope::Cold {
            job_dir: job.clone(),
            sources: vec![r.join("p").join("sources")],
        },
    };
    let exe = r.join("bin").join("fin-workers.exe");
    let own = sid("finisher-job-j1");
    let family = Trustee::Container(capability::worker_container("media").unwrap());
    let got = worker_in(&i, "media", &exe, &Empty).unwrap();
    assert_eq!(
        sorted(got),
        sorted(vec![
            g(job.join("tmp"), own.clone(), MODIFY, Scope::Owner),
            g(job.join("out"), own.clone(), MODIFY, Scope::Owner),
            g(job.join("in"), own.clone(), READ, Scope::Owner),
            g(r.join("p").join("sources"), own, READ, Scope::Owner),
            g(data.join("fonts"), family.clone(), READ, Scope::Static),
            g(exe, family, READ_EXECUTE, Scope::Static),
        ])
    );
}

#[test]
fn a_warm_worker_gets_its_own_directory_never_the_jobs_root() {
    let r = root();
    let data = r.join("data");
    let w = data.join("jobs").join("warm-1-2");
    let i = WorkerInput {
        tmp: w.join("tmp"),
        caches: vec![],
        devices: vec![],
        server_data: data.clone(),
        scope: WorkerScope::Warm {
            worker_dir: w.clone(),
            library_roots: vec![r.join("p").join("sources")],
        },
    };
    let exe = r.join("bin").join("fin-workers.exe");
    let own = sid("finisher-worker-warm-1-2");
    let got = worker_in(&i, "test", &exe, &Empty).unwrap();
    let family = Trustee::Container(capability::worker_container("test").unwrap());
    assert_eq!(
        sorted(got),
        sorted(vec![
            g(w.join("tmp"), own.clone(), MODIFY, Scope::Owner),
            g(w.join("jobs"), own.clone(), MODIFY, Scope::Owner),
            g(r.join("p").join("sources"), own, READ, Scope::Owner),
            g(exe, family, READ_EXECUTE, Scope::Static),
        ])
    );
}

#[test]
fn a_worker_input_the_profile_refuses_grants_nothing() {
    let r = root();
    let data = r.join("data");
    // A job directory outside the jobs root.
    let i = WorkerInput {
        tmp: data.join("x").join("tmp"),
        caches: vec![],
        devices: vec![],
        server_data: data.clone(),
        scope: WorkerScope::Cold {
            job_dir: data.join("x"),
            sources: vec![],
        },
    };
    assert!(worker_in(&i, "test", &r.join("w.exe"), &Empty).is_err());
}

#[test]
fn a_plugin_gets_its_call_its_package_and_the_toolchain_its_runtime() {
    let r = root();
    let data = r.join("data");
    let store = data.join("plugins").join("store").join("acme-1");
    let call = data.join("jobs").join("c1");
    let i = PluginInput {
        store_dir: store.clone(),
        runtime: vec![r.join("node")],
        system: vec![],
        onnx_runtime: None,
        models: vec![data.join("plugins").join("models").join("m1")],
        call_dir: call.clone(),
        tmp: call.join("tmp"),
        plugin_data: Some(PluginData {
            dir: data.join("plugins").join("data").join("acme"),
            write: true,
        }),
        gpu: vec![],
        memory_bytes: None,
        server_data: data.clone(),
        home: r.join("home"),
    };
    let exe = r.join("node").join("node.exe");
    let own = sid("finisher-call-c1");
    let tools = sid(TOOLCHAIN);
    let container = Trustee::Container(capability::plugin_container(
        &store,
        Some(&data.join("plugins").join("data").join("acme")),
    ));
    let got = plugin_in(&i, &exe, &Empty).unwrap();
    assert_eq!(
        sorted(got),
        sorted(vec![
            // The program lies in the runtime: one grant covers both (the roots are minimal).
            g(r.join("node"), tools, READ_EXECUTE, Scope::Static),
            g(call.join("in"), own.clone(), READ, Scope::Owner),
            g(call.join("out"), own.clone(), MODIFY, Scope::Owner),
            g(call.join("tmp"), own, MODIFY, Scope::Owner),
            g(store, container.clone(), READ_EXECUTE, Scope::Static),
            g(
                data.join("plugins").join("models").join("m1"),
                container.clone(),
                READ,
                Scope::Static
            ),
            g(
                data.join("plugins").join("data").join("acme"),
                container,
                MODIFY,
                Scope::Static
            ),
        ])
    );
}

#[test]
fn a_visiting_job_is_granted_to_its_warm_worker_only_inside_its_jobs() {
    let w = root().join("data").join("jobs").join("warm-1-2");
    let job = w.join("jobs").join("01J");
    assert_eq!(
        visit(&w, &job).unwrap(),
        g(
            job.clone(),
            sid("finisher-worker-warm-1-2"),
            MODIFY,
            Scope::Owner
        )
    );
    // Not inside the worker's `jobs/`: the worker's own directory, another worker's, deeper.
    for bad in [
        w.clone(),
        w.join("tmp").join("01J"),
        w.join("jobs").join("01J").join("in"),
        root()
            .join("data")
            .join("jobs")
            .join("warm-9")
            .join("jobs")
            .join("01J"),
    ] {
        assert!(visit(&w, &bad).is_err(), "{}", bad.display());
    }
}
