//! A warm worker under the `worker` profile (plan 7C Task 12, Decision 19), with the job host's
//! `SandboxSpawner` over the real `fin-workers test --warm`: granted its own directory (and its
//! `jobs/`, where each job visits) and one project's sources, it reads the job visiting it and
//! that project, and is refused another job's directory, the jobs root, another project and
//! server data. Each denial is also run under a spawner that passes, as the control.
#![cfg(target_os = "linux")]
#![allow(clippy::disallowed_methods)]

mod host_support;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use fin_jobhost::*;
use fin_jobs::{FileMode, HostMsg};
use fin_sandbox::linux::landlock::{Support, query};
use fin_sandbox::spawner::{Decision, SandboxSpawner};
use host_support::{TestHost, host_with};
use serde_json::{Value, json};

const EACCES: i64 = libc::EACCES as i64;

struct Server {
    _d: tempfile::TempDir,
    sd: PathBuf,
    worker: PathBuf,
    a: PathBuf,
    b: PathBuf,
}

impl Server {
    fn new() -> Server {
        let _d = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(_d.path()).unwrap();
        let sd = root.join("data");
        let worker = sd.join("jobs/warm-t-1");
        for d in ["tmp", "jobs/j1/in"] {
            std::fs::create_dir_all(worker.join(d)).unwrap();
        }
        std::fs::write(worker.join("jobs/j1/in/x"), b"mine").unwrap();
        std::fs::create_dir_all(sd.join("jobs/j9/in")).unwrap();
        std::fs::write(sd.join("jobs/j9/in/x"), b"another job").unwrap();
        std::fs::create_dir_all(sd.join("auth")).unwrap();
        std::fs::write(sd.join("auth/local.key"), b"key").unwrap();
        let src = |p: &str| {
            let d = root.join(p).join("sources");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("clip.mov"), p).unwrap();
            d
        };
        let (a, b) = (src("A"), src("B"));
        Server {
            _d,
            sd,
            worker,
            a,
            b,
        }
    }

    fn host(&self, worker: Decision) -> TestHost {
        let sd = self.sd.clone();
        host_with(move |o| {
            o.spawner = Arc::new(SandboxSpawner::with(Some(sd), None, worker, Decision::Pass));
        })
    }

    /// `warm-worker.ts`'s request: `tmp/` and `jobs/` of its directory, project A's sources.
    fn req(&self) -> SpawnRequest {
        let tmp = self.worker.join("tmp");
        let g = |p: PathBuf, mode| PathGrant { path: p, mode };
        SpawnRequest {
            role: Role::Worker,
            profile: Profile::Worker,
            family: "test".into(),
            program: Program::Bundled("fin-workers".into()),
            args: vec!["test".into(), "--warm".into()],
            env: vec![(OsString::from("TMPDIR"), tmp.clone().into_os_string())],
            cwd: self.worker.clone(),
            grants: vec![
                g(tmp, FileMode::ReadWrite),
                g(self.worker.join("jobs"), FileMode::ReadWrite),
                g(self.a.clone(), FileMode::Read),
            ],
            qos: Qos::Normal,
            label: "warm test".into(),
        }
    }

    /// One `test.warm` job's probes on a new warm worker under `decision`: (selfSandbox, errnos).
    fn probe(&self, decision: Decision) -> (Option<String>, Value) {
        let mut h = self.host(decision);
        let g = h.group();
        let p = h.spawn(g, self.req()).expect("spawns");
        let hello = h.hello(p);
        let paths = [
            self.worker.join("jobs/j1/in/x"),
            self.a.join("clip.mov"),
            self.sd.join("jobs/j9/in/x"),
            self.sd.join("jobs"),
            self.b.join("clip.mov"),
            self.sd.join("auth/local.key"),
        ];
        let probe: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        let HostMsg::Start(mut start) =
            h.start_msg("j1", "test.warm", json!({ "value": 1, "probe": probe }))
        else {
            unreachable!()
        };
        start.dirs.job = self.worker.join("jobs/j1");
        start.dirs.tmp = self.worker.join("jobs/j1/tmp");
        h.host.send(p, &HostMsg::Start(start)).unwrap();
        let v = loop {
            match h.message(p) {
                fin_jobs::WorkerMsg::Result { value, .. } => break value,
                fin_jobs::WorkerMsg::Error { error, .. } => panic!("{error:?}"),
                _ => {}
            }
        };
        (hello.self_sandbox, v["probes"].clone())
    }
}

#[test]
fn a_warm_worker_reads_only_the_job_visiting_it_and_its_project() {
    if !matches!(query(), Support::Abi(_)) {
        assert_ne!(
            std::env::var("FIN_REQUIRE_LANDLOCK").as_deref(),
            Ok("1"),
            "FIN_REQUIRE_LANDLOCK=1 but Landlock answers {:?}",
            query()
        );
        eprintln!("skipped: Landlock is not available");
        return;
    }
    let s = Server::new();
    let h = s.host(Decision::Apply);
    assert!(h.host.caps().per_job_grants);
    let (sandbox, confined) = s.probe(Decision::Apply);
    assert_eq!(sandbox.as_deref(), Some("worker"));
    assert_eq!(
        confined,
        json!([null, null, EACCES, EACCES, EACCES, EACCES]),
        "its job, its project; not another job, the jobs root, another project, server data"
    );
    // Control: a spawner that passes reads them all.
    assert!(!s.host(Decision::Pass).host.caps().per_job_grants);
    let (sandbox, open) = s.probe(Decision::Pass);
    assert_eq!(sandbox, None);
    assert_eq!(open, json!([null, null, null, null, null, null]));
}
